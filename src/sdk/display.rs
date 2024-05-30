use std::{
    num::NonZeroU16,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::Context;
use base64::prelude::*;
use mint::Point2;
use rgb::RGB8;
use vexide_simulator_protocol::{
    Command, DrawCommand, Event, Shape, TextLocation, TextMetrics, V5FontFamily, V5FontSize, V5Text,
};
use wasmtime::*;

use crate::{protocol::Protocol, ProgramOptions};

use super::{clone_c_string, JumpTableBuilder, MemoryExt, SdkState};

// MARK: Jump Table

pub fn build_display_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    // vexDisplayForegroundColor
    builder.insert(0x640, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.foreground_color = RGB8 {
            r: (col >> 16) as u8,
            g: (col >> 8) as u8,
            b: col as u8,
        };
    });

    // vexDisplayBackgroundColor
    builder.insert(0x644, move |mut caller: Caller<'_, SdkState>, col: u32| {
        caller.data_mut().display.background_color = RGB8 {
            r: (col >> 16) as u8,
            g: (col >> 8) as u8,
            b: col as u8,
        };
    });

    // vexDisplayRectDraw
    builder.insert(
        0x668,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).draw(
                Shape::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayRectFill
    builder.insert(
        0x670,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).draw(
                Shape::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayCircleDraw
    builder.insert(
        0x674,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).draw(
                Shape::Circle {
                    center: [cx, cy].into(),
                    radius: radius as u16,
                },
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayCircleFill
    builder.insert(
        0x67c,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).draw(
                Shape::Circle {
                    center: [cx, cy].into(),
                    radius: radius as u16,
                },
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayStringWidthGet
    builder.insert(
        0x6c0,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;
            let sdk = caller.data_mut();

            let font_size = sdk.display.last_font_size;
            let size = sdk
                .display
                .ctx(&mut sdk.protocol)
                .get_text_metrics(V5Text {
                    data: string,
                    font_family: V5FontFamily::UserMono,
                    font_size,
                })?;
            Ok(size.width as u32)
        },
    );

    // vexDisplayStringHeightGet
    builder.insert(
        0x6c4,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;
            let sdk = caller.data_mut();

            let font_size = sdk.display.last_font_size;
            let size = sdk
                .display
                .ctx(&mut sdk.protocol)
                .get_text_metrics(V5Text {
                    data: string,
                    font_family: V5FontFamily::UserMono,
                    font_size,
                })?;
            Ok(size.height as u32)
        },
    );

    // vexDisplayRender
    builder.insert(
        0x7a0,
        move |mut caller: Caller<'_, SdkState>, vsync_wait: i32, run_scheduler: i32| {
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).render()?;
            let vsync_finish = Instant::now() + Duration::from_secs_f64(1.0 / 60.0);
            if vsync_wait != 0 {
                while Instant::now() < vsync_finish {
                    sleep(Duration::from_millis(1));
                    if run_scheduler != 0 {
                        sdk.recv_all_commands()?;
                    }
                }
            }
            Ok(())
        },
    );

    // vexDisplayDoubleBufferDisable
    builder.insert(0x7a4, move |mut caller: Caller<'_, SdkState>| {
        let sdk = caller.data_mut();
        sdk.display
            .ctx(&mut sdk.protocol)
            .set_double_buffered(false)?;
        Ok(())
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
            let sdk = caller.data_mut();

            sdk.display.ctx(&mut sdk.protocol).write(
                V5Text {
                    data: format,
                    font_family: Default::default(),
                    font_size: Default::default(),
                },
                TextLocation::Coordinates {
                    point: [x_pos, y_pos].into(),
                },
                opaque == 0,
            )?;
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
            let sdk = caller.data_mut();

            sdk.display.ctx(&mut sdk.protocol).write(
                V5Text {
                    data: format,
                    font_family: Default::default(),
                    font_size: Default::default(),
                },
                TextLocation::Line { line: line_number },
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayVStringAt
    builder.insert(
        0x688,
        move |mut caller: Caller<'_, SdkState>,
              x_pos: i32,
              y_pos: i32,
              format_ptr: u32,
              _args: u32|
              -> Result<()> {
            let format = clone_c_string!(format_ptr as usize, from caller using memory)?;
            let sdk = caller.data_mut();

            sdk.display.ctx(&mut sdk.protocol).write(
                V5Text {
                    data: format,
                    font_family: Default::default(),
                    font_size: Default::default(),
                },
                TextLocation::Coordinates {
                    point: [x_pos, y_pos].into(),
                },
                true,
            )?;
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
            let sdk = caller.data_mut();

            sdk.display.ctx(&mut sdk.protocol).write(
                V5Text {
                    data: format,
                    font_family: Default::default(),
                    font_size: V5FontSize::Small,
                },
                TextLocation::Coordinates {
                    point: [x_pos, y_pos].into(),
                },
                true,
            )?;
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
            let sdk = caller.data_mut();
            sdk.display.ctx(&mut sdk.protocol).draw_buffer(
                &buffer,
                [x1, y1],
                [x2, y2],
                NonZeroU16::new(stride as u16)
                    .with_context(|| format!("Unexpected stride value {stride:?}"))?,
            )?;
            Ok(())
        },
    );
}

// MARK: Display

pub const DISPLAY_HEIGHT: i32 = 272;
pub const DISPLAY_WIDTH: i32 = 480;
pub const HEADER_HEIGHT: i32 = 32;

pub const BLACK: RGB8 = RGB8::new(0, 0, 0);
pub const WHITE: RGB8 = RGB8::new(255, 255, 255);
pub const HEADER_BG: RGB8 = RGB8::new(0x00, 0x99, 0xCC);

pub struct DisplayCtx<'a> {
    display: &'a mut Display,
    protocol: &'a mut Protocol,
}

