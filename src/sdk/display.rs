use std::{
    fs::File,
    io::BufWriter,
    ops::{Deref, DerefMut},
};

use anyhow::Context;
use piet::{
    kurbo::{Circle, Rect, Shape, Size},
    Color, FontFamily, ImageBuf, ImageFormat, InterpolationMode, RenderContext, Text, TextLayout,
    TextLayoutBuilder, TextStorage,
};
use piet_common::{BitmapTarget, Device, PietTextLayoutBuilder};
use piet_common::{Piet, PietTextLayout};
use png::{ColorType, Encoder};
use wasmtime::*;

use crate::ProgramOptions;

use super::{clone_c_string, JumpTableBuilder, MemoryExt, SdkState};

// MARK: Jump Table

pub fn build_display_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    // vexDisplayForegroundColor
    builder.insert(0x640, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.foreground_color = Color::from_rgb_u32(col);
    });

    // vexDisplayBackgroundColor
    builder.insert(0x644, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.background_color = Color::from_rgb_u32(col);
    });

    // vexDisplayRectDraw
    builder.insert(
        0x668,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            let rect = Rect::new(x1 as f64, y1 as f64, x2 as f64, y2 as f64);
            caller.data_mut().display.draw(rect, true).unwrap();
        },
    );

    // vexDisplayRectFill
    builder.insert(
        0x670,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            let rect = Rect::new(x1 as f64, y1 as f64, x2 as f64, y2 as f64);
            caller.data_mut().display.draw(rect, false).unwrap();
        },
    );

    // vexDisplayCircleDraw
    builder.insert(
        0x674,
        move |mut caller: Caller<'_, SdkState>, xc: i32, yc: i32, radius: i32| {
            let circle = Circle::new((xc as f64, yc as f64), radius as f64);
            caller.data_mut().display.draw(circle, true).unwrap();
        },
    );

    // vexDisplayCircleFill
    builder.insert(
        0x67c,
        move |mut caller: Caller<'_, SdkState>, xc: i32, yc: i32, radius: i32| {
            let circle = Circle::new((xc as f64, yc as f64), radius as f64);
            caller.data_mut().display.draw(circle, false).unwrap();
        },
    );

    // vexDisplayStringWidthGet
    builder.insert(
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
    });

    // vexDisplayRender
    builder.insert(
        0x7a0,
        move |mut caller: Caller<'_, SdkState>, _vsync_wait: i32, _run_scheduler: i32| {
            caller.data_mut().display.render(true).unwrap();
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
    );
}

// MARK: API

pub const DISPLAY_HEIGHT: usize = 272;
pub const DISPLAY_WIDTH: usize = 480;
pub const HEADER_HEIGHT: usize = 32;

pub struct Display<'a> {
    pub foreground_color: Color,
    pub background_color: Color,
    pub bitmap: BitmapTarget<'a>,
    mono_font: piet::FontFamily,
    width: usize,
    height: usize,
    program_options: ProgramOptions,
    render_mode: RenderMode,
}

impl<'a> Deref for Display<'a> {
    type Target = BitmapTarget<'a>;

    fn deref(&self) -> &Self::Target {
        &self.bitmap
    }
}

impl<'a> DerefMut for Display<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.bitmap
    }
}

impl<'a> Display<'a> {
    pub fn new(
        width: usize,
        height: usize,
        renderer: &'a mut Device,
        program_options: ProgramOptions,
    ) -> Result<Self, piet::Error> {
        let mut bitmap = renderer.bitmap_target(width, height, 1.0)?;
        let mut display = Display {
            foreground_color: program_options.default_fg_color(),
            background_color: program_options.default_bg_color(),
            mono_font: {
                // For some reason you need to create a render context to load the font.
                let mut rc = bitmap.render_context();
                let mono_font = Display::load_font(&mut rc)?;
                rc.finish()?;
                mono_font
            },
            bitmap,
            width,
            height,
            program_options,
            render_mode: RenderMode::default(),
        };

        display.erase()?; // The bitmap is transparent by default, erase it to make it black.
        Ok(display)
    }

    /// Returns the bundled monospace font.
    fn load_font(rc: &mut Piet) -> Result<FontFamily, piet::Error> {
        let noto_sans_mono = include_bytes!("../../fonts/NotoSansMono-Regular.ttf");
        rc.text().load_font(noto_sans_mono)
    }

