# V5Wasm

> Execute vexide programs using a WebAssembly sandbox.

V5Wasm is a code executor for VEX V5 programs compiled to WebAssembly.
By emulating the V5's jump table, it can run programs intended for the robot brain with minimal modifications.
V5Wasm is not a GUI or application; instead, it's a simulator engine that could power one.

If you'd like any help getting this working, don't hesitate to ask on the Vexide Discord server linked on [our website](https://vexide.dev/)!

## Building

Prerequisites:

- A recent version of Rust
- CMake, so that Cargo can build SDL2

Apart from that, you should be able to run `cargo install --git "https://github.com/vexide/v5wasm.git"` to install the engine.

You might also be able to get builds from the "Releases" tab on Github, but they could be out of date or missing.

## Getting Started

V5Wasm expects a single argument `<PROGRAM>` which is the path to the `.wasm` program that you would like to run. In Rust projects,
this file is located at `./target/wasm32-unknown-unknown/debug/{crate-name}.wasm`.

Example usage:

```sh
cargo pros build -s
v5wasm ./target/wasm32-unknown-unknown/debug/vexide-template.wasm
```

After starting, V5Wasm will attempt to initiate a [Vexide Simulator Protocol](https://internals.vexide.dev//simulators/protocol) session over its standard output and standard input streams.

### Building the WASM file

V5Wasm doesn't work with every `.wasm` file, so you'll have to follow these instructions to make one that's compatible.

To make a vexide project compatible with the engine, ensure the following code snippet is somewhere in `.cargo/config.toml`:

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-Clink-arg=--export-memory", "-Clink-arg=--import-table"]
```

Then, compile the project with `cargo pros build -s` or `cargo build --target wasm32-unknown-unknown`.

### Interacting with the simulator

You can use a debugger (such as LLDB or CodeLLDB in VS Code) to set breakpoints inside *simulated robot code*, as long as you built your program with debug symbols.
Check out `.vscode/launch.json` and `.vscode/tasks.json` for an example of what that might look like. Those files currently expect there to be a `../vexide` repo, and they compile the `basic` example. If you have a symlink from `../vexide/target/..../basic.wasm` to `program.wasm`, it should work correctly.

What's working:

- ALL of the serial SDK!
- The controller SDK
- Some of the display SDK
- Some of the system and tasks SDK
- A little bit of the competition SDK

An incomplete list of what's missing:

- Every Device API (yeah...)
- Stdin
- Touch support for the display

## Understanding error messages

If the simulator crashes with "No such file or directory", your program is probably missing.

If you get a "wasm trap: uninitialized element" error, it's possible an SDK call isn't implemented yet. For example, this error means `vexBatteryCurrentGet` isn't implemented:

```
Error: error while executing at wasm backtrace:
    0: 0x69340 - vex_sdk::battery::vexBatteryCurrentGet::hc5f5e7af7e7aca72
                    at /vex-sdk-0.12.3/src/lib.rs:79:21
    1: 0x9452 - basic::main::{{closure}}::hfcc5ba2ee817eb06
                    at /basic.rs:14:9

        (...more backtrace...)

   10: 0x17b47 - vexide_startup::program_entry::hbc8e02734bc7e165
                    at /vexide-startup/src/lib.rs:117:9
   11: 0xa17d - _entry
                    at /basic.rs:8:1

Caused by:
    wasm trap: uninitialized element
```
