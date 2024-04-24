use std::{
    cell::Cell,
    hash::DefaultHasher,
    ops::{Deref, DerefMut},
    sync::Arc,
    thread::{sleep, spawn},
    time::{Duration, Instant},
};

use anyhow::Context;
use fimg::{pixels::convert::RGB, Image, Pack};
use resource::{resource, Resource};
use rusttype::{point, Font, Point, PositionedGlyph, Rect, Scale};
use single_value_channel::{channel_starting_with, Receiver, Updater};
use wasmtime::*;

use crate::ProgramOptions;

use super::{clone_c_string, JumpTableBuilder, MemoryExt, SdkState};

// MARK: Jump Table

pub fn build_display_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    // vexDisplayForegroundColor
    builder.insert(0x640, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.foreground_color = RGB::unpack(col);
    });

    // vexDisplayBackgroundColor
    builder.insert(0x644, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.background_color = RGB::unpack(col);
    });

    // vexDisplayRectDraw
    builder.insert(
        0x668,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller
                .data_mut()
                .display
                .draw(Path::Rect { x1, y1, x2, y2 }, true);
        },
    );

    // vexDisplayRectFill
    builder.insert(
        0x670,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller
                .data_mut()
                .display
                .draw(Path::Rect { x1, y1, x2, y2 }, false);
        },
    );

    // vexDisplayCircleDraw
    builder.insert(
        0x674,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            caller
                .data_mut()
                .display
                .draw(Path::Circle { cx, cy, radius }, true);
        },
    );

    // vexDisplayCircleFill
    builder.insert(
        0x67c,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            caller
                .data_mut()
                .display
                .draw(Path::Circle { cx, cy, radius }, false);
        },
    );

    // vexDisplayStringWidthGet
    builder.insert(
        0x6c0,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;
            let size = caller.data().display.calculate_string_size(
                string,
                TextOptions {
                    font_type: FontType::Normal,
                    ..Default::default()
                },
            );
            Ok(size.x as u32)
        },
    );

    // vexDisplayStringHeightGet
    builder.insert(
        0x6c4,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;
            let size = caller.data().display.calculate_string_size(
                string,
                TextOptions {
                    font_type: FontType::Normal,
                    ..Default::default()
                },
            );
            Ok(size.y as u32)
        },
    );

    // vexDisplayRender
    builder.insert(
        0x7a0,
        move |mut caller: Caller<'_, SdkState>, _vsync_wait: i32, _run_scheduler: i32| {
            caller.data_mut().display.render(true);
        },
    );

    // vexDisplayDoubleBufferDisable
    builder.insert(0x7a4, move |mut caller: Caller<'_, SdkState>| {
        caller.data_mut().display.disable_double_buffer();
    });

    // vexDisplayVPrintf
    builder.insert(
        0x680,
        move |mut caller: Caller<'_, SdkState>,
              x_pos: i32,
              y_pos: i32,
              opaque: i32,
              format_ptr: u32,
              _args: u32|
              -> Result<()> {
            let format = clone_c_string!(format_ptr as usize, from caller using memory)?;
            caller.data_mut().display.write_text(
                format,
                (x_pos, y_pos),
                opaque == 0,
                TextOptions::default(),
            );
            Ok(())
        },
    );

    // vexDisplayVString
    builder.insert(
        0x684,
        move |mut caller: Caller<'_, SdkState>,
              line_number: i32,
              format_ptr: u32,
              _args: u32|
              -> Result<()> {
            let format = clone_c_string!(format_ptr as usize, from caller using memory)?;
            caller.data_mut().display.write_text(
                format,
                TextLine(line_number).coords(),
                false,
                TextOptions::default(),
            );
            Ok(())
        },
    );

    // vexDisplayVSmallStringAt
    builder.insert(
        0x6b0,
        move |mut caller: Caller<'_, SdkState>,
              x_pos: i32,
              y_pos: i32,
              format_ptr: u32,
              _args: u32|
              -> Result<()> {
            let format = clone_c_string!(format_ptr as usize, from caller using memory)?;
            caller.data_mut().display.write_text(
                format,
                (x_pos, y_pos),
                false,
                TextOptions {
                    font_type: FontType::Small,
                    ..Default::default()
                },
            );
            Ok(())
        },
    );

    // vexDisplayCopyRect
    builder.insert(
        0x654,
        move |mut caller: Caller<'_, SdkState>,
              x1: i32,
              y1: i32,
              x2: i32,
              y2: i32,
              buffer_ptr: u32,
              stride: u32|
              -> Result<()> {
            let buffer_len = (x2 - x1) as usize * (y2 - y1) as usize * 4;
            let buffer = memory.data(&mut caller)[buffer_ptr as usize..][..buffer_len].to_vec();
            caller
                .data_mut()
                .display
                .draw_buffer(&buffer, (x1, y1), (x2, y2), stride);
            Ok(())
        },
    );
}

