use std::{
    fs::File,
    io::BufWriter,
    ops::{Deref, DerefMut},
};

use anyhow::Context;
use fimg::{pixels::convert::RGB, Image};
use fontdue::Font;
use png::{ColorType, Encoder};
use resource::{resource, Resource};
use wasmtime::*;

use crate::ProgramOptions;

use super::{clone_c_string, JumpTableBuilder, MemoryExt, SdkState};

// MARK: Jump Table

pub fn build_display_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    // vexDisplayForegroundColor
    builder.insert(0x640, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.foreground_color = RGB::from_u32(col);
    });

    // vexDisplayBackgroundColor
    builder.insert(0x644, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.background_color = RGB::from_u32(col);
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
    /*builder.insert(
        0x6c0,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;
            let size = caller
                .data_mut()
                .display
                .calculate_string_size(&string, FontType::Normal)
                .unwrap();
            Ok(size.width as u32)
        },
    );

    // vexDisplayStringHeightGet
    builder.insert(0x6c4, move |_string_ptr: i32| {
        Ok(FontType::Normal.line_height() as u32)
    });*/

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

    /*// vexDisplayVPrintf
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
            caller
                .data_mut()
                .display
                .write_text(
                    &format,
                    (x_pos, y_pos),
                    TextOptions {
                        transparent: opaque == 0,
                        ..Default::default()
                    },
                )
                .unwrap();
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
            caller
                .data_mut()
                .display
                .write_text(
                    &format,
                    TextLine(line_number).coords(),
                    TextOptions::default(),
                )
                .unwrap();
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
            caller
                .data_mut()
                .display
                .write_text(
                    &format,
                    (x_pos, y_pos),
                    TextOptions {
                        font_type: FontType::Small,
                        ..Default::default()
                    },
                )
                .unwrap();
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
                .draw_buffer(
                    &buffer,
                    (x1 as usize, y1 as usize),
                    (x2 as usize, y2 as usize),
                    stride as usize,
                )
                .unwrap();
            Ok(())
        },
    );*/
}

// MARK: API

pub enum Path {
    Rect { x1: i32, y1: i32, x2: i32, y2: i32 },
    Circle { cx: i32, cy: i32, radius: i32 },
}

impl Path {
    fn draw<T: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        canvas: &mut Image<T, 3>,
        stroke: bool,
        color: RGB,
    ) {
        match self {
            &Path::Rect { x1, y1, x2, y2 } => {
                let coords = (x1 as u32, y1 as u32);
                let width = (x2 - x1) as u32;
                let height = (y2 - y1) as u32;
                if stroke {
                    canvas.r#box(coords, width, height, color);
                } else {
                    canvas.filled_box(coords, width, height, color);
                }
            }
            &Path::Circle { cx, cy, radius } => {
                if stroke {
                    canvas.border_circle((cx, cy), radius, color);
                } else {
                    canvas.circle((cx, cy), radius, color);
                }
            }
        }
    }
}

pub const DISPLAY_HEIGHT: u32 = 272;
pub const DISPLAY_WIDTH: u32 = 480;
pub const HEADER_HEIGHT: u32 = 32;

pub const BLACK: RGB = [0, 0, 0];
pub const WHITE: RGB = [255, 255, 255];
pub const HEADER_BG: RGB = [0x00, 0x99, 0xCC];

pub struct Display {
    pub foreground_color: RGB,
    pub background_color: RGB,
    pub canvas: Image<Box<[u8]>, 3>,
    mono_font: Resource<[u8]>,
    program_options: ProgramOptions,
    render_mode: RenderMode,
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
    pub fn new(program_options: ProgramOptions) -> Self {
        let canvas = fimg::builder::Builder::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
            .fill(program_options.default_bg_color());

        let mut display = Display {
            foreground_color: program_options.default_fg_color(),
            background_color: program_options.default_bg_color(),
            mono_font: resource!("/fonts/NotoMono-Regular.ttf"),
            canvas,
            program_options,
            render_mode: RenderMode::default(),
        };

        display.erase(); // The bitmap is transparent by default, erase it to make it black.
        display
    }

    fn draw_buffer(
        &mut self,
        buf: &[u8],
        top_left: (usize, usize),
        bot_right: (usize, usize),
        stride: usize,
    ) {
        todo!()
    }

    /// Draws the blue program header at the top of the display.
    fn draw_header(&mut self) {
        self.filled_box((0, 0), DISPLAY_WIDTH, HEADER_HEIGHT, HEADER_BG);
    }

    /// Saves the display to a PNG file.
    pub fn render(&mut self, explicitly_requested: bool) {
        if explicitly_requested {
            self.render_mode = RenderMode::DoubleBuffered;
        } else if self.render_mode == RenderMode::DoubleBuffered {
            return;
        }

        self.draw_header();
        // self.flush();

        self.save("display.png");
    }

    pub fn disable_double_buffer(&mut self) {
        self.render_mode = RenderMode::Immediate;
    }

    /// Erases the display by filling it with the background color.
    pub fn erase(&mut self) {
        self.canvas.filled_box(
            (0, 0),
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
            self.program_options.default_bg_color(),
        );
    }

