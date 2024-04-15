//! Example of instantiating two modules which link to each other.

// You can execute this example with `cargo run --example linking`

use std::io::Cursor;

use anyhow::Context;
use bytes::{Buf, Bytes};
use piet::{Color, TextLayout};

use piet_common::Device;

use wasmparser::{Parser, Payload};
use wasmtime::*;

use crate::sdk::{JumpTable, SdkState};

mod sdk;

const HEADER_MAGIC: &[u8] = b"XVX5";

// #define V5_SIG_MAGIC            0x35585658  //XVX5
// #define IQ_SIG_MAGIC            0x32515658  //XVQ2
// #define EX_SIG_MAGIC            0x45585658  //XVXE
// #define V5_SIG_TYPE_USER        0
// #define V5_SIG_OWNER_SYS        0
// #define V5_SIG_OWNER_VEX        1
// #define V5_SIG_OWNER_PARTNER    2
// #define V5_SIG_OPTIONS_NONE     0
// #define V5_SIG_OPTIONS_INDG     (1 << 0)   // Invert default graphics colors
// #define V5_SIG_OPTIONS_EXIT     (1 << 1)   // Kill threads when main exits
// #define V5_SIG_OPTIONS_THDG     (1 << 2)   // Invert graphics based on theme

const PROGRAM_TYPE_USER: u32 = 0;
const PROGRAM_OWNER_SYS: u32 = 0;
const PROGRAM_OWNER_VEX: u32 = 1;
const PROGRAM_OWNER_PARTNER: u32 = 2;

#[derive(Debug, Clone, Copy)]
pub struct ProgramOptions {
    /// The program type. PROS sets this to 0.
    pub program_type: u32,
    /// The owner of the program. PROS sets this to 2.
    pub owner: u32,
    pub invert_default_graphics_colors: bool,
    pub kill_threads_when_main_exits: bool,
    pub invert_graphics_based_on_theme: bool,
}

impl ProgramOptions {
    pub const fn default_fg_color(&self) -> Color {
        if self.invert_default_graphics_colors {
            Color::BLACK
        } else {
            Color::WHITE
        }
    }

    pub const fn default_bg_color(&self) -> Color {
        if self.invert_default_graphics_colors {
            Color::WHITE
        } else {
            Color::BLACK
        }
    }
}

fn load_program(engine: &Engine, path: &str) -> Result<(Module, ProgramOptions)> {
    const PROGRAM_OPTIONS_INVERT_DEFAULT_GRAPHICS_COLORS: u32 = 1 << 0;
    const PROGRAM_OPTIONS_KILL_THREADS_WHEN_MAIN_EXITS: u32 = 1 << 1;
    const PROGRAM_OPTIONS_INVERT_GRAPHICS_BASED_ON_THEME: u32 = 1 << 2;

    let program = std::fs::read(path)?;

    let mut cold_header = None;
    let parser = Parser::new(0);
    for payload in parser.parse_all(&program) {
        if let Payload::CustomSection(custom_section) = payload? {
            if custom_section.name() == ".cold_magic" {
                cold_header = Some(Bytes::copy_from_slice(custom_section.data()));
                break;
            }
        }
    }

    let mut cold_header = cold_header
        .context("No cold header found in the program")
        .unwrap();
    let magic = cold_header.copy_to_bytes(HEADER_MAGIC.len());
    if magic != HEADER_MAGIC {
        return Err(anyhow::anyhow!("Invalid magic number"));
    }
    let program_type = cold_header.get_u32_le();
    let owner = cold_header.get_u32_le();
    let options = cold_header.get_u32_le();
    let cold_header = ProgramOptions {
        program_type,
        owner,
        invert_default_graphics_colors: options & PROGRAM_OPTIONS_INVERT_DEFAULT_GRAPHICS_COLORS
            != 0,
        kill_threads_when_main_exits: options & PROGRAM_OPTIONS_KILL_THREADS_WHEN_MAIN_EXITS != 0,
        invert_graphics_based_on_theme: options & PROGRAM_OPTIONS_INVERT_GRAPHICS_BASED_ON_THEME
            != 0,
    };

    let module = Module::from_binary(engine, &program)?;
    Ok((module, cold_header))
}

fn main() -> Result<()> {
    println!("Compiling...");
    let engine = Engine::new(
        Config::new()
            .debug_info(true)
            .wasm_backtrace_details(WasmBacktraceDetails::Enable),
    )?;
    let (module, cold_header) = load_program(
        &engine,
        "../vexide-wasm/target/wasm32-unknown-unknown/debug/examples/basic.wasm",
    )?;
    println!("Booting...");
    let mut renderer = Device::new().unwrap();

    let state = SdkState::new(module.clone(), cold_header, &mut renderer);

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

    let sdk = JumpTable::new(&mut store, memory);
    sdk.expose(&mut store, &table, &memory)?;

    println!("_entry()");
    let run = instance.get_typed_func::<(), ()>(&mut store, "_entry")?;
    run.call(&mut store, ())?;
    Ok(())
}
