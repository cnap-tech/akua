//! Precompile the embedded Helm engine WASM → native-code `.cwasm` artifact.
//!
//! Runtime code (`src/lib.rs`) deserializes this artifact with
//! `Module::deserialize`, which is a memcpy + fixup instead of a full
//! Cranelift compile (~6-8s for the 75 MB Go→wasip1 module).
//!
//! The artifact is target-specific: each `(arch, OS, wasmtime version,
//! Config flags)` tuple needs its own `.cwasm`. Release CI produces one
//! per target in the matrix.
//!
//! The engine config here MUST match `engine_config()` in src/lib.rs —
//! otherwise `precompile_compatibility_hash` differs and deserialize fails.

use std::env;
use std::fs;
use std::path::PathBuf;

use wasmtime::{Config, Engine};

fn main() {
    let wasm_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("helm-engine.wasm");

    println!("cargo:rerun-if-changed={}", wasm_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let dest = out.join("helm-engine.cwasm");

    if !wasm_path.is_file() {
        // Dev-friendly fallback: emit an empty placeholder so the crate
        // compiles without the Go toolchain. Runtime use will fail the
        // length-check in `src/lib.rs`, surfacing the same roadmap message
        // Packages see via `helm.template`.
        println!(
            "cargo:warning=helm-engine.wasm missing at {} — crate builds with a 0-byte placeholder. Run `task build:helm-engine-wasm` to produce the real artifact.",
            wasm_path.display()
        );
        fs::write(&dest, []).expect("write empty helm-engine.cwasm placeholder");
        return;
    }

    let wasm = fs::read(&wasm_path).expect("read helm-engine.wasm");

    let mut config = Config::new();
    config.wasm_exceptions(true);

    let engine = Engine::new(&config).expect("create wasmtime engine");
    let cwasm = engine
        .precompile_module(&wasm)
        .expect("precompile helm-engine.wasm");

    fs::write(&dest, &cwasm).expect("write helm-engine.cwasm");

    println!(
        "cargo:warning=precompiled helm-engine.wasm ({} MB) -> {} MB cwasm",
        wasm.len() / 1_048_576,
        cwasm.len() / 1_048_576
    );
}