    /// Draws or strokes a shape on the display, in the foreground color.
    pub fn draw(&mut self, mut shape: Path, stroke: bool) {
        shape.draw(&mut self.canvas, stroke, self.foreground_color);
        self.render(false);
    }

    /*/// Calculates the shape of the area behind a text layout, so that it can be drawn on top of a background color.
    fn calculate_text_background(
        text_layout: &PietTextLayout,
        coords: (f64, f64),
        font_size: FontType,
    ) -> Rect {
        let size = text_layout.size();
        Rect::new(
            coords.0,
            coords.1 + font_size.backdrop_y_offset(),
            coords.0 + size.width,
            coords.1 + font_size.line_height() + font_size.backdrop_y_offset(),
        )
    }

    fn with_text_layout<T>(
        &mut self,
        text: &str,
        font_type: FontType,
        func: impl FnOnce(Piet, PietTextLayoutBuilder) -> Result<T, piet::Error>,
    ) -> Result<T, piet::Error> {
        let text = text.replace('\n', ".");
        #[cfg(not(target_os = "windows"))]
        let font = self.mono_font.clone();
        {
            let mut rc = self.render_context();
            // apparently you need to load the font every time on Windows,
            // this obviously isn't good for performance but is there really an alternative?
            // might come back to this later
            #[cfg(target_os = "windows")]
            let font = Display::load_font(&mut rc)?;
            let text_layout = rc
                .text()
                .new_text_layout(text)
                .font(font, font_type.font_size());
            func(rc, text_layout)
        }
    }

    /// Writes text to the display at a given line number.
    ///
    /// # Arguments
    ///
    /// * `opaque`: Whether the text should be drawn on top of a background color.
    pub fn write_text(
        &mut self,
        text: &str,
        coords: (i32, i32),
        options: TextOptions,
    ) -> Result<(), piet::Error> {
        let coords = (
            coords.0 as f64,
            (coords.1 as f64) + options.font_type.y_offset(),
        );
        let fg = self.foreground_color;
        let bg = self.background_color;

        self.with_text_layout(text, options.font_type, |mut rc, layout| {
            let layout = layout.text_color(fg).build()?;
            if !options.transparent {
                rc.fill(
                    Display::calculate_text_background(&layout, coords, options.font_type),
                    &bg,
                );
            }

            rc.draw_text(&layout, coords);
            rc.finish()?;
            Ok(())
        })?;

        self.render(false)?;
        Ok(())
    }

    pub fn calculate_string_size(
        &mut self,
        text: &str,
        font_size: FontType,
    ) -> Result<Size, piet::Error> {
        self.with_text_layout(text, font_size, |mut rc, layout| {
            let mut size = layout.build()?.size();
            size.height = font_size.line_height();
            rc.finish()?;
            Ok(size)
        })
    }*/
}

pub trait RGBExt {
    /// Creates a `Color` from a 32-bit RGB value.
    ///
    /// ```
    /// # use femtovg::Color;
    /// assert_eq!(Color::rgb_u32(0x00FF00), Color::rgb(0x00, 0xFF, 0x00));
    /// ```
    ///
    /// # Arguments
    ///
    /// * `rgb` - The 32-bit RGB value representing the color.
    ///
    /// # Returns
    ///
    /// A `Color` instance representing the specified RGB value.
    fn from_u32(rgb: u32) -> RGB;
}

impl RGBExt for RGB {
    fn from_u32(rgb: u32) -> RGB {
        [
            ((rgb >> 16) & 0xFF) as u8,
            ((rgb >> 8) & 0xFF) as u8,
            (rgb & 0xFF) as u8,
        ]
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
pub enum FontType {
    Small,
    #[default]
    Normal,
    Big,
}

impl FontType {
    pub fn font_size(&self) -> f64 {
        match self {
            FontType::Small => 13.0,
            FontType::Normal => 16.0,
            FontType::Big => 32.0,
        }
    }

    pub fn y_offset(&self) -> f64 {
        match self {
            FontType::Small => -4.0,
            FontType::Normal => -2.0,
            FontType::Big => -1.0,
        }
    }

    pub fn line_height(&self) -> f64 {
        match self {
            FontType::Small => 13.0,
            FontType::Normal => 2.0,
            FontType::Big => 2.0,
        }
    }

    pub fn backdrop_y_offset(&self) -> f64 {
        match self {
            FontType::Small => 4.0,
            FontType::Normal => 0.0,
            FontType::Big => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextOptions {
    pub transparent: bool,
    pub font_type: FontType,
}

/*pub trait Strokable {
    /// Creates a Rect that can be used to draw the border of `other`.
    fn stroking(other: Self) -> Self;
}

impl Strokable for Rect {
    fn stroking(mut other: Self) -> Self {
        other.x0 += 0.5;
        other.y0 += 0.5;
        other.x1 += 0.5;
        other.y1 += 0.5;
        other
    }
}

impl Strokable for Circle {
    fn stroking(mut other: Self) -> Self {
        other.radius += 0.5;
        other
    }
}*/

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderMode {
    #[default]
    Immediate,
    DoubleBuffered,
}
