//! `akua-wasm` — browser/Node/Deno/Bun-facing WASM bundle for
//! `@akua/sdk`. Lightweight `wasm-bindgen` wrapper around
//! `akua-core`'s pure-KCL render path.
//!
//! Scope for v0.1.0 first slice: a single `render(source, inputs)`
//! entry point that evaluates a Package.k text buffer and returns
//! the rendered YAML. Engine callouts (`helm.template`,
//! `kustomize.build`) are NOT yet available — Packages that import
//! them fail with a diagnostic pointing at the CLI, until Phase 4B
//! finishes engine bundling.
//!
//! Target is `wasm32-unknown-unknown` (runs directly under
//! `WebAssembly.instantiate` in any JS runtime). The CLI's sandbox
//! path uses `wasm32-wasip1` inside wasmtime — a different target
//! because the CLI has a WASI host available and the browser
//! doesn't.

use wasm_bindgen::prelude::*;

/// Evaluate a Package.k source buffer against an inputs JSON value
/// and return the rendered YAML.
///
/// * `package_filename` is used for diagnostic rendering only; no
///   filesystem is touched (there isn't one).
/// * `source` is the Package.k KCL text.
/// * `inputs_json` is an optional JSON string to inject as KCL's
///   `option("input")`. Pass `null` or an empty string for no
///   inputs.
///
/// Returns the rendered top-level YAML (same shape the CLI's
/// sandbox path returns). Errors surface as JS exceptions carrying
/// the KCL diagnostic text.
#[wasm_bindgen]
pub fn render(
    package_filename: &str,
    source: &str,
    inputs_json: Option<String>,
) -> Result<String, JsError> {
    let inputs: serde_yaml::Value = match inputs_json.as_deref() {
        None | Some("") | Some("null") => serde_yaml::Value::Mapping(Default::default()),
        Some(json) => serde_json::from_str(json).map_err(|e| {
            JsError::new(&format!("inputs JSON parse: {e}"))
        })?,
    };

    akua_core::package_k::eval_source_full(
        std::path::Path::new(package_filename),
        source,
        &inputs,
        None,
    )
    .map_err(|e| JsError::new(&e.to_string()))
}

/// Version tag — cheap sanity check for JS consumers that the
/// bundle they loaded matches what they expect.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
