# VEX SDK Simulator

> Execute WebAssembly programs that rely on the VEX V5 SDK and jump table.

If you'd like any help getting this working, don't hesitate to ask on the Vexide Discord server linked on [our website](https://pros.rs/)!

## Building

You will need Cmake installed so that Cargo can build SDL2.

Apart from that, you should be able to run `cargo install --git "https://github.com/vexide/vex-sdk-sim.git"` to install the simulator.

You might also be able to get builds from the "Releases" tab on Github, but they could be out of date or missing.

## Getting Started

The simulator expects a single argument `<PROGRAM>` which is the path to the `.wasm` program that you would like to run. In Rust projects,
this file is located at `./target/wasm32-unknown-unknown/debug/{crate-name}.wasm`.

Example usage:

```sh
cargo pros build -s
vex-sdk-sim ./target/wasm32-unknown-unknown/debug/vexide-template.wasm
```

### Building the WASM file

You can't just simulate any `.wasm` file! The simulator will error out early if the program does not have a V5 code signature ("cold header").

To make a Vexide project compatible with the simulator, add code snippet to `.cargo/config.toml`:

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-Clink-arg=--export-memory", "-Clink-arg=--import-table"]
```

Then, compile the project with `cargo build --target wasm32-unknown-unknown` or `cargo pros build -s`.

### Interacting with the simulator

You can use a debugger (such as LLDB or CodeLLDB in VS Code) to set breakpoints inside *simulated robot code*, as long as you built your program with debug symbols.
Check out `.vscode/launch.json` and `.vscode/tasks.json` for an example of what that might look like. Those files currently expect there to be a `../vexide` repo, and they compile the `basic` example. If you have a symlink from `../vexide/target/..../basic.wasm` to `program.wasm`, it should work correctly.

Serial output to Stdout will be sent over the simulator's Stdout.

If you plug in a game controller SDL2 should detect it and patch it into the simulator, usually automatically.

Display output will be saved to `./display.png`. If you're using VS Code it might be helpful to split-screen this file!

What's missing:

- Every Device API (yeah...)
- Stdin/Stderr
- Touch support for the display
- [Vexide Simulator Interface](https://internals.pros.rs/simulators/interface)
- A few random missing APIs, if you need them just ask in the Discord mentioned above and I'll fix it (or show you how to fix it if you want)

## Understanding error messages

If the program crashes with "No such file or directory", your program is probably missing.

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
