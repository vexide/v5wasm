use std::{
    collections::VecDeque,
    io::{stdin, stdout, StdoutLock},
    sync::mpsc::{self, RecvError, TryRecvError},
};

use jsonl::ReadError;
use snafu::{IntoError, OptionExt, ResultExt, Snafu};
use vexide_simulator_protocol::{Command, Event};

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

type Result<T, E = ProtocolError> = std::result::Result<T, E>;

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

    pub fn handshake(&mut self) -> Result<()> {
        if self.handshake_finished {
            panic!("Attempted to perform handshake twice");
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
