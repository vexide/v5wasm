use std::{collections::HashMap, ffi::CStr, time::Instant};

use anyhow::bail;
use bitflags::bitflags;

use serial::{build_serial_jump_table, Serial};
use vexide_simulator_protocol::{Command, CompMode, CompetitionMode, Event, LogLevel};
use wasmtime::*;

use crate::{
    protocol::{self, Log, Protocol},
    ProgramOptions,
};

use self::{
    controller::{build_controller_jump_table, Inputs},
    display::{build_display_jump_table, Display},
};

mod controller;
pub mod display;
mod serial;

/// The state of the SDK, containing the program's WASM module, the robot display, and other peripherals.
pub struct SdkState {
    module: Module,
    program_start: Instant,
    display: Display,
    program_options: ProgramOptions,
    inputs: Inputs,
    competition_mode: CompetitionMode,
    protocol: Protocol,
    is_executing: bool,
    serial: Serial,
}

impl SdkState {
    pub fn new(module: Module, program_options: ProgramOptions, protocol: Protocol) -> Self {
        let sdl = sdl2::init().unwrap();
        let start = Instant::now();
        SdkState {
            module,
            display: Display::new(program_options, start),
            program_options,
            inputs: Inputs::new(sdl),
            program_start: start,
            competition_mode: CompetitionMode::default(),
            protocol,
            is_executing: false,
            serial: Serial::new(),
        }
    }

    /// Signal that the simulator is ready to begin and process all setup commands.
    pub fn setup(&mut self) -> anyhow::Result<()> {
        self.protocol.send(&Event::Ready)?;
        while !self.is_executing {
            self.recv_command()?;
        }
        Ok(())
    }

    /// Process the next command, blocking if it hasn't been received yet.
    pub fn recv_command(&mut self) -> anyhow::Result<()> {
        let cmd = self.protocol.next()?;
        self.execute_command(cmd)
    }

    /// Process all available commands.
    pub fn recv_all_commands(&mut self) -> anyhow::Result<()> {
        while let Some(cmd) = self.protocol.try_next()? {
            self.execute_command(cmd)?;
        }
        Ok(())
    }

    /// Process a command.
    pub fn execute_command(&mut self, cmd: Command) -> anyhow::Result<()> {
        match cmd {
            Command::Handshake { .. } => unreachable!(),
            Command::Touch { pos, event } => todo!(),
            Command::ControllerUpdate(primary, partner) => {
                self.inputs.set_controller(0, primary)?;
                self.inputs.set_controller(1, partner)?;
            }
            Command::USD { root } => todo!(),
            Command::VEXLinkOpened { port, mode } => todo!(),
            Command::VEXLinkClosed { port } => todo!(),
            Command::CompetitionMode(mode) => {
                self.competition_mode = mode;
            }
            Command::ConfigureDevice { port, device } => todo!(),
            Command::AdiInput { port, voltage } => todo!(),
            Command::StartExecution => {
                if self.is_executing {
                    bail!("Cannot start execution twice");
                }

                self.is_executing = true;
            }
            Command::SetBatteryCapacity { capacity } => todo!(),
            Command::SetTextMetrics { text, metrics } => {
                self.display.set_metrics_cache(text, metrics);
            }
            Command::Serial(serial_data) => {
                self.serial
                    .buffer_input(serial_data.channel, &serial_data.to_bytes()?)?;
            }
        }
        Ok(())
    }

    /// Returns whether the simulator is in the execution phase.
    pub fn executing(&self) -> bool {
        self.is_executing
    }

    pub fn run_tasks(&mut self) -> anyhow::Result<()> {
        self.recv_all_commands()?;
        self.inputs.update()?;
        self.serial.flush(&mut self.protocol)?;
        Ok(())
    }
}

impl Log for SdkState {
    fn log(&mut self, level: LogLevel, message: String) -> protocol::Result<()> {
        self.protocol.send(&Event::Log { level, message })?;
        Ok(())
    }
}

const JUMP_TABLE_START: usize = 0x037FC000;

/// Wrapper for the jump table which allows for easily adding new functions to it.
pub struct JumpTableBuilder<'a> {
    store: &'a mut Store<SdkState>,
    jump_table: JumpTable,
}

impl<'a> JumpTableBuilder<'a> {
    /// Inserts a function into the jump table at the given address.
    pub fn insert<Params, Results>(
        &mut self,
        address: usize,
        func: impl IntoFunc<SdkState, Params, Results>,
    ) {
        debug_assert!(
            !self.jump_table.api.contains_key(&address),
            "Duplicate jump table function at address {:#x}",
            address
        );
        let func = Func::wrap(&mut self.store, func);
        self.jump_table.api.insert(address, func);
    }
}