impl<'a> DisplayCtx<'a> {
    /// Copies a buffer of pixels to the display.
    fn draw_buffer(
        &mut self,
        buf: &[u8],
        top_left: impl Into<Point2<i32>>,
        bot_right: impl Into<Point2<i32>>,
        stride: NonZeroU16,
    ) -> anyhow::Result<()> {
        let buffer = BASE64_STANDARD.encode(buf);
        self.protocol.send(&Event::ScreenDraw {
            command: DrawCommand::CopyBuffer {
                top_left: top_left.into(),
                bottom_right: bot_right.into(),
                stride,
                buffer,
            },
            color: self.display.foreground_color,
            background: self.display.background_color,
        })?;

        Ok(())
    }

    /// Draws or strokes a shape on the display, using the current foreground color.
    pub fn draw(&mut self, shape: Shape, stroke: bool) -> anyhow::Result<()> {
        self.protocol.send(&Event::ScreenDraw {
            command: if stroke {
                DrawCommand::Stroke { shape }
            } else {
                DrawCommand::Fill { shape }
            },
            color: self.display.foreground_color,
            background: self.display.background_color,
        })?;
        Ok(())
    }

    pub fn write(
        &mut self,
        text: V5Text,
        location: TextLocation,
        opaque: bool,
    ) -> anyhow::Result<()> {
        self.display.last_font_size = text.font_size;
        self.protocol.send(&Event::ScreenDraw {
            command: DrawCommand::Write {
                text,
                location,
                opaque,
            },
            color: self.display.foreground_color,
            background: self.display.background_color,
        })?;
        Ok(())
    }

    pub fn with_colors<R>(&mut self, fg: RGB8, bg: RGB8, func: impl FnOnce(&mut Self) -> R) -> R {
        let old_fg = self.display.foreground_color;
        let old_bg = self.display.background_color;
        self.display.foreground_color = fg;
        self.display.background_color = bg;
        let result = func(self);
        self.display.foreground_color = old_fg;
        self.display.background_color = old_bg;
        result
    }

