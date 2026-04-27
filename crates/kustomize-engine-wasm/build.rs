//! Stage `assets/kustomize-engine.wasm` into OUT_DIR (and AOT-compile
//! to `.cwasm` when the `precompile` feature is on). See
//! `engine_host_wasm::build_engine_wasm` for the shared logic.

fn main() {
    engine_host_wasm::build_engine_wasm("kustomize-engine");
}