/// A set of function pointers in memory which can be called by the WebAssembly module to perform SDK operations.
///
/// Addresses are the same as in the real VEX SDK and the `vex-sdk` rust crate.
pub struct JumpTable {
    api: HashMap<usize, Func>,
}

impl JumpTable {
    /// Creates a new jump table which will use the given memory, and populates it with the default API.
    ///
    /// No changes are actually to the user program made apart from creating the resources for the jump table.
    pub fn new(store: &mut Store<SdkState>, memory: Memory) -> Self {
        let mut builder = JumpTableBuilder {
            store,
            jump_table: JumpTable {
                api: HashMap::new(),
            },
        };

        build_display_jump_table(memory, &mut builder);
        build_controller_jump_table(memory, &mut builder);
        build_serial_jump_table(memory, &mut builder);

        // vexTasksRun
        builder.insert(0x05c, move |mut caller: Caller<'_, SdkState>| {
            caller.data_mut().run_tasks()
        });

        // vexSystemHighResTimeGet
        builder.insert(0x134, move |caller: Caller<'_, SdkState>| -> Result<u64> {
            Ok(caller.data().program_start.elapsed().as_micros() as u64)
        });

        // vexSystemExitRequest
        builder.insert(0x130, move || {
            std::process::exit(0);
        });

        bitflags! {
            /// The status bits returned by [`vex_sdk::vexCompetitionStatus`].
            #[derive(Debug, Clone, Copy, Eq, PartialEq)]
            struct CompetitionStatus: u32 {
                /// Robot is disabled by field control.
                const DISABLED = 1 << 0;

                /// Robot is in autonomous mode.
                const AUTONOMOUS = 1 << 1;

                /// Robot is connected to competition control (either competition switch or field control).
                const CONNECTED = 1 << 2;

                /// Robot is connected to field control (NOT competition switch)
                const SYSTEM = 1 << 3;
            }
        }

        // vexCompetitionStatus
        builder.insert(0x9d8, move || -> u32 { CompetitionStatus::empty().bits() });

        builder.jump_table
    }

    /// Applies the memory and table changes required to expose the jump table to the WebAssembly module.
    ///
    /// The memory must be big enough to hold the jump table. The indirect function table will be expanded with
    /// enough new slots to hold all the functions in the jump table.
    pub fn expose(self, store: &mut Store<SdkState>, table: &Table, memory: &Memory) -> Result<()> {
        let sdk_base = table.size(&mut *store);
        let api_size = self.api.len() as u32;
        table.grow(&mut *store, api_size, Ref::Func(None))?;

        for (offset, (address, method)) in self.api.into_iter().enumerate() {
            let sdk_index = sdk_base + (offset as u32);
            // Expose the function to the WASM module. The index of the function in the indirect function table is not constant.
            table.set(&mut *store, sdk_index, Ref::Func(Some(method)))?;
            // Write the index of the function to a constant location in the jump table memory.
            memory.write(
                &mut *store,
                JUMP_TABLE_START + address,
                &sdk_index.to_le_bytes(),
            )?;
        }
        store
            .data_mut()
            .trace(format!("Jump table exposed with {api_size} functions"))?;
        Ok(())
    }
}

pub trait MemoryExt {
    /// Utility method for reading a C-style string from this memory. Handles converting the bytes to a UTF-8 string.
    ///
    /// The string is guaranteed to exist for its entire lifetime, but because it is borrowed, it isn't possible for
    /// API consumers to call back into WASM code while holding it.
    fn read_c_string<'a>(&self, store: &'a impl AsContext, offset: usize) -> Option<&'a str>;
}

impl MemoryExt for Memory {
    fn read_c_string<'a>(&self, store: &'a impl AsContext, offset: usize) -> Option<&'a str> {
        let bytes = &self.data(store)[offset..];
        let c_str = CStr::from_bytes_until_nul(bytes).ok()?;
        c_str.to_str().ok()
    }
}

/// Utility macro for cloning a C-style string into simulator memory, returning a Result.
macro_rules! clone_c_string {
    ($addr:expr, from $caller:ident using $memory:ident) => {
        $memory
            .read_c_string(&mut $caller, $addr)
            .context("Failed to read C-string")
            .map(|s| s.to_string())
    };
}
pub(crate) use clone_c_string;
