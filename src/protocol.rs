use std::{
    collections::VecDeque,
    io::{stdin, stdout, StdoutLock},
    sync::mpsc::{self, TryRecvError},
};

use jsonl::ReadError;
use snafu::{OptionExt, ResultExt, Snafu};
use vexide_simulator_protocol::{Command, Event, LogLevel};
use wasmtime::{AsContext, WasmBacktrace};

#[derive(Debug, Snafu)]
pub enum ProtocolError {
    #[snafu(context(false))]
    Send {
        source: jsonl::WriteError,
    },
    #[snafu(context(false))]
    Recv {
        source: jsonl::ReadError,
    },
    RecvWorkerStopped,
    ReceivedInvalidCommandDuringHandshake {
        command: Command,
    },
    ReceivedHandshakeAttemptAfterHandshakeFinished,
    IncompatibleFrontendVersion {
        expected: i32,
        got: i32,
    },
}

pub type Result<T, E = ProtocolError> = std::result::Result<T, E>;

pub struct Protocol {
    handshake_finished: bool,
    outbound: StdoutLock<'static>,
    pub inbound: mpsc::Receiver<Result<Command, jsonl::ReadError>>,
    command_process_queue: VecDeque<Command>,
}

impl Protocol {
    pub fn open() -> Self {
        let stdout_lock = stdout().lock();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let stdin_lock = stdin().lock();
            let msg = match jsonl::read(stdin_lock) {
                Ok(msg) => Ok(msg),
                Err(ReadError::Eof) => std::process::exit(0),
                Err(err) => Err(err),
            };

            if tx.send(msg).is_err() {
                break;
            }
        });

        Self {
            handshake_finished: false,
            outbound: stdout_lock,
            inbound: rx,
            command_process_queue: VecDeque::new(),
        }
    }

    pub fn send(&mut self, event: &Event) -> Result<()> {
        Ok(jsonl::write(&mut self.outbound, event)?)
    }

    pub fn try_next(&mut self) -> Result<Option<Command>> {
        let cmd = self
            .command_process_queue
            .pop_front()
            .map_or_else(|| self.try_recv(), |x| Ok(Some(x)))?;
        Ok(cmd)
    }

    pub fn try_recv(&mut self) -> Result<Option<Command>> {
        match self.inbound.try_recv() {
            Ok(Ok(Command::Handshake { .. })) if self.handshake_finished => {
                ReceivedHandshakeAttemptAfterHandshakeFinishedSnafu.fail()
            }
            Ok(msg) => Ok(Some(msg?)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(_) => RecvWorkerStoppedSnafu.fail(),
        }
    }

    pub fn next(&mut self) -> Result<Command> {
        let cmd = self
            .command_process_queue
            .pop_front()
            .map_or_else(|| self.recv(), Ok)?;
        Ok(cmd)
    }

    pub fn recv(&mut self) -> Result<Command> {
        let msg = self.inbound.recv().ok().context(RecvWorkerStoppedSnafu)??;
        if matches!(msg, Command::Handshake { .. }) && self.handshake_finished {
            return ReceivedHandshakeAttemptAfterHandshakeFinishedSnafu.fail();
        }
        Ok(msg)
    }

    pub fn handshake(&mut self, implied: bool) -> Result<()> {
        if self.handshake_finished {
            panic!("Attempted to perform handshake twice");
        }

        if implied {
            self.handshake_finished = true;
            return Ok(());
        }

        const COMPATIBLE_PROTOCOL_VERSION: i32 = 1;

        let handshake = self.next()?;
        let (version, _) = match handshake {
            Command::Handshake {
                version,
                extensions,
            } => (version, extensions),
            command => return ReceivedInvalidCommandDuringHandshakeSnafu { command }.fail(),
        };

        if version < COMPATIBLE_PROTOCOL_VERSION {
            return IncompatibleFrontendVersionSnafu {
                expected: COMPATIBLE_PROTOCOL_VERSION,
                got: version,
            }
            .fail();
        }

        self.send(&Event::Handshake {
            version: COMPATIBLE_PROTOCOL_VERSION,
            extensions: vec![],
        })?;

        self.handshake_finished = true;

        Ok(())
    }

    /// Blocks until a command has been received that satisfies the condition, then executes the command.
    pub fn wait_for_command(
        &mut self,
        check: impl Fn(&Command) -> bool,
    ) -> anyhow::Result<Command> {
        loop {
            let cmd = self.recv()?;
            if check(&cmd) {
                return Ok(cmd);
            } else {
                self.command_process_queue.push_back(cmd);
            }
        }
    }
}

pub trait Log {
    fn log(&mut self, level: LogLevel, message: String) -> Result<()>;
    fn trace(&mut self, message: impl Into<String>) -> Result<()> {
        self.log(LogLevel::Trace, message.into())
    }
    fn info(&mut self, message: impl Into<String>) -> Result<()> {
        self.log(LogLevel::Info, message.into())
    }
    fn warn(&mut self, message: impl Into<String>) -> Result<()> {
        self.log(LogLevel::Warn, message.into())
    }
    fn error(&mut self, message: impl Into<String>) -> Result<()> {
        self.log(LogLevel::Error, message.into())
    }
}

impl Log for Protocol {
    fn log(&mut self, level: LogLevel, message: String) -> Result<()> {
        self.send(&Event::Log { level, message })
    }
}

macro_rules! warn_bt {
    ($ctx:expr, $($arg:tt)*) => {{
        let bt = wasmtime::WasmBacktrace::capture(&$ctx);
        $ctx.data_mut().warn(format!($($arg)*))?;
        $ctx.data_mut().warn(bt.to_string())?;
        Ok::<(), anyhow::Error>(())
    }};
}

pub(crate) use warn_bt;

macro_rules! error_bt {
    ($ctx:expr, $($arg:tt)*) => {{
        let bt = wasmtime::WasmBacktrace::capture(&$ctx);
        $ctx.data_mut().error(format!($($arg)*))?;
        $ctx.data_mut().error(bt.to_string())?;
        Ok::<(), anyhow::Error>(())
    }};
}

pub(crate) use error_bt;
