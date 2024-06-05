use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use anyhow::{anyhow, Context};
use bytes::{Buf, Bytes};
use clap::Parser as _;
use fs_err as fs;

use protocol::{Log, Protocol};
use rgb::RGB8;
use sdk::{
    display::{BLACK, WHITE},
    SdlRequest,
};
use sdl2::{
    controller::{Axis, Button},
};
use vexide_simulator_protocol::{Command, ControllerState, Event, VCodeSig};
use wasmparser::{Parser, Payload};
use wasmtime::*;

use crate::sdk::{JumpTable, SdkState};

mod printf;
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
    /// Skips the protocol handshake and immediately starts execution.
    #[clap(long, short = 'I')]
    imply_start: bool,
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
fn load_program(
    engine: &Engine,
    path: &Path,
    protocol: &mut Protocol,
) -> Result<(Module, ProgramOptions)> {
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
    let v_code_sig = VCodeSig::new(&cold_header);
    let magic = cold_header.copy_to_bytes(HEADER_MAGIC.len());
    if magic != HEADER_MAGIC {
        return Err(anyhow::anyhow!("Invalid magic number"));
    }

    protocol.send(&Event::VCodeSig(v_code_sig))?;

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

fn start(args: Args, sdl_request_channel: mpsc::Sender<SdlRequest>) -> Result<()> {
    let mut protocol = Protocol::open();
    protocol.handshake(args.imply_start)?;

    protocol.info("Compiling...")?;
    let engine = Engine::new(
        Config::new()
            .debug_info(true)
            .wasm_backtrace_details(WasmBacktraceDetails::Enable),
    )?;
    let (module, cold_header) = load_program(&engine, &args.program, &mut protocol)
        .context("Failed to load robot program")?;

    protocol.info("Booting...")?;

    let state = SdkState::new(module.clone(), cold_header, protocol, sdl_request_channel);

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
        |mut caller: Caller<'_, SdkState>| {
            let backtrace = WasmBacktrace::capture(&caller);
            caller.data_mut().error(format!("{}", backtrace))?;
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
    if args.imply_start {
        store.data_mut().execute_command(Command::StartExecution)?;
    }
    store.data_mut().setup()?;
    // We should be ready to actually run the entrypoint now.
    store.data_mut().trace("Calling _entry()")?;
    run.call(&mut store, ())?;
    Ok(())
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

    let sdl = sdl2::init().unwrap();
    let (tx, rx) = mpsc::channel();

    let mut event_pump = sdl.event_pump().unwrap();
    let joystick_subsystem = sdl.joystick().unwrap();
    let controller_subsystem = sdl.game_controller().unwrap();

    thread::spawn(move || {
        start(args, tx).unwrap();
    });

    while let Ok(req) = rx.recv() {
        match req {
            SdlRequest::EventPump => {
                event_pump.pump_events();
            }
            SdlRequest::V5Controller { guid, response } => {
                let val = || {
                    let joysticks = controller_subsystem
                        .num_joysticks()
                        .map_err(|s| anyhow!(s))?;
                    for idx in 0..joysticks {
                        if controller_subsystem.is_game_controller(idx) {
                            let Ok(joystick) = joystick_subsystem.open(idx) else {
                                break;
                            };
                            if joystick.guid() != guid || !joystick.attached() {
                                continue;
                            }
                            let Ok(sdl_controller) = controller_subsystem.open(idx) else {
                                continue;
                            };

                            return anyhow::Ok(Some(ControllerState {
                                axis1: (sdl_controller.axis(Axis::LeftX) as i32) * 127
                                    / (i16::MAX as i32),
                                axis2: -(sdl_controller.axis(Axis::LeftY) as i32) * 127
                                    / (i16::MAX as i32),
                                axis3: -(sdl_controller.axis(Axis::RightY) as i32) * 127
                                    / (i16::MAX as i32),
                                axis4: (sdl_controller.axis(Axis::RightX) as i32) * 127
                                    / (i16::MAX as i32),
                                button_l1: sdl_controller.button(Button::LeftShoulder),
                                button_l2: sdl_controller.axis(Axis::TriggerLeft) > 0,
                                button_r1: sdl_controller.button(Button::RightShoulder),
                                button_r2: sdl_controller.axis(Axis::TriggerRight) > 0,
                                button_up: sdl_controller.button(Button::DPadUp),
                                button_down: sdl_controller.button(Button::DPadDown),
                                button_left: sdl_controller.button(Button::DPadLeft),
                                button_right: sdl_controller.button(Button::DPadRight),
                                button_x: sdl_controller.button(Button::X),
                                button_b: sdl_controller.button(Button::B),
                                button_y: sdl_controller.button(Button::Y),
                                button_a: sdl_controller.button(Button::A),
                                battery_capacity: 0,
                                battery_level: 0,
                                button_all: false,
                                button_sel: false,
                                flags: 0,
                            }));
                        }
                    }
                    Ok(None)
                };

                _ = response.send(val());
            }
        }
    }

    Ok(())
}
