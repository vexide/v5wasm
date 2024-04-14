//! Example of instantiating two modules which link to each other.

// You can execute this example with `cargo run --example linking`

use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use piet::ImageFormat;
use piet::{kurbo::Rect, Color, RenderContext};
use piet_common::kurbo::{Point, Size};
use piet_common::{BitmapTarget, Device};
use png::{ColorType, Encoder};
use wasmtime::*;

const DISPLAY_HEIGHT: usize = 272;
const DISPLAY_WIDTH: usize = 480;

struct SdkState<'a> {
    program_start: Instant,
    foreground_color: Color,
    display: BitmapTarget<'a>,
}

impl<'a> SdkState<'a> {
    fn save_display(&mut self) -> std::result::Result<(), piet::Error> {
        {
            let mut rc = self.display.render_context();
            rc.fill(
                Rect::new(0.0, 0.0, DISPLAY_WIDTH as f64, 0x20 as f64),
                &Color::AQUA,
            );
            rc.finish().unwrap();
        }
        let mut data = vec![0; DISPLAY_HEIGHT * DISPLAY_WIDTH * 4];
        self.display
            .copy_raw_pixels(ImageFormat::RgbaPremul, &mut data)?;
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
            println!("vexDisplayForegroundColor({:x})", col);
            caller.data_mut().foreground_color = Color::rgb8(
                (col >> 16) as u8,
                (col >> 8) as u8,
                col as u8,
            );
        },
        0x670 => fn vexDisplayRectFill(mut caller: Caller<'_, SdkState>, x1: i32, y1: i32, x2: i32, y2: i32) -> () {
            println!("vexDisplayRectFill({}, {}, {}, {})", x1, y1, x2, y2);
            let rect = Rect::new(x1 as f64, y1 as f64, x2 as f64, y2 as f64);
            let fg_color = caller.data().foreground_color;
            {
                let mut rc = caller.data_mut().display.render_context();
                rc.fill(rect, &fg_color);
                rc.finish().unwrap();
            }
            caller.data_mut().save_display().unwrap();
            Ok(())
        },
        0x674 => fn vexDisplayCircleDraw(xc: i32, yc: i32, radius: i32) {
            println!("TODO: vexDisplayCircleDraw({}, {}, {})", xc, yc, radius);
        },
        0x684 => fn vexDisplayVString(line_number: i32, format_ptr: u32, args: u32) {
            println!("TODO: vexDisplayVString({}, {:x}, {:x})", line_number, format_ptr, args);
        },
        0x8ac => fn vexSerialWriteFree(_channel: u32) -> i32 {
            Ok(2048)
        },
        0x130 => fn vexSystemExitRequest() {
            std::process::exit(0);
        },
    }
}

fn main() -> Result<()> {
    println!("Compiling...");
    let engine = Engine::new(
        Config::new()
            .debug_info(true)
            .wasm_backtrace_details(WasmBacktraceDetails::Enable),
    )?;
    let module = Module::from_file(
        &engine,
        "../vexide-wasm/target/wasm32-unknown-unknown/debug/examples/basic.wasm",
    )?;
    println!("Booting...");
    let mut renderer = Device::new().unwrap();
    let mut display = renderer
        .bitmap_target(DISPLAY_WIDTH, DISPLAY_HEIGHT, 1.0)
        .unwrap();
    {
        let mut rc = display.render_context();
        rc.fill(
            Rect::new(0.0, 0.0, DISPLAY_WIDTH as f64, DISPLAY_HEIGHT as f64),
            &Color::BLACK,
        );
        rc.fill(
            Rect::new(0.0, 0.0, DISPLAY_WIDTH as f64, 0x20 as f64),
            &Color::AQUA,
        );
        rc.finish().unwrap();
    }
    let mut state = SdkState {
        program_start: Instant::now(),
        foreground_color: Color::WHITE,
        display,
    };
    state.save_display().unwrap();
    let mut store = Store::new(&engine, state);
    let imports = module
        .imports()
        .filter_map(|i| match i.ty() {
            ExternType::Table(table_ty) => Some(table_ty),
            _ => None,
        })
        .next()
        .unwrap();

    // First set up our linker which is going to be linking modules together. We
    // want our linker to have wasi available, so we set that up here as well.
    let mut linker = Linker::new(&engine);
    let table = Table::new(&mut store, imports, Ref::Func(None))?;
    linker.define(&store, "env", "__indirect_function_table", table)?;
    linker.func_wrap(
        "env",
        "sim_log_backtrace",
        |caller: Caller<'_, SdkState>| {
            let backtrace = WasmBacktrace::capture(caller);
            println!("backtrace: {:?}", backtrace);
            Ok(())
        },
    )?;

    // Load and compile our module

    let instance = linker.instantiate(&mut store, &module)?;

    let memory = instance.get_memory(&mut store, "memory").unwrap();

    let target_pages = 0x700;
    let memory_size = memory.size(&store);
    memory.grow(&mut store, target_pages - memory_size)?;

    let sdk = Sdk::new(&mut store, memory);
    sdk.expose_jump_table(&mut store, &table, &memory)?;

    println!("_entry()");
    let run = instance.get_typed_func::<(), ()>(&mut store, "_entry")?;
    run.call(&mut store, ())?;
    Ok(())
}

const JUMP_TABLE_START: usize = 0x037FC000;

#[macro_export]
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
        struct $Sdk {
            api: Vec<(usize, ::wasmtime::Func)>,
        }

        impl $Sdk {
            fn new(store: &mut ::wasmtime::Store<$State>, memory: ::wasmtime::Memory) -> Self {
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

            fn expose_jump_table(
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
