use std::{
    collections::HashMap,
    ffi::CStr,
    ops::{Index, IndexMut},
    time::Instant,
};

use anyhow::Context;
use piet_common::{
    kurbo::{Point, Size},
    Piet,
};
use piet_common::{BitmapTarget, Device};
use png::{ColorType, Encoder};
use wasmtime::*;

use self::display::{build_display_jump_table, ColorExt, Display, DISPLAY_HEIGHT, DISPLAY_WIDTH};

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

        // vexSerialWriteBuffer
        builder.insert(
            0x89c,
            move |caller: Caller<'_, SdkState>,
                  channel: i32,
                  data: i32,
                  data_len: i32|
                  -> Result<i32> {
                {
                    if channel == 1 {
                        let data_bytes = memory.data(&caller)
                            [data as usize..(data + data_len) as usize]
                            .to_vec();
                        let data_str = String::from_utf8(data_bytes).unwrap();
                        print!("{}", data_str);
                    }
                    Ok(data_len)
                }
            },
        );

        // vexTasksRun
        builder.insert(0x05c, move || {});

        // vexSystemHighResTimeGet
        builder.insert(0x134, move |caller: Caller<'_, SdkState>| -> Result<u64> {
            {
                Ok(caller.data().program_start.elapsed().as_micros() as u64)
            }
        });

        // vexSerialWriteFree
        builder.insert(0x8ac, move |_channel: u32| -> Result<i32> {
            {
                Ok(2048)
            }
        });

        // vexSystemExitRequest
        builder.insert(0x130, move || {
            std::process::exit(0);
        });

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
