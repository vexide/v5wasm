use std::{
    io::Cursor,
    mem::size_of,
    num::NonZeroU16,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use base64::prelude::*;
use bytemuck::{NoUninit, Pod, Zeroable};
use bytes::{Buf, Bytes, BytesMut};
use embedded_graphics_core::{
    geometry::Dimensions,
    pixelcolor::{
        raw::{RawData, RawU24},
        Rgb888,
    },
};
use mint::Point2;
use rgb::RGB8;
use tinybmp::Bmp;
use vexide_simulator_protocol::{
    Command, DrawCommand, Event, LogLevel, Rect, ScrollLocation, Shape, TextLocation, TextMetrics,
    V5FontFamily, V5FontSize, V5Text,
};
use wasmtime::*;

use crate::{
    protocol::{warn_bt, Log, Protocol},
    ProgramOptions,
};

use super::{clone_c_string, JumpTableBuilder, MemoryExt, SdkState};

// MARK: Jump Table

#[repr(C, packed)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Pod, Zeroable)]
#[allow(non_camel_case_types)]
struct V5Image {
    pub width: u16,
    pub height: u16,
    pub data: u32,
    pub p: u32,
}

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

    // vexDisplayErase
    builder.insert(0x648, move |mut caller: Caller<'_, SdkState>| {
        caller.data_mut().display_ctx().erase()?;
        Ok(())
    });

    // vexDisplayScroll
    builder.insert(
        0x64c,
        move |mut caller: Caller<'_, SdkState>, n_start_line: i32, n_lines: i32| {
            caller
                .data_mut()
                .display_ctx()
                .scroll(ScrollLocation::Line { line: n_start_line }, n_lines)?;
            Ok(())
        },
    );

    // vexDisplayScrollRect
    builder.insert(
        0x650,
        move |mut caller: Caller<'_, SdkState>,
              x1: i32,
              y1: i32,
              x2: i32,
              y2: i32,
              n_lines: i32| {
            caller.data_mut().display_ctx().scroll(
                ScrollLocation::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                n_lines,
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

            caller.data_mut().display_ctx().draw_buffer(
                &buffer,
                [x1, y1],
                [x2, y2],
                NonZeroU16::new(stride as u16)
                    .with_context(|| format!("Unexpected stride value {stride:?}"))?,
            )?;
            Ok(())
        },
    );

    // vexDisplayPixelSet
    builder.insert(
        0x658,
        move |mut caller: Caller<'_, SdkState>, x: i32, y: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Rectangle {
                    top_left: [x, y].into(),
                    bottom_right: [x, y].into(),
                },
                false,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayPixelClear
    builder.insert(
        0x65c,
        move |mut caller: Caller<'_, SdkState>, x: i32, y: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Rectangle {
                    top_left: [x, y].into(),
                    bottom_right: [x, y].into(),
                },
                false,
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayLineDraw
    builder.insert(
        0x660,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Line {
                    start: [x1, y1].into(),
                    end: [x2, y2].into(),
                },
                true,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayLineClear
    builder.insert(
        0x664,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Line {
                    start: [x1, y1].into(),
                    end: [x2, y2].into(),
                },
                true,
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayRectDraw
    builder.insert(
        0x668,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                true,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayRectClear
    builder.insert(
        0x66c,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                true,
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayRectFill
    builder.insert(
        0x670,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Rectangle {
                    top_left: [x1, y1].into(),
                    bottom_right: [x2, y2].into(),
                },
                false,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayCircleDraw
    builder.insert(
        0x674,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Circle {
                    center: [cx, cy].into(),
                    radius: radius as u16,
                },
                true,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayCircleClear
    builder.insert(
        0x678,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Circle {
                    center: [cx, cy].into(),
                    radius: radius as u16,
                },
                true,
                true,
            )?;
            Ok(())
        },
    );

    // vexDisplayCircleFill
    builder.insert(
        0x67c,
        move |mut caller: Caller<'_, SdkState>, cx: i32, cy: i32, radius: i32| {
            caller.data_mut().display_ctx().draw(
                Shape::Circle {
                    center: [cx, cy].into(),
                    radius: radius as u16,
                },
                false,
                false,
            )?;
            Ok(())
        },
    );

    // vexDisplayTextSize
    builder.insert(0x6a8, move |_u: u32, _d: u32| -> Result<()> {
        bail!("vexDisplayTextSize is not implemented");
    });

    // vexDisplayFontNamedSet
    builder.insert(0x6b4, move |_name: u32| -> Result<()> {
        bail!("vexDisplayFontNamedSet is not implemented");
    });

    // vexDisplayForegroundColorGet
    builder.insert(0x6b8, move |caller: Caller<'_, SdkState>| -> u32 {
        let color = caller.data().display.foreground_color;
        (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
    });

    // vexDisplayBackgroundColorGet
    builder.insert(0x6bc, move |caller: Caller<'_, SdkState>| -> u32 {
        let color = caller.data().display.background_color;
        (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
    });

    // vexDisplayClipRegionSet
    builder.insert(
        0x794,
        move |mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32| {
            caller.data_mut().display.set_clip_region(x1, y1, x2, y2);
            Ok(())
        },
    );

    // vexDisplayRender
    builder.insert(
        0x7a0,
        move |mut caller: Caller<'_, SdkState>, vsync_wait: i32, run_scheduler: i32| {
            caller.data_mut().display_ctx().render()?;
            let vsync_finish = Instant::now() + Duration::from_secs_f64(1.0 / 60.0);
            if vsync_wait != 0 {
                let sdk = caller.data_mut();
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
        caller.data_mut().display_ctx().set_double_buffered(false)?;
        Ok(())
    });

    // vexDisplayClipRegionSetWithIndex
    builder.insert(
        0x7a8,
        move |mut caller: Caller<'_, SdkState>, index: i32, x1: i32, y1: i32, x2: i32, y2: i32| {
            if index != 0 {
                warn_bt!(caller, "vexDisplayClipRegionSetWithIndex: the only supported index is 0, but got {index:?} instead")?;
                return Ok(());
            }

            caller.data_mut().display.set_clip_region(x1, y1, x2, y2);

            Ok(())
        },
    );

    // vexImageBmpRead
    builder.insert(
        0x990,
        move |mut caller: Caller<'_, SdkState>,
              i_buf: u32,
              o_buf: u32,
              maxw: u32,
              maxh: u32|
              -> Result<u32> {
            if i_buf == 0 {
                warn_bt!(caller, "vexImageBmpRead: ibuf must not be null")?;
                return Ok(0);
            }
            if o_buf == 0 {
                warn_bt!(caller, "vexImageBmpRead: oBuf must not be null")?;
                return Ok(0);
            }

            let mut img = {
                let o_buf_mem =
                    &mut memory.data_mut(&mut caller)[o_buf as usize..][..size_of::<V5Image>()];
                *bytemuck::from_bytes_mut::<V5Image>(o_buf_mem)
            };

            if img.data == 0 {
                warn_bt!(caller, "vexImageBmpRead: oBuf data field must not be null")?;
                return Ok(0);
            }

            let bmp = {
                let i_buf_mem = &memory.data(&mut caller)[i_buf as usize..];
                match Bmp::<Rgb888>::from_slice(i_buf_mem) {
                    Ok(bmp) => bmp.to_owned(),
                    Err(err) => {
                        warn_bt!(caller, "vexImageBmpRead: failed to read BMP: {err:?}")?;
                        return Ok(0);
                    }
                }
            };

            let size = bmp.bounding_box().size;

            if size.width > maxw {
                warn_bt!(caller, "vexImageBmpRead: image has {:?}px width but the specified max width was {maxw:?}", size.width)?;
                return Ok(0);
            }

            if size.height > maxh {
                warn_bt!(caller, "vexImageBmpRead: image has {:?}px height but the specified max height was {maxh:?}", size.height)?;
                return Ok(0);
            }

            let mut bytes = Bytes::from_iter(
                bmp.pixels()
                    .flat_map(|p| RawU24::from(p.1).into_inner().to_le_bytes()),
            );

            let bytes_len = bytes.len();
            let max_len = (maxw * maxh * 4) as usize;
            if bytes_len > max_len {
                warn_bt!(caller, "vexImageBmpRead: image has {bytes_len:?} bytes but the output buffer only has space for {max_len:?} bytes")?;
                return Ok(0);
            }

            let data_ptr = u32::from_le(img.data);
            let out_data = &mut memory.data_mut(&mut caller)[(data_ptr as usize)..][..bytes.len()];
            bytes.copy_to_slice(out_data);

            img.width = (size.width as u16).to_le();
            img.height = (size.height as u16).to_le();
            img.p = (data_ptr + (size.width * 4)).to_le();

            memory.data_mut(&mut caller)[o_buf as usize..][..size_of::<V5Image>()]
                .copy_from_slice(bytemuck::bytes_of(&img));
            Ok(1)
        },
    );

    // vexDisplayStringWidthGet
    builder.insert(
        0x6c0,
        move |mut caller: Caller<'_, SdkState>, string_ptr: i32| {
            let string = clone_c_string!(string_ptr as usize, from caller using memory)?;

            let sdk = caller.data_mut();
            let font_size = sdk.display.last_font_size;
            let size = sdk.display_ctx().get_text_metrics(V5Text {
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
            let size = sdk.display_ctx().get_text_metrics(V5Text {
                data: string,
                font_family: V5FontFamily::UserMono,
                font_size,
            })?;
            Ok(size.height as u32)
        },
    );

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

            caller.data_mut().display_ctx().write(
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

            caller.data_mut().display_ctx().write(
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

            caller.data_mut().display_ctx().write(
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

            caller.data_mut().display_ctx().write(
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
            color: self.display.foreground_color.into(),
            clip_region: self.display.clip_region,
        })?;

        Ok(())
    }

    /// Draws or strokes a shape on the display, using the current foreground color.
    pub fn draw(&mut self, shape: Shape, stroke: bool, erase: bool) -> anyhow::Result<()> {
        self.protocol.send(&Event::ScreenDraw {
            command: if stroke {
                DrawCommand::Stroke { shape }
            } else {
                DrawCommand::Fill { shape }
            },
            color: if erase {
                self.display.background_color
            } else {
                self.display.foreground_color
            }
            .into(),
            clip_region: self.display.clip_region,
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
                background: self.display.background_color.into(),
            },
            color: self.display.foreground_color.into(),
            clip_region: self.display.clip_region,
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

    /// Erases the display by filling it with the current background color.
    pub fn erase(&mut self) -> anyhow::Result<()> {
        self.draw(
            Shape::Rectangle {
                top_left: [0, 0].into(),
                bottom_right: [DISPLAY_WIDTH, DISPLAY_HEIGHT].into(),
            },
            false,
            true,
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

    pub fn render(&mut self) -> anyhow::Result<()> {
        self.set_double_buffered(true)?;
        self.protocol.send(&Event::ScreenRender)?;
        Ok(())
    }

    pub fn scroll(&mut self, bounds: ScrollLocation, lines: i32) -> anyhow::Result<()> {
        self.protocol.send(&Event::ScreenScroll {
            location: bounds,
            lines,
            background: self.display.background_color.into(),
            clip_region: self.display.clip_region,
        })?;
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
    clip_region: Rect,
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
            clip_region: Rect {
                top_left: Point2 {
                    x: 0,
                    y: HEADER_HEIGHT,
                },
                bottom_right: Point2 {
                    x: DISPLAY_WIDTH,
                    y: DISPLAY_HEIGHT,
                },
            },
        }
    }

    pub fn ctx<'a>(&'a mut self, protocol: &'a mut Protocol) -> DisplayCtx<'a> {
        DisplayCtx {
            display: self,
            protocol,
        }
    }

    pub fn set_metrics_cache(&mut self, text: V5Text, metrics: TextMetrics) {
        self.text_metrics_cache = Some((text, metrics));
    }

    pub fn set_clip_region(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        self.clip_region = Rect {
            top_left: [
                x1.clamp(0, DISPLAY_WIDTH),
                y1.clamp(HEADER_HEIGHT, DISPLAY_HEIGHT),
            ]
            .into(),
            bottom_right: [
                x2.clamp(0, DISPLAY_WIDTH),
                y2.clamp(HEADER_HEIGHT, DISPLAY_HEIGHT),
            ]
            .into(),
        };
    }
}
