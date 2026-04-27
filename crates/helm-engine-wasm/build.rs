//! Stage `assets/helm-engine.wasm` into OUT_DIR (and AOT-compile
//! to `.cwasm` when the `precompile` feature is on). Shared logic
//! lives in `engine_host_wasm::build_engine_wasm` — the helm and
//! kustomize shims call the same helper with their respective name.

fn main() {
    engine_host_wasm::build_engine_wasm("helm-engine");
}
