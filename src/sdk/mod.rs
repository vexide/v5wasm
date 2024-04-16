use std::{collections::HashMap, ffi::CStr, time::Instant};

use bitflags::bitflags;
use piet_common::Device;

use sdl2::GameControllerSubsystem;
use wasmtime::*;

use crate::ProgramOptions;

use self::{
    controller::{build_controller_jump_table, Inputs},
    display::{build_display_jump_table, Display, DISPLAY_HEIGHT, DISPLAY_WIDTH},
};

mod controller;
mod display;

pub struct SdkState<'a> {
    module: Module,
    program_start: Instant,
    display: Display<'a>,
    program_options: ProgramOptions,
    inputs: Inputs,
}

impl<'a> SdkState<'a> {
    pub fn new(module: Module, program_options: ProgramOptions, renderer: &'a mut Device) -> Self {
        let sdl = sdl2::init().unwrap();
        SdkState {
            module,
            display: Display::new(DISPLAY_WIDTH, DISPLAY_HEIGHT, renderer, program_options)
                .unwrap(),
            program_options,
            inputs: Inputs::new(sdl.game_controller().unwrap()),
            program_start: Instant::now(),
        }
    }
}

const JUMP_TABLE_START: usize = 0x037FC000;

pub struct JumpTableBuilder<'a, 'b> {
    store: &'a mut Store<SdkState<'b>>,
    jump_table: JumpTable,
}

impl<'a, 'b> JumpTableBuilder<'a, 'b> {
    /// Inserts a function into the jump table at the given address.
    pub fn insert<Params, Results>(
        &mut self,
        address: usize,
        func: impl IntoFunc<SdkState<'b>, Params, Results>,
    ) {
        let func = Func::wrap(&mut self.store, func);
        self.jump_table.api.insert(address, func);
    }
}

pub struct JumpTable {
    api: HashMap<usize, Func>,
}

impl JumpTable {
    /// Creates a new jump table using the given memory, and populates it with the default API.
    pub fn new(store: &mut Store<SdkState>, memory: Memory) -> Self {
        let mut builder = JumpTableBuilder {
            store,
            jump_table: JumpTable {
                api: HashMap::new(),
            },
        };

        build_display_jump_table(memory, &mut builder);
        build_controller_jump_table(memory, &mut builder);

        // vexSerialWriteBuffer
        builder.insert(
            0x89c,
            move |caller: Caller<'_, SdkState>,
                  channel: i32,
                  data: i32,
                  data_len: i32|
                  -> Result<i32> {
                if channel == 1 {
                    let data_bytes =
                        memory.data(&caller)[data as usize..(data + data_len) as usize].to_vec();
                    let data_str = String::from_utf8(data_bytes).unwrap();
                    print!("{}", data_str);
                }
                Ok(data_len)
            },
        );

        // vexTasksRun
        builder.insert(0x05c, move || {});

        // vexSystemHighResTimeGet
        builder.insert(0x134, move |caller: Caller<'_, SdkState>| -> Result<u64> {
            Ok(caller.data().program_start.elapsed().as_micros() as u64)
        });

        // vexSerialWriteFree
        builder.insert(0x8ac, move |_channel: u32| -> Result<i32> { Ok(2048) });

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
    pub fn expose(self, store: &mut Store<SdkState>, table: &Table, memory: &Memory) -> Result<()> {
        let sdk_base = table.size(&mut *store);
        let api_size = self.api.len() as u32;
        table.grow(&mut *store, api_size, Ref::Func(None))?;
        for (offset, (address, method)) in self.api.into_iter().enumerate() {
            let sdk_index = sdk_base + (offset as u32);
            table.set(&mut *store, sdk_index, Ref::Func(Some(method)))?;
            memory.write(
                &mut *store,
                JUMP_TABLE_START + address,
                &sdk_index.to_le_bytes(),
            )?;
        }
        println!("Jump table exposed with {api_size} functions");
        Ok(())
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

macro_rules! read_c_string {
    ($addr:expr, from $caller:ident using $memory:ident) => {
        $memory
            .read_c_string(&mut $caller, $addr)
            .context("Failed to read C-string")
            .map(|s| s.to_string())
    };
}
pub(crate) use read_c_string;
