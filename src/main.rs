//! Example of instantiating two modules which link to each other.

// You can execute this example with `cargo run --example linking`

use wasmtime::*;

fn main() -> Result<()> {
    let engine = Engine::new(
        Config::new()
            .debug_info(true)
            .wasm_backtrace_details(WasmBacktraceDetails::Enable),
    )?;
    let mut store = Store::new(&engine, ());
    println!("Compiling...");
    let module = Module::from_file(
        &engine,
        "../vexide/target/wasm32-unknown-unknown/debug/examples/basic.wasm",
    )?;
    println!("Booting...");
    let imports = module
        .imports()
        .filter_map(|i| match i.ty() {
            ExternType::Table(table_ty) => Some(table_ty),
            _ => None,
        })
        .next()
        .unwrap();

    // First set up our linker which is going to be linking modules together. We
    // want our linker to have wasi available, so we set that up here as well.
    let mut linker = Linker::new(&engine);
    let memory_ty = MemoryType::new(0x10000, None);
    let memory = Memory::new(&mut store, memory_ty)?;
    linker.define(&store, "env", "memory", memory)?;
    let table = Table::new(&mut store, imports, Ref::Func(None))?;
    linker.define(&store, "env", "__indirect_function_table", table)?;
    linker.func_wrap("env", "sim_log_backtrace", |caller: Caller<'_, ()>| {
        let backtrace = WasmBacktrace::capture(&caller);
        println!("backtrace: {:?}", backtrace);
        Ok(())
    })?;

    // Load and compile our two modules

    // Configure WASI and insert it into a `Store`

    // Instantiate our first module which only uses WASI, then register that
    // instance with the linker since the next linking will use it.

    // And with that we can perform the final link and the execute the module.
    let instance = linker.instantiate(&mut store, &module)?;
    let table_base = table.size(&store);
    table.grow(&mut store, 2, Ref::Func(None))?;
    let vex_serial_write_buffer = Func::wrap(
        &mut store,
        move |mut caller: Caller<'_, ()>, channel: i32, data: i32, data_len: i32| {
            println!(
                "// vexSerialWriteBuffer({}, {}, {})",
                channel, data, data_len
            );
            if channel == 1 {
                let data_bytes =
                    memory.data(&caller)[data as usize..(data + data_len) as usize].to_vec();
                let data_str = String::from_utf8(data_bytes).unwrap();
                print!("{}", data_str);
            }
            Ok(data_len)
        },
    );
    table.set(
        &mut store,
        table_base + 1,
        Ref::Func(Some(vex_serial_write_buffer)),
    )?;
    let unimplemented_addr = (table_base).to_le_bytes();
    let write_buffer_addr = (table_base + 1).to_le_bytes();
    // fill 0x37fc000 to 0x3800000 with unimplemented_addr, but only every 4 bytes so that it is readable
    for i in 0..(0x4000 / 4) {
        memory.write(&mut store, 0x37fc000 + i * 4, &unimplemented_addr)?;
    }
    memory.write(&mut store, 0x37fc89c, &write_buffer_addr)?;
    let run = instance.get_typed_func::<(), ()>(&mut store, "_entry")?;
    run.call(&mut store, ())?;
    Ok(())
}