    fn draw_buffer(
        &mut self,
        buf: &[u8],
        top_left: (usize, usize),
        bot_right: (usize, usize),
        stride: usize,
    ) -> Result<(), piet::Error> {
        let mut rc = self.render_context();
        let img_width = bot_right.0 - top_left.0;
        let img_height = bot_right.1 - top_left.1;
        let bitmap =
            rc.make_image_with_stride(img_width, img_height, stride, buf, ImageFormat::Rgb)?;
        rc.draw_image(
            &bitmap,
            Rect::new(
                top_left.0 as f64,
                top_left.1 as f64,
                bot_right.0 as f64,
                bot_right.1 as f64,
            ),
            InterpolationMode::Bilinear,
        );
        rc.finish()?;
        Ok(())
    }

    /// Draws the blue program header at the top of the display.
    fn draw_header(&mut self) -> Result<(), piet::Error> {
        const HEADER_BG: Color = Color::rgb8(0x00, 0x99, 0xCC);
        let width = self.width as f64;
        let mut rc = self.render_context();
        rc.fill(Rect::new(0.0, 0.0, width, HEADER_HEIGHT as f64), &HEADER_BG);
        rc.finish()?;
        Ok(())
    }

    /// Saves the display to a PNG file.
    pub fn render(&mut self, explicitly_requested: bool) -> Result<(), piet::Error> {
        if explicitly_requested {
            self.render_mode = RenderMode::DoubleBuffered;
        } else if self.render_mode == RenderMode::DoubleBuffered {
            return Ok(());
        }

        self.draw_header()?;
        let mut data = vec![0; DISPLAY_HEIGHT * DISPLAY_WIDTH * 4];
        self.copy_raw_pixels(ImageFormat::RgbaPremul, &mut data)?;
        piet::util::unpremultiply_rgba(&mut data);
        let file = BufWriter::new(File::create("display.png").map_err(Into::<Box<_>>::into)?);
        let mut encoder = Encoder::new(file, DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .map_err(Into::<Box<_>>::into)?
            .write_image_data(&data)
            .map_err(Into::<Box<_>>::into)?;
        Ok(())
    }

    pub fn disable_double_buffer(&mut self) {
        self.render_mode = RenderMode::Immediate;
    }

    /// Erases the display by filling it with the background color.
    pub fn erase(&mut self) -> Result<(), piet::Error> {
        let entire_screen = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        let fg_color = self.foreground_color;
        self.foreground_color = self.program_options.default_bg_color();
        self.draw(entire_screen, false)?;
        self.foreground_color = fg_color;
        Ok(())
    }

    /// Draws or strokes a shape on the display, in the foreground color.
    pub fn draw(
        &mut self,
        mut shape: impl Shape + Strokable,
        stroke: bool,
    ) -> Result<(), piet::Error> {
        if stroke {
            shape = Strokable::stroking(shape);
        }
        let fg = self.foreground_color;
        {
            let mut rc = self.render_context();
            if stroke {
                rc.stroke(&shape, &fg, 1.0);
            } else {
                rc.fill(&shape, &fg);
            }
            rc.finish()?;
        }
        self.render(false)?;
        Ok(())
    }

    /// Calculates the shape of the area behind a text layout, so that it can be drawn on top of a background color.
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
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

pub trait ColorExt {
    /// Creates a `Color` from a 32-bit RGB value.
    ///
    /// ```
    /// # use piet::Color;
    /// assert_eq!(Color::from_rgb_u32(0x00FF00), Color::rgb8(0x00, 0xFF, 0x00));
    /// ```
    ///
    /// # Arguments
    ///
    /// * `rgb` - The 32-bit RGB value representing the color.
    ///
    /// # Returns
    ///
    /// A `Color` instance representing the specified RGB value.
    fn from_rgb_u32(rgb: u32) -> Color;
}

impl ColorExt for Color {
    fn from_rgb_u32(rgb: u32) -> Color {
        Color::rgb8(
            ((rgb >> 16) & 0xFF) as u8,
            ((rgb >> 8) & 0xFF) as u8,
            (rgb & 0xFF) as u8,
        )
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

pub trait Strokable {
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderMode {
    #[default]
    Immediate,
    DoubleBuffered,
}
