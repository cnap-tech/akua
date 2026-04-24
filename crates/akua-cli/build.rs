//! Build-time: AOT-compile `akua-render-worker.wasm` → `.cwasm` so the
//! per-render `Module::deserialize` at runtime is a memcpy, not a
//! multi-second Cranelift compile.
//!
//! The `.wasm` itself is produced by `task build:render-worker`
//! (cargo build against `wasm32-wasip1` in `crates/akua-render-worker/`).
//! This build.rs expects that artifact to already exist; it does NOT
//! recurse into cargo to build the worker. That keeps the build
//! topology cycle-free — akua-cli depends on the precompiled .cwasm,
//! the worker is built with its own cargo invocation.
//!
//! If the worker .wasm isn't present we emit a sentinel empty `.cwasm`
//! and a build-time warning. The runtime host checks for the zero-length
//! marker and surfaces `E_SANDBOX_UNAVAILABLE` so users get a clear
//! "run `task build:render-worker` first" message instead of a cryptic
//! wasmtime parse error.

use std::path::PathBuf;

// Matches runtime Config at `src/render_worker.rs` — MUST stay in sync
// or `Module::deserialize` rejects the artifact via the compat-hash
// check.
fn worker_config() -> wasmtime::Config {
    let mut c = wasmtime::Config::new();
    c.consume_fuel(true);
    c.epoch_interruption(true);
    c
}

fn main() {
    let worker_wasm = workspace_root()
        .join("target/wasm32-wasip1/release/akua-render-worker.wasm");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let cwasm_out = out_dir.join("akua-render-worker.cwasm");

    // Invalidate on .wasm content change; no other inputs feed this.
    println!("cargo:rerun-if-changed={}", worker_wasm.display());
    println!("cargo:rerun-if-changed=build.rs");

    if !worker_wasm.exists() {
        println!(
            "cargo:warning=akua-render-worker.wasm not found at {} — run `task build:render-worker` first. Emitting empty sandbox module (runtime will surface E_SANDBOX_UNAVAILABLE).",
            worker_wasm.display()
        );
        // Zero-length marker — render_worker.rs checks for this and
        // returns a typed error on first invocation.
        std::fs::write(&cwasm_out, []).expect("write empty cwasm marker");
        return;
    }

    let wasm = std::fs::read(&worker_wasm)
        .unwrap_or_else(|e| panic!("read worker wasm from {}: {e}", worker_wasm.display()));
    let engine = wasmtime::Engine::new(&worker_config())
        .expect("wasmtime::Engine::new(worker_config)");
    let cwasm = engine
        .precompile_module(&wasm)
        .expect("precompile_module failed");
    std::fs::write(&cwasm_out, cwasm).expect("write cwasm");
    println!(
        "cargo:warning=akua-render-worker.cwasm: {} bytes (AOT from {})",
        std::fs::metadata(&cwasm_out).map(|m| m.len()).unwrap_or(0),
        worker_wasm.display()
    );
}

/// akua-cli's build.rs runs with CWD = crates/akua-cli. The workspace
/// root sits two parents up.
fn workspace_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/akua-cli parent chain")
        .to_path_buf()
}
