// Wasm capability environment (ADR 0021): run a `.wasm`/`.wat` module under
// wasmtime + WASI, isolated — no network (WASI p1 has no sockets) and no
// filesystem (no preopened dirs). Args arrive via the CODECODER_CAPABILITY_ARGS
// env var; stdout/stderr are captured.
use std::path::Path;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::pipe::MemoryOutputPipe;
use wasmtime_wasi::WasiCtxBuilder;

/// Run `<cap_dir>/<entry>` (a wasm binary or wat text). Returns (output, is_error).
pub fn run_wasm(cap_dir: &Path, entry: &str, args_json: &str) -> anyhow::Result<(String, bool)> {
    let path = cap_dir.join(entry);
    let bytes = std::fs::read(&path)?;

    let engine = Engine::default();
    // Module::new accepts a wasm binary or (with the default `wat` feature) wat text.
    let module = Module::new(&engine, &bytes)?;

    let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
    preview1::add_to_linker_sync(&mut linker, |t| t)?;

    let stdout = MemoryOutputPipe::new(1 << 20);
    let stderr = MemoryOutputPipe::new(1 << 20);
    let wasi = WasiCtxBuilder::new()
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .env("CODECODER_CAPABILITY_ARGS", args_json)
        .build_p1();

    let mut store = Store::new(&engine, wasi);
    linker.module(&mut store, "", &module)?;
    let run = linker
        .get_default(&mut store, "")?
        .typed::<(), ()>(&store)?;
    let result = run.call(&mut store, ());
    drop(store); // release the pipe writers before reading

    let out = String::from_utf8_lossy(&stdout.contents()).into_owned();
    let err = String::from_utf8_lossy(&stderr.contents()).into_owned();
    match result {
        Ok(()) => Ok((if out.is_empty() { err } else { format!("{out}{err}") }, false)),
        Err(e) => Ok((format!("{out}{err}\ntrap: {e}"), true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal WASI module that writes "hello-wasm\n" to stdout via fd_write.
    const HELLO_WAT: &str = r#"(module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 8) "hello-wasm\n")
      (func (export "_start")
        (i32.store (i32.const 0) (i32.const 8))
        (i32.store (i32.const 4) (i32.const 11))
        (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 20)))))"#;

    #[test]
    fn runs_wasi_module_and_captures_stdout() {
        let dir = std::env::temp_dir().join(format!("cc_wasm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.wat"), HELLO_WAT).unwrap();
        let (out, err) = run_wasm(&dir, "m.wat", "{}").unwrap();
        assert!(!err, "{out}");
        assert!(out.contains("hello-wasm"), "{out}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
