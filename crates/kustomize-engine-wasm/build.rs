//! Precompile kustomize-engine.wasm → native-code .cwasm via the shared
//! `engine-host-wasm` helper.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let wasm_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("kustomize-engine.wasm");

    println!("cargo:rerun-if-changed={}", wasm_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let dest = out.join("kustomize-engine.cwasm");

    if !wasm_path.is_file() {
        println!(
            "cargo:warning=kustomize-engine.wasm missing at {} — crate builds with a 0-byte placeholder. Run `task build:kustomize-engine-wasm` to produce the real artifact.",
            wasm_path.display()
        );
        fs::write(&dest, []).expect("write empty kustomize-engine.cwasm placeholder");
        return;
    }

    let wasm = fs::read(&wasm_path).expect("read kustomize-engine.wasm");
    let cwasm = engine_host_wasm::precompile(&wasm).expect("precompile kustomize-engine.wasm");
    fs::write(&dest, &cwasm).expect("write kustomize-engine.cwasm");

    println!(
        "cargo:warning=precompiled kustomize-engine.wasm ({} MB) -> {} MB cwasm",
        wasm.len() / 1_048_576,
        cwasm.len() / 1_048_576
    );
}
