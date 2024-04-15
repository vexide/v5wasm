use std::{
    fs::File,
    io::BufWriter,
    ops::{Deref, DerefMut},
};

use anyhow::Context;
use piet::{
    kurbo::{Circle, Rect, Shape},
    Color, ImageFormat, RenderContext, Text, TextLayout, TextLayoutBuilder,
};
use piet_common::PietTextLayout;
use piet_common::{BitmapTarget, Device};
use png::{ColorType, Encoder};
use wasmtime::*;

use super::{read_c_string, JumpTableBuilder, MemoryExt, SdkState};

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

    // vexDisplayVString
    builder.insert(
        0x684,
        move |mut caller: Caller<'_, SdkState>,
              line_number: i32,
              format_ptr: u32,
              _args: u32|
              -> Result<()> {
            let format = read_c_string!(format_ptr as usize, from caller using memory)?;
            caller
                .data_mut()
                .display
                .write_text(format, line_number, true)
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
    pub fn new(width: usize, height: usize, renderer: &'a mut Device) -> Result<Self, piet::Error> {
        let mut bitmap = renderer.bitmap_target(width, height, 1.0)?;
        let mut display = Display {
            foreground_color: Color::WHITE,
            background_color: Color::BLACK,
            mono_font: {
                let mut rc = bitmap.render_context();
                let noto_sans_mono = include_bytes!("../../fonts/NotoSansMono-Regular.ttf");
                rc.text().load_font(noto_sans_mono)?
            },
            bitmap,
            width,
            height,
        };
        display.erase()?;
        Ok(display)
    }

    fn draw_header(&mut self) -> Result<(), piet::Error> {
        const HEADER_BG: Color = Color::rgb8(0x01, 0x99, 0xCC);
        let width = self.width as f64;
        let mut rc = self.render_context();
        rc.fill(Rect::new(0.0, 0.0, width, HEADER_HEIGHT as f64), &HEADER_BG);
        rc.finish()?;
        Ok(())
    }

    pub fn render(&mut self) -> Result<(), piet::Error> {
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

    pub fn erase(&mut self) -> Result<(), piet::Error> {
        let entire_screen = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        let fg_color = self.foreground_color;
        self.foreground_color = Color::BLACK;
        self.draw(entire_screen, false)?;
        self.foreground_color = fg_color;
        Ok(())
    }

    pub fn draw(&mut self, shape: impl Shape, stroke: bool) -> Result<(), piet::Error> {
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
        self.render()?;
        Ok(())
    }

    fn calculate_text_background(text_layout: &PietTextLayout, y_coord: f64) -> Rect {
        const LINE_HEIGHT_OFFSET: f64 = -2.0;
        let size = text_layout.size();
        Rect::new(
            0.0,
            y_coord - LINE_HEIGHT_OFFSET,
            size.width,
            y_coord + size.height + LINE_HEIGHT_OFFSET * 2.0,
        )
    }

    pub fn write_text(
        &mut self,
        text: String,
        line_number: i32,
        opaque: bool,
    ) -> Result<(), piet::Error> {
        let text = text.replace('\n', ".");
        let fg = self.foreground_color;
        let bg = self.background_color;
        let font = self.mono_font.clone();
        {
            let mut rc = self.render_context();
            let text_layout = rc
                .text()
                .new_text_layout(text)
                .text_color(fg)
                .font(font, 16.0)
                .build()?;

            let y_coord = line_number as f64 * 20.0 + HEADER_HEIGHT as f64;
            if opaque {
                rc.fill(
                    Display::calculate_text_background(&text_layout, y_coord),
                    &bg,
                );
            }

            rc.draw_text(&text_layout, (0.0, y_coord - 2.0));
            rc.finish()?;
        }
        self.render()?;
        Ok(())
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
