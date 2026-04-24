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
        Some(json) => serde_json::from_str(json)
            .map_err(|e| JsError::new(&format!("inputs JSON parse: {e}")))?,
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

// ---------------------------------------------------------------------------
// Pure-compute verbs — lint, fmt, inspect, check, tree, diff.
//
// Each takes source strings / JSON maps, returns a JSON string the
// JS side parses. No filesystem access, no network — just akua-core
// primitives wrapped for the wasm-bindgen boundary.
// ---------------------------------------------------------------------------

/// Parse a Package.k source buffer and return lint issues.
/// JSON shape: `{ "status": "ok"|"fail", "issues": [...] }` —
/// matches `akua lint --json`.
#[wasm_bindgen]
pub fn lint(filename: &str, source: &str) -> Result<String, JsError> {
    let issues =
        akua_core::lint_kcl_source(filename, source).map_err(|e| JsError::new(&e.to_string()))?;
    let status = if issues.is_empty() { "ok" } else { "fail" };
    serde_json::to_string(&serde_json::json!({
        "status": status,
        "issues": issues,
    }))
    .map_err(|e| JsError::new(&e.to_string()))
}

/// Format a KCL source buffer. `check_mode=true` is read-only and
/// reports `changed` per file; `check_mode=false` returns the
/// formatted text in the `formatted` field (JS writes back to disk).
/// JSON shape: `{ "files": [{ "path": "<filename>", "changed": bool }], "formatted": "..." }`.
#[wasm_bindgen]
pub fn fmt(filename: &str, source: &str, check_mode: bool) -> Result<String, JsError> {
    let formatted = akua_core::format_kcl(source).map_err(|e| JsError::new(&e.to_string()))?;
    let changed = formatted != source;
    let out = serde_json::json!({
        "files": [{ "path": filename, "changed": changed }],
        "formatted": if check_mode { String::new() } else { formatted },
    });
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Introspect a Package.k source buffer — list its `option()` call
/// sites for SDK consumers that want to drive inputs programmatically.
/// JSON shape matches `akua inspect --json --package …` (kind=package).
#[wasm_bindgen]
pub fn inspect_package(filename: &str, source: &str) -> Result<String, JsError> {
    let options = akua_core::list_options_kcl_source(filename, source)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_json::to_string(&serde_json::json!({
        "kind": "package",
        "path": filename,
        "options": options,
    }))
    .map_err(|e| JsError::new(&e.to_string()))
}

/// Run the three structural gates. Every source is optional — the
/// CLI verb surfaces missing-file errors at the file-reading layer;
/// this pure primitive only checks the source buffers it's given.
#[wasm_bindgen]
pub fn check(
    manifest: Option<String>,
    lock: Option<String>,
    package_filename: Option<String>,
    package_source: Option<String>,
) -> Result<String, JsError> {
    let package = match (&package_filename, &package_source) {
        (Some(f), Some(s)) => Some((f.as_str(), s.as_str())),
        _ => None,
    };
    let out = akua_core::check_from_sources(manifest.as_deref(), lock.as_deref(), package);
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Walk manifest + optional lock and produce the tree output.
#[wasm_bindgen]
pub fn tree(manifest: &str, lock: Option<String>) -> Result<String, JsError> {
    let out = akua_core::tree_from_sources(manifest, lock.as_deref())
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Diff two `{ "path": "sha256-hex" }` maps passed as JSON strings.
/// Returns the `DirDiff` JSON shape `akua diff --json` emits.
#[wasm_bindgen]
pub fn diff(before_json: &str, after_json: &str) -> Result<String, JsError> {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn parse_map(json: &str) -> Result<BTreeMap<PathBuf, String>, JsError> {
        let raw: std::collections::HashMap<String, String> = serde_json::from_str(json)
            .map_err(|e| JsError::new(&format!("diff input JSON: {e}")))?;
        Ok(raw
            .into_iter()
            .map(|(k, v)| (PathBuf::from(k), v))
            .collect())
    }
    let before = parse_map(before_json)?;
    let after = parse_map(after_json)?;
    let out = akua_core::dir_diff::diff_manifests(&before, &after);
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}