// MARK: API

pub enum Path {
    Rect { x1: i32, y1: i32, x2: i32, y2: i32 },
    Circle { cx: i32, cy: i32, radius: i32 },
}

impl From<Rect<i32>> for Path {
    fn from(rect: Rect<i32>) -> Self {
        Path::Rect {
            x1: rect.min.x,
            y1: rect.min.y,
            x2: rect.max.x,
            y2: rect.max.y,
        }
    }
}

impl Path {
    fn normalize(&mut self) {
        match self {
            Path::Rect { x1, y1, x2, y2 } => {
                *x1 = (*x1).clamp(0, DISPLAY_WIDTH as i32 - 1);
                *y1 = (*y1).clamp(0, DISPLAY_HEIGHT as i32 - 1);
                *x2 = (*x2).clamp(0, DISPLAY_WIDTH as i32 - 1);
                *y2 = (*y2).clamp(0, DISPLAY_HEIGHT as i32 - 1);
            }
            Path::Circle { cx, cy, .. } => {
                *cx = (*cx).clamp(0, DISPLAY_WIDTH as i32 - 1);
                *cy = (*cy).clamp(0, DISPLAY_HEIGHT as i32 - 1);
            }
        }
    }

    fn draw<T: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        canvas: &mut Image<T, 3>,
        stroke: bool,
        color: RGB,
    ) {
        match *self {
            Path::Rect { x1, y1, x2, y2 } => {
                let coords = (x1 as u32, y1 as u32);
                let width = (x2 - x1).try_into().unwrap();
                let height = (y2 - y1).try_into().unwrap();
                if stroke {
                    canvas.r#box(coords, width, height, color);
                } else {
                    canvas.filled_box(coords, width, height, color);
                }
            }
            Path::Circle { cx, cy, radius } => {
                if stroke {
                    canvas.border_circle((cx, cy), radius, color);
                } else {
                    canvas.circle((cx, cy), radius, color);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextLine(pub i32);

impl TextLine {
    pub fn coords(&self) -> (i32, i32) {
        (0, self.0 * 20 + HEADER_HEIGHT as i32)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontFamily {
    #[default]
    UserMono,
    TimerMono,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontType {
    Small,
    #[default]
    Normal,
    Big,
}

impl FontType {
    /// Multiplier for the X axis scale of the font.
    pub fn x_scale() -> f32 {
        0.9
    }

    /// Extra spacing in pixels between characters (x-axis).
    pub fn x_spacing() -> f32 {
        1.1
    }

    /// Font size in pixels.
    pub fn font_size(&self) -> f32 {
        match self {
            FontType::Small => 15.0,
            FontType::Normal => 16.0,
            FontType::Big => 32.0,
        }
    }

    /// Y-axis offset applied before rendering.
    pub fn y_offset(&self) -> i32 {
        match self {
            FontType::Small => -2,
            FontType::Normal => -2,
            FontType::Big => -1,
        }
    }

    /// Line height of the highlighted area behind text.
    pub fn line_height(&self) -> i32 {
        match self {
            FontType::Small => 13,
            FontType::Normal => 2,
            FontType::Big => 2,
        }
    }

    /// Y-axis offset applied to the highlighted area behind text.
    pub fn backdrop_y_offset(&self) -> i32 {
        match self {
            FontType::Small => 2,
            FontType::Normal => 0,
            FontType::Big => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextOptions {
    pub font_type: FontType,
    pub family: FontFamily,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderMode {
    #[default]
    Immediate,
    DoubleBuffered,
}

/// Blends a partially transparent foreground color with a background color.
fn blend_pixel(bg: RGB, fg: RGB, fg_alpha: f32) -> RGB {
    // outputRed = (foregroundRed * foregroundAlpha) + (backgroundRed * (1.0 - foregroundAlpha));

    [
        (fg[0] as f32 * fg_alpha + bg[0] as f32 * (1.0 - fg_alpha)).round() as u8,
        (fg[1] as f32 * fg_alpha + bg[1] as f32 * (1.0 - fg_alpha)).round() as u8,
        (fg[2] as f32 * fg_alpha + bg[2] as f32 * (1.0 - fg_alpha)).round() as u8,
    ]
}

/// Calculates the size of a layout of glyphs.
fn size_of_layout(glyphs: &[PositionedGlyph]) -> Option<Rect<i32>> {
    let last_char = glyphs.last()?;
    let first_char = &glyphs[0];
    let last_bounding_box = last_char.pixel_bounding_box().unwrap();
    let first_bounding_box = first_char.pixel_bounding_box().unwrap();
    Some(Rect {
        min: first_bounding_box.min,
        max: Point {
            x: last_bounding_box.max.x + (FontType::x_spacing() * glyphs.len() as f32) as i32,
            y: last_bounding_box.max.y,
        },
    })
}

// MARK: Display

pub const DISPLAY_HEIGHT: u32 = 272;
pub const DISPLAY_WIDTH: u32 = 480;
pub const HEADER_HEIGHT: u32 = 32;

pub const BLACK: RGB = [0, 0, 0];
pub const WHITE: RGB = [255, 255, 255];
pub const HEADER_BG: RGB = [0x00, 0x99, 0xCC];

type Canvas = Image<Box<[u8]>, 3>;

pub struct Display {
    /// The display's saved foreground color.
    pub foreground_color: RGB,
    /// The display's saved background color.
    pub background_color: RGB,
    /// The display's image buffer.
    pub canvas: Canvas,
    user_mono: Font<'static>,
    /// Font for the program header's timer.
    timer_mono: Font<'static>,
    program_options: ProgramOptions,
    /// Controls when the display is rendered.
    render_mode: RenderMode,
    /// Cache for text layout calculations, to avoid re-calculating the same text layout multiple times in a row.
    text_layout_cache: Cell<Option<(String, TextOptions, Vec<PositionedGlyph<'static>>)>>,
    /// Will be None if this is the render thread's direct display, which instead of "rendering"
    /// by sending the buffer to another thread, simply shows the display to the user.
    render_tx: Option<Updater<Option<Canvas>>>,
    /// The instant at which the program started.
    start_instant: Instant,
}

impl Deref for Display {
    type Target = Image<Box<[u8]>, 3>;

    fn deref(&self) -> &Self::Target {
        &self.canvas
    }
}

impl DerefMut for Display {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.canvas
    }
}

impl Display {
    pub fn new(program_options: ProgramOptions, start_instant: Instant) -> Self {
        let canvas =
            Image::build(DISPLAY_WIDTH, DISPLAY_HEIGHT).fill(program_options.default_bg_color());
        let user_mono =
            Font::try_from_vec(resource!("/fonts/NotoMono-Regular.ttf").to_vec()).unwrap();
        let timer_mono =
            Font::try_from_vec(resource!("/fonts/droid-sans-mono.ttf").to_vec()).unwrap();

        // Start the off-thread renderer to update the timer even while the main thread is blocked.
        let (rx, tx) = channel_starting_with(None);
        Self::start_render_thread(
            Self {
                foreground_color: program_options.default_fg_color(),
                background_color: program_options.default_bg_color(),
                user_mono: user_mono.clone(),
                timer_mono: timer_mono.clone(),
                canvas: canvas.clone(),
                program_options,
                render_mode: RenderMode::DoubleBuffered,
                text_layout_cache: Cell::default(),
                render_tx: None,
                start_instant,
            },
            rx,
        );

        Self {
            foreground_color: program_options.default_fg_color(),
            background_color: program_options.default_bg_color(),
            user_mono,
            timer_mono,
            canvas,
            program_options,
            render_mode: RenderMode::default(),
            text_layout_cache: Cell::default(),
            render_tx: Some(tx),
            start_instant,
        }
    }

    /// Starts the render thread, which renders the program header and handles showing the display to the user.
    fn start_render_thread(mut direct_display: Self, mut rx: Receiver<Option<Canvas>>) {
        spawn(move || {
            // Pretty much a copy of the normal display, but it never calls render().
            // Controls whether the display should be re-rendered. None or >1s ago will always re-render.
            let mut last_update = None::<Instant>;

            while !rx.has_no_updater() {
                // If there's a new frame, render that instead.
                if let Some(canvas) = rx.latest_mut().take() {
                    direct_display.canvas = canvas;
                    last_update = None;
                }

                // No need to re-render unless we have a new frame or should update the timer.
                if last_update.map_or(true, |last| last.elapsed() > Duration::from_secs(1)) {
                    direct_display.draw_header();
                    direct_display.save("display.png");
                    last_update = Some(Instant::now());
                }

                // FPS goal
                sleep(Duration::from_secs_f64(1.0 / 10.0));
            }
        });
    }

    /// Returns the font data for the given font family.
    fn font_family(&self, family: FontFamily) -> &Font<'static> {
        match family {
            FontFamily::UserMono => &self.user_mono,
            FontFamily::TimerMono => &self.timer_mono,
        }
    }

    /// Copies a buffer of pixels to the display.
    fn draw_buffer(
        &mut self,
        buf: &[u8],
        top_left: (i32, i32),
        bot_right: (i32, i32),
        stride: u32,
    ) {
        let mut y = top_left.1;
        for row in buf.chunks((stride * 4) as usize) {
            if y > bot_right.1 {
                break;
            }

            let mut x = top_left.0;
            for pixel in row.chunks(4) {
                let color = RGB::unpack(u32::from_le_bytes(pixel[0..4].try_into().unwrap()));
                if x >= 0 && x < self.width() as i32 && y >= 0 && y < self.height() as i32 {
                    // I didn't see a safe version of this...?
                    // SAFETY: bounds are checked
                    unsafe { self.set_pixel(x.try_into().unwrap(), y.try_into().unwrap(), color) };
                }
                x += 1;
            }
            y += 1;
        }
    }

    /// Draws the blue program header at the top of the display.
    fn draw_header(&mut self) {
        self.filled_box((0, 0), DISPLAY_WIDTH, HEADER_HEIGHT, HEADER_BG);
        let elapsed = self.start_instant.elapsed().as_secs();
        let secs = elapsed % 60;
        let mins = elapsed / 60;
        let time = format!("{:01}:{:02}", mins, secs);
        self.write_text(
            time,
            ((DISPLAY_WIDTH / 2) as i32, 3),
            true,
            TextOptions {
                font_type: FontType::Big,
                family: FontFamily::TimerMono,
            },
        );
    }

    fn normalize_text(text: &str) -> String {
        text.replace('\n', ".")
    }

    /// Sends the display to the render thread.
    pub fn render(&mut self, explicitly_requested: bool) {
        if explicitly_requested {
            self.render_mode = RenderMode::DoubleBuffered;
        } else if self.render_mode == RenderMode::DoubleBuffered {
            return;
        }
        if let Some(tx) = self.render_tx.as_mut() {
            tx.update(Some(self.canvas.clone())).unwrap();
        }
    }

    /// Disables double buffering, causing the display to render after every update.
    pub fn disable_double_buffer(&mut self) {
        assert!(self.render_tx.is_some());
        self.render_mode = RenderMode::Immediate;
    }

    /// Erases the display by filling it with the default background color.
    pub fn erase(&mut self) {
        self.canvas.filled_box(
            (0, 0),
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
            self.program_options.default_bg_color(),
        );
    }

    /// Draws or strokes a shape on the display, using the current foreground color.
    pub fn draw(&mut self, mut shape: Path, stroke: bool) {
        shape.normalize();
        shape.draw(&mut self.canvas, stroke, self.foreground_color);
        self.render(false);
    }

    /// Removes the last text layout from the cache if it matches the given text and options.
    fn take_cached_glyphs_for(
        &self,
        text: &str,
        options: TextOptions,
    ) -> Option<Vec<PositionedGlyph<'static>>> {
        let (cached_text, cached_options, glyphs) = self.text_layout_cache.take()?;
        if text == cached_text && options == cached_options {
            Some(glyphs)
        } else {
            None
        }
    }

    /// Returns the glyphs for the given text, using the given options.
    ///
    /// May either return cached glyphs or calculate them when called.
    fn glyphs_for(&self, text: &str, options: TextOptions) -> Vec<PositionedGlyph<'static>> {
        if let Some(glyphs) = self.take_cached_glyphs_for(text, options) {
            return glyphs;
        }

        let scale = Scale {
            y: options.font_type.font_size(),
            // V5's version of the Noto Mono font is slightly different
            // than the one bundled with the simulator, so we have to apply
            // an scale on the X axis and later move the characters further apart.
            x: options.font_type.font_size() * FontType::x_scale(),
        };
        let font = self.font_family(options.family);
        let v_metrics = font.v_metrics(scale);
        font.layout(text, scale, point(0.0, 0.0 + v_metrics.ascent))
            .collect()
    }

    /// Calculates the shape of the area behind a text layout, so that it can be drawn on top of a background color.
    fn calculate_text_background(
        glyphs: &[PositionedGlyph],
        coords: (i32, i32),
        font_size: FontType,
    ) -> Option<Path> {
        let size = size_of_layout(glyphs)?;
        let mut backdrop = Path::Rect {
            x1: size.min.x + coords.0 - 1,
            y1: coords.1 + font_size.backdrop_y_offset(),
            x2: size.max.x + coords.0 + 1,
            y2: coords.1 + font_size.backdrop_y_offset() + font_size.line_height() - 1,
        };

        backdrop.normalize();
        Some(backdrop)
    }

    /// Writes text to the display at a given line number.
    ///
    /// # Arguments
    ///
    /// * `text`: The text to write to the display.
    /// * `coords`: The coordinates at which to write the text.
    /// * `transparent`: Whether the text should not have a background (highlight) color.
    /// * `options`: The options to use when rendering the text.
    pub fn write_text(
        &mut self,
        mut text: String,
        mut coords: (i32, i32),
        transparent: bool,
        options: TextOptions,
    ) {
        text = Self::normalize_text(&text);
        if text.is_empty() {
            return;
        }

        // The V5's text is all offset vertically from ours, so this adjustment makes it consistent.
        coords.1 += options.font_type.y_offset();

        let fg = self.foreground_color;
        let glyphs = self.glyphs_for(&text, options);

        if !transparent {
            let backdrop =
                Self::calculate_text_background(&glyphs, coords, options.font_type).unwrap();
            backdrop.draw(&mut self.canvas, false, self.background_color);
        }

        for (idx, glyph) in glyphs.iter().enumerate() {
            if let Some(bounding_box) = glyph.pixel_bounding_box() {
                // Draw the glyph into the image per-pixel
                glyph.draw(|mut x, mut y, alpha| {
                    // Apply offsets to make the coordinates image-relative, not text-relative
                    x += bounding_box.min.x as u32
                        + coords.0 as u32
                        // Similar reasoning to when we applied the x scale to the font.
                        + (FontType::x_spacing() * idx as f32) as u32;
                    y += bounding_box.min.y as u32 + coords.1 as u32;

                    if !(x < self.width() && y < self.height()) {
                        return;
                    }

                    // I didn't find a safe version of pixel and set_pixel.
                    // SAFETY: Pixel bounds are checked.
                    unsafe {
                        let old_pixel = self.pixel(x, y);

                        self.set_pixel(
                            x,
                            y,
                            // Taking this power seems to make the alpha blending look better;
                            // otherwise it's not heavy enough.
                            blend_pixel(old_pixel, fg, alpha.powf(0.4).clamp(0.0, 1.0)),
                        );
                    }
                });
            }
        }

        // Add (or re-add) the laid-out glyphs to the cache so they can be used later.
        self.text_layout_cache.set(Some((text, options, glyphs)));
        self.render(false);
    }

    /// Calculates how big a string will be when rendered.
    ///
    /// Caches the result so that the same text and options don't have to be calculated multiple times in a row.
    pub fn calculate_string_size(&self, mut text: String, options: TextOptions) -> Point<i32> {
        text = Self::normalize_text(&text);
        let glyphs = self.glyphs_for(&text, options);
        let size = size_of_layout(&glyphs);
        self.text_layout_cache.set(Some((text, options, glyphs)));
        size.unwrap_or_default().max
    }
}
