use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use anyhow::{anyhow, bail, Context};
use vexide_simulator_protocol::{Event, SerialData};
use wasmtime::*;

use crate::{protocol::Protocol, sdk::SdkState};

use super::JumpTableBuilder;

// MARK: Jump table

pub fn build_serial_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    // vexSerialWriteChar
    builder.insert(
        0x898,
        move |mut caller: Caller<'_, SdkState>, channel: u32, c: u32| -> Result<i32> {
            let written = caller.data_mut().serial.write(channel, &[c as u8]);
            Ok(written.map(|w| w as i32).unwrap_or(-1))
        },
    );
    // vexSerialWriteBuffer
    builder.insert(
        0x89c,
        move |mut caller: Caller<'_, SdkState>, channel: u32, data: u32, len: u32| -> Result<i32> {
            let (memory, sdk) = memory.data_and_store_mut(&mut caller);
            let buffer = &memory[data as usize..(data + len) as usize];
            let written = sdk.serial.write(channel, buffer);
            Ok(written.map(|w| w as i32).unwrap_or(-1))
        },
    );
    // vexSerialReadChar
    builder.insert(
        0x8a0,
        move |mut caller: Caller<'_, SdkState>, channel: u32| -> Result<i32> {
            let byte = caller
                .data_mut()
                .serial
                .read_byte(channel)
                .map(|c| c as i32);
            Ok(byte.unwrap_or(-1))
        },
    );
    // vexSerialPeekChar
    builder.insert(
        0x8a4,
        move |mut caller: Caller<'_, SdkState>, channel: u32| -> Result<i32> {
            let byte = caller
                .data_mut()
                .serial
                .peek_byte(channel)
                .map(|c| c as i32);
            Ok(byte.unwrap_or(-1))
        },
    );
    // vexSerialWriteFree
    // TODO: Can this return input buffer capacity?
    builder.insert(
        0x8ac,
        move |mut caller: Caller<'_, SdkState>, channel: u32| -> Result<i32> {
            let num_free = caller
                .data_mut()
                .serial
                .num_free_bytes(channel)
                .map(|f| f as i32);
            // FIXME: What do invalid channels return?
            Ok(num_free.unwrap_or(0))
        },
    );
    // TODO: vex_printf and related functions
}

// MARK: API

const STDOUT_BUFFER_SIZE: usize = 2048;
const STDIN_BUFFER_SIZE: usize = 4096;

pub struct Serial {
    stdout_buffer: Cursor<[u8; STDOUT_BUFFER_SIZE]>,
    stdin_buffer: Cursor<[u8; STDIN_BUFFER_SIZE]>,
}

impl Serial {
    pub fn new() -> Self {
        Self {
            stdout_buffer: Cursor::new([0; STDOUT_BUFFER_SIZE]),
            stdin_buffer: Cursor::new([0; STDIN_BUFFER_SIZE]),
        }
    }

    pub fn write(&mut self, channel: u32, buffer: &[u8]) -> Result<usize> {
        match channel {
            1 => {
                let count = self
                    .stdout_buffer
                    .write(buffer)
                    .context("Failed to write to stdout")?;
                Ok(count)
            }
            _ => Err(anyhow!("Invalid channel")),
        }
    }

    pub fn buffer_input(&mut self, channel: u32, buffer: &[u8]) -> Result<()> {
        match channel {
            1 => {
                self.stdin_buffer
                    .write_all(buffer)
                    .context("Failed to write to stdin")?;
                Ok(())
            }
            _ => Err(anyhow!("Invalid channel")),
        }
    }

    pub fn read_byte(&mut self, channel: u32) -> Result<u8> {
        match channel {
            1 => {
                let byte = self.peek_byte(channel)?;
                self.stdin_buffer.seek(SeekFrom::Current(-1)).unwrap();
                Ok(byte)
            }
            _ => Err(anyhow!("Invalid channel")),
        }
    }

    pub fn peek_byte(&mut self, channel: u32) -> Result<u8> {
        match channel {
            1 => {
                let pos = self.stdin_buffer.position();
                if pos == 0 {
                    bail!("No data in stdin buffer");
                }
                let idx = pos - 1;
                let byte = self.stdin_buffer.get_ref()[idx as usize];
                Ok(byte)
            }
            _ => Err(anyhow!("Invalid channel")),
        }
    }

    pub fn num_free_bytes(&mut self, channel: u32) -> Result<usize> {
        match channel {
            1 => Ok(STDOUT_BUFFER_SIZE - self.stdout_buffer.position() as usize),
            _ => Err(anyhow!("Invalid channel")),
        }
    }

    pub fn flush(&mut self, protocol: &mut Protocol) -> Result<()> {
        if self.stdout_buffer.position() == 0 {
            return Ok(());
        }
        let stdout = std::mem::replace(
            &mut self.stdout_buffer,
            Cursor::new([0; STDOUT_BUFFER_SIZE]),
        );
        let len = stdout.position() as usize;
        let bytes = &stdout.into_inner()[0..len];
        protocol.send(&Event::Serial(SerialData::new(1, bytes)))?;
        Ok(())
    }
}
