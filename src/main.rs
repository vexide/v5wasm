use std::path::{Path, PathBuf};

use anyhow::Context;
use bytes::{Buf, Bytes};
use clap::Parser as _;
use fs_err as fs;

use protocol::Protocol;
use rgb::RGB8;
use sdk::display::{BLACK, WHITE};
use wasmparser::{Parser, Payload};
use wasmtime::*;

use crate::sdk::{JumpTable, SdkState};

mod protocol;
mod sdk;

const HEADER_MAGIC: &[u8] = b"XVX5";

/// Execute WebAssembly programs that rely on the VEX V5 SDK and jump table.
///
/// In order to be simulated, robot code should be WebAssembly-formatted (`.wasm`
/// file) and contain a V5 code signature in a custom section named `.cold_magic`.
/// Programs may utilize the VEX V5 jump table to interact with simulated subsystems.
///
/// A WASI environment is not provided.
#[derive(Debug, clap::Parser)]
#[command(version)]
struct Args {
    /// The path to the WebAssembly robot program that will be executed.
    program: PathBuf,
}

// const PROGRAM_TYPE_USER: u32 = 0;
// const PROGRAM_OWNER_SYS: u32 = 0;
// const PROGRAM_OWNER_VEX: u32 = 1;
// const PROGRAM_OWNER_PARTNER: u32 = 2;

/// Options parsed from the program's code signature/cold header.
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
    pub const fn default_fg_color(&self) -> RGB8 {
        if self.invert_default_graphics_colors {
            BLACK
        } else {
            WHITE
        }
    }

    pub const fn default_bg_color(&self) -> RGB8 {
        if self.invert_default_graphics_colors {
            WHITE
        } else {
            BLACK
        }
    }
}

/// Loads a user program from a file, parsing the cold header and creating a module.
fn load_program(engine: &Engine, path: &Path) -> Result<(Module, ProgramOptions)> {
    const PROGRAM_OPTIONS_INVERT_DEFAULT_GRAPHICS_COLORS: u32 = 1 << 0;
    const PROGRAM_OPTIONS_KILL_THREADS_WHEN_MAIN_EXITS: u32 = 1 << 1;
    const PROGRAM_OPTIONS_INVERT_GRAPHICS_BASED_ON_THEME: u32 = 1 << 2;

    let program = fs::read(path)?;

    // in vexide programs the cold header is stored in a section called ".cold_magic"
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

    // copy_to_bytes is used to remove the magic number from the start of the buffer
    let magic = cold_header.copy_to_bytes(HEADER_MAGIC.len());
    if magic != HEADER_MAGIC {
        return Err(anyhow::anyhow!("Invalid magic number"));
    }

    // Parse the rest of the options, these are all the ones found in the public SDK
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

    // this operation will do a lot of JIT compilation so it's probably the slowest part of the program
    let module = Module::from_binary(engine, &program)?;
    Ok((module, cold_header))
}

fn main() -> Result<()> {
    ctrlc::set_handler(move || {
        std::process::exit(0);
    })
    .unwrap();

    let args = Args::parse();

    // This is required for certain controllers to work on Windows without the
    // video subsystem enabled:
    sdl2::hint::set("SDL_JOYSTICK_THREAD", "1");

    let mut protocol = Protocol::open();
    protocol.handshake()?;

    eprintln!("Compiling...");
    let engine = Engine::new(
        Config::new()
            .debug_info(true)
            .wasm_backtrace_details(WasmBacktraceDetails::Enable),
    )?;
    let (module, cold_header) =
        load_program(&engine, &args.program).context("Failed to load robot program")?;

    eprintln!("Booting...");

    let state = SdkState::new(module.clone(), cold_header, protocol);

    let mut store = Store::new(&engine, state);

    // Here we get the metadata of the imported indirect function table.
    // User programs will request a varying starting number of entries.
    // If the starting number of entries actually given to the program is too low, it will not start successfully.
    let imported_table_ty = module
        .imports()
        .filter_map(|i| match i.ty() {
            ExternType::Table(table_ty) => Some(table_ty),
            _ => None,
        })
        .next()
        .unwrap();

    let mut linker = Linker::new(&engine);
    let table = Table::new(&mut store, imported_table_ty, Ref::Func(None))?;
    linker.define(&store, "env", "__indirect_function_table", table)?;
    linker.func_wrap(
        "env",
        "sim_log_backtrace",
        |caller: Caller<'_, SdkState>| {
            let backtrace = WasmBacktrace::capture(caller);
            eprintln!("{}", backtrace);
            Ok(())
        },
    )?;

    // Load and compile our module

    let instance = linker.instantiate(&mut store, &module)?;

    // Allocate space for the jump table. 0x700 total pages covers the entire range of the jump table.
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    let target_pages = 0x700;
    let memory_size = memory.size(&store);
    memory.grow(&mut store, target_pages - memory_size)?;

    // Add the jump table to memory and create the WASM FFI interface.
    let jump_table = JumpTable::new(&mut store, memory);
    jump_table.expose(&mut store, &table, &memory)?;

    let run = instance.get_typed_func::<(), ()>(&mut store, "_entry")?;
    store.data_mut().setup()?;
    // We should be ready to actually run the entrypoint now.
    eprintln!("_entry()");
    run.call(&mut store, ())?;

    Ok(())
}
