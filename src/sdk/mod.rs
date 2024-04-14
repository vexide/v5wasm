use std::{ffi::CStr, time::Instant};

use anyhow::Context;
use piet::{
    kurbo::{Circle, Rect, Shape},
    Color, FontFamily, ImageFormat, IntoBrush, RenderContext, Text, TextAttribute, TextLayout,
    TextLayoutBuilder,
};
use piet_common::{
    kurbo::{Point, Size},
    Piet,
};
use piet_common::{BitmapTarget, Device};
use png::{ColorType, Encoder};
use wasmtime::{AsContext, Caller, Memory};

use self::display::{ColorExt, Display, DISPLAY_HEIGHT, DISPLAY_WIDTH};

mod display;

pub struct SdkState<'a> {
    program_start: Instant,
    display: Display<'a>,
}

impl<'a> SdkState<'a> {
    pub fn new(renderer: &'a mut Device) -> Self {
        SdkState {
            program_start: Instant::now(),
            display: Display::new(DISPLAY_WIDTH, DISPLAY_HEIGHT, renderer).unwrap(),
        }
    }
}

const JUMP_TABLE_START: usize = 0x037FC000;

macro_rules! map_jump_table {
    {
        state = $State:ty;
        memory as $memory:ident;
        $Sdk:ident {
            $(
                $offset:expr =>
                fn $name:ident($($args:tt)*) $(-> $ret:ty)? $block:block
            ),+ $(,)?
        }
    } => {
        pub struct $Sdk {
            api: Vec<(usize, ::wasmtime::Func)>,
        }

        impl $Sdk {
            pub fn new(store: &mut ::wasmtime::Store<$State>, memory: ::wasmtime::Memory) -> Self {
                let $memory = memory;
                Self {
                    api: vec![
                        $(
                            (
                                JUMP_TABLE_START + $offset,
                                ::wasmtime::Func::wrap(
                                    &mut *store,
                                    move |$($args)*| $(-> ::wasmtime::Result<$ret>)? {
                                        $block
                                    }
                                )
                            )
                        ),+
                    ],
                }
            }

            pub fn expose_jump_table(
                self,
                store: &mut ::wasmtime::Store<$State>,
                table: &::wasmtime::Table,
                memory: &::wasmtime::Memory
            ) -> ::wasmtime::Result<()> {
                let sdk_base = table.size(&mut *store);
                let api_size = self.api.len() as u32;
                table.grow(&mut *store, api_size, ::wasmtime::Ref::Func(None))?;
                for (offset, (address, method)) in self.api.into_iter().enumerate() {
                    let sdk_index = sdk_base + (offset as u32);
                    table.set(&mut *store, sdk_index, ::wasmtime::Ref::Func(Some(method)))?;
                    memory.write(&mut *store, address, &sdk_index.to_le_bytes())?;
                }
                println!("Jump table exposed with {api_size} functions");
                Ok(())
            }
        }
    };
}

map_jump_table! {
    state = SdkState;
    memory as memory;
    Sdk {
        0x89c => fn vexSerialWriteBuffer(caller: Caller<'_, SdkState>, channel: i32, data: i32, data_len: i32) -> i32 {
            if channel == 1 {
                let data_bytes =
                    memory.data(&caller)[data as usize..(data + data_len) as usize].to_vec();
                let data_str = String::from_utf8(data_bytes).unwrap();
                print!("{}", data_str);
            }
            Ok(data_len)
        },
        0x05c => fn vexTasksRun() {},
        0x134 => fn vexSystemHighResTimeGet(caller: Caller<'_, SdkState>) -> u64 {
            Ok(caller.data().program_start.elapsed().as_micros() as u64)
        },
        0x640 => fn vexDisplayForegroundColor(mut caller: Caller<'_, SdkState>, col: u32) {
            caller.data_mut().display.foreground_color = Color::from_rgb_u32(col);
        },
        0x644 => fn vexDisplayBackgroundColor(mut caller: Caller<'_, SdkState>, col: u32) {
            caller.data_mut().display.background_color = Color::from_rgb_u32(col);
        },
        0x670 => fn vexDisplayRectFill(mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32)  {
            let rect = Rect::new(x1 as f64, y1 as f64, x2 as f64, y2 as f64);
            caller.data_mut().display.draw(rect, false).unwrap();
        },
        0x674 => fn vexDisplayCircleDraw(mut caller: Caller<'_, SdkState>, xc: i32, yc: i32, radius: i32) {
            println!("vexDisplayCircleDraw({}, {}, {})", xc, yc, radius);
            let circle = Circle::new((xc as f64, yc as f64), radius as f64);
            caller.data_mut().display.draw(circle, true).unwrap();
        },
        0x684 => fn vexDisplayVString(mut caller: Caller<'_, SdkState>, line_number: i32, format_ptr: u32, _args: u32) -> () {
            println!("vexDisplayVString({}, {:x}, {:x})", line_number, format_ptr, _args);
            let format = memory.read_c_string(&caller, format_ptr as usize).context("Failed to read C-string")?.to_string();
            caller.data_mut().display.write_text(format, line_number).unwrap();
            Ok(())
        },
        0x8ac => fn vexSerialWriteFree(_channel: u32) -> i32 {
            Ok(2048)
        },
        0x130 => fn vexSystemExitRequest() {
            std::process::exit(0);
        },
    }
}

trait MemoryExt {
    fn read_c_string<'a>(&self, store: &'a impl AsContext, offset: usize) -> Option<&'a str>;
}

impl MemoryExt for Memory {
    fn read_c_string<'a>(&self, store: &'a impl AsContext, offset: usize) -> Option<&'a str> {
        let bytes = &self.data(store)[offset..];
        let c_str = CStr::from_bytes_until_nul(bytes).ok()?;
        c_str.to_str().ok()
    }
}