    /// Draws the blue program header at the top of the display.
    fn draw_header(&mut self) -> anyhow::Result<()> {
        self.with_colors(HEADER_BG, RGB8::default(), |ctx| {
            ctx.draw(
                Shape::Rectangle {
                    top_left: [0, 0].into(),
                    bottom_right: [DISPLAY_WIDTH, HEADER_HEIGHT].into(),
                },
                false,
            )
        })?;

        let elapsed = self.display.start_instant.elapsed().as_secs();
        let secs = elapsed % 60;
        let mins = elapsed / 60;
        let time = format!("{:01}:{:02}", mins, secs);
        self.write(
            V5Text {
                data: time,
                font_family: V5FontFamily::TimerMono,
                font_size: V5FontSize::Large,
            },
            TextLocation::Coordinates {
                point: [DISPLAY_WIDTH / 2, 3].into(),
            },
            true,
        )?;
        Ok(())
    }

    /// Sends the display to the render thread.
    pub fn set_double_buffered(&mut self, enable: bool) -> anyhow::Result<()> {
        if self.display.double_buffered == enable {
            return Ok(());
        }
        self.display.double_buffered = enable;
        self.protocol
            .send(&Event::ScreenDoubleBufferMode { enable })?;
        Ok(())
    }

    /// Erases the display by filling it with the default background color.
    pub fn erase(&mut self) -> anyhow::Result<()> {
        self.with_colors(
            self.display.program_options.default_fg_color(),
            BLACK,
            |ctx| {
                ctx.draw(
                    Shape::Rectangle {
                        top_left: [0, 0].into(),
                        bottom_right: [DISPLAY_WIDTH, DISPLAY_HEIGHT].into(),
                    },
                    false,
                )
            },
        )?;
        Ok(())
    }

    /// Fetches how big a string will be when rendered.
    ///
    /// Caches the result so that the same text and options don't have to be calculated multiple times in a row.
    pub fn get_text_metrics(&mut self, text: V5Text) -> anyhow::Result<TextMetrics> {
        if let Some((cached_text, metrics)) = &self.display.text_metrics_cache {
            if cached_text == &text {
                return Ok(*metrics);
            }
        }
        self.protocol
            .send(&Event::TextMetricsRequest { text: text.clone() })?;

        let cmd = self.protocol.wait_for_command(
            |c| matches!(c, Command::SetTextMetrics { text: recv_text, .. } if *recv_text == text),
        )?;
        let metrics = match cmd {
            Command::SetTextMetrics { metrics, .. } => metrics,
            _ => unreachable!(),
        };
        self.display.text_metrics_cache = Some((text, metrics));
        Ok(metrics)
    }

    pub fn set_metrics_cache(&mut self, text: V5Text, metrics: TextMetrics) {
        self.display.text_metrics_cache = Some((text, metrics));
    }

    pub fn render(&mut self) -> anyhow::Result<()> {
        self.set_double_buffered(true)?;
        self.protocol.send(&Event::ScreenRender)?;
        Ok(())
    }
}

pub struct Display {
    /// The display's saved foreground color.
    pub foreground_color: RGB8,
    /// The display's saved background color.
    pub background_color: RGB8,
    start_instant: Instant,
    program_options: ProgramOptions,
    /// Cache for text layout calculations, to avoid re-calculating the same text layout multiple times in a row.
    text_metrics_cache: Option<(V5Text, TextMetrics)>,
    last_font_size: V5FontSize,
    double_buffered: bool,
}

impl Display {
    pub fn new(program_options: ProgramOptions, start_instant: Instant) -> Self {
        Self {
            foreground_color: program_options.default_fg_color(),
            background_color: program_options.default_bg_color(),
            program_options,
            text_metrics_cache: None,
            start_instant,
            last_font_size: V5FontSize::Normal,
            double_buffered: false,
        }
    }

    pub fn ctx<'a>(&'a mut self, protocol: &'a mut Protocol) -> DisplayCtx<'a> {
        DisplayCtx {
            display: self,
            protocol,
        }
    }
}
