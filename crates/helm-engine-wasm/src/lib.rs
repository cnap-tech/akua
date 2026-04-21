//! Rust host for the embedded Helm v4 template engine.
//!
//! The engine is a Go program (in `go-src/`) compiled to `wasip1` and hosted
//! via [`wasmtime`]. The compiled `.wasm` is embedded via `include_bytes!` so
//! the akua binary is self-contained — no external download, no `helm` CLI.
//!
//! ## Why raw wasmtime (not Extism)
//!
//! Helm's dep tree (k8s.io/klog, client-go, etc.) has package `init()`
//! chains that need more WASI capabilities than Extism's deny-all sandbox
//! grants. Since we ship this wasm ourselves — not a third-party plugin —
//! the sandbox threat model doesn't apply. Raw wasmtime with stock WASI
//! works immediately, zero config.
//!
//! ## Why 70+ MB wasm
//!
//! Go's linker can't prune types referenced by a package's public API even
//! if no function that uses them is called. `pkg/engine.New(*rest.Config)`
//! and `pkg/chart/common.DefaultCapabilities = makeDefaultCapabilities()`
//! drag `k8s.io/client-go` transitively. A fork with the `rest.Config`
//! path stripped produces a ~15 MB wasm — see crate README "Option 2".
//! Not done yet; bundled path gives zero maintenance burden.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use wasmtime::{Config, Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

// Cranelift-compiling the 75 MB Go→wasip1 Helm engine takes ~6-8s cold.
// build.rs precompiles it to a native-code `.cwasm` artifact that's
// `Module::deserialize`d here — a memcpy + fixup, not a compile. Ready
// in single-digit milliseconds.
const HELM_ENGINE_CWASM: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/helm-engine.cwasm"));

/// Config used at build time AND runtime. Must match exactly — the
/// deserialized artifact is tied to the engine's `precompile_compatibility_hash`.
pub fn engine_config() -> Config {
    let mut config = Config::new();
    config.wasm_exceptions(true);
    config
}

static COMPILED: OnceLock<(Engine, Module)> = OnceLock::new();

fn compiled_engine() -> Result<&'static (Engine, Module), HelmEngineError> {
    if let Some(x) = COMPILED.get() {
        return Ok(x);
    }
    if HELM_ENGINE_CWASM.is_empty() {
        // build.rs emits an empty placeholder when `assets/helm-engine.wasm`
        // is missing — crate compiles, but runtime fails with a clear error.
        return Err(HelmEngineError::Engine(
            "helm-engine.wasm not built. Run `task build:helm-engine-wasm` to produce the Go→wasip1 artifact at crates/helm-engine-wasm/assets/helm-engine.wasm, then rebuild.".to_string(),
        ));
    }
    let engine = Engine::new(&engine_config()).map_err(wasm_err)?;
    // SAFETY: the cwasm bytes were produced by build.rs against an engine
    // with the same `engine_config()`. They're embedded at compile time, so
    // tampering requires tampering with the akua binary itself.
    let module = unsafe { Module::deserialize(&engine, HELM_ENGINE_CWASM) }
        .map_err(wasm_err)?;
    let _ = COMPILED.set((engine, module));
    Ok(COMPILED.get().expect("just set"))
}

#[derive(Debug, thiserror::Error)]
pub enum HelmEngineError {
    #[error("wasmtime: {0}")]
    Wasm(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("serializing input: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
}

impl Default for Release {
    fn default() -> Self {
        Self {
            name: "release".to_string(),
            namespace: "default".to_string(),
            revision: 1,
            service: "Helm".to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RenderRequest<'a> {
    chart_tar_gz_b64: String,
    values_yaml: &'a str,
    release: Release,
}

#[derive(Debug, Deserialize)]
struct RenderResponse {
    #[serde(default)]
    manifests: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    error: String,
}

/// Render a chart tarball with values via the embedded Helm engine.
/// Returns `<template-path>` → rendered YAML (matches
/// `helm.sh/helm/v4/pkg/engine.Render`'s output shape).
pub fn render(
    chart_tar_gz: &[u8],
    values_yaml: &str,
    release: &Release,
) -> Result<std::collections::BTreeMap<String, String>, HelmEngineError> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(chart_tar_gz);
    let req = RenderRequest {
        chart_tar_gz_b64: b64,
        values_yaml,
        release: release.clone(),
    };
    let input = serde_json::to_vec(&req)?;

    let output = call_wasm(&input)?;
    let resp: RenderResponse = serde_json::from_slice(&output)?;
    if !resp.error.is_empty() {
        return Err(HelmEngineError::Engine(resp.error));
    }
    Ok(resp.manifests)
}

/// Render from a chart directory on disk (convenience wrapper).
pub fn render_dir(
    chart_dir: &Path,
    chart_name: &str,
    values_yaml: &str,
    release: &Release,
) -> Result<std::collections::BTreeMap<String, String>, HelmEngineError> {
    let tar_gz = tar_chart_dir(chart_dir, chart_name)?;
    render(&tar_gz, values_yaml, release)
}

fn tar_chart_dir(chart_dir: &Path, chart_name: &str) -> Result<Vec<u8>, HelmEngineError> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(chart_name, chart_dir)?;
        tar.finish()?;
    }
    gz.flush()?;
    Ok(gz.finish()?)
}

fn call_wasm(input: &[u8]) -> Result<Vec<u8>, HelmEngineError> {
    let (engine, module) = compiled_engine()?;

    // klog's init() reads os.Args[0] unconditionally — an empty argv crashes
    // Go's runtime with index-out-of-range. We provide a dummy arg and nothing
    // else from the host.
    let wasi = WasiCtxBuilder::new().arg("helm-engine").build_p1();
    let mut store = Store::new(engine, wasi);
    let mut linker: Linker<WasiP1Ctx> = Linker::new(engine);
    p1::add_to_linker_sync(&mut linker, |s: &mut WasiP1Ctx| s).map_err(wasm_err)?;

    let instance = linker.instantiate(&mut store, module).map_err(wasm_err)?;
    // Reactor module: `_initialize` runs Go runtime + package init() chains
    // without exiting. Exports are callable after.
    if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
        init.call(&mut store, ()).map_err(wasm_err)?;
    }

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| wasm_err("helm-engine.wasm missing `memory` export".to_string()))?;
    let malloc = instance
        .get_typed_func::<i32, i32>(&mut store, "helm_malloc")
        .map_err(wasm_err)?;
    let free = instance
        .get_typed_func::<i32, ()>(&mut store, "helm_free")
        .map_err(wasm_err)?;
    let render_fn = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "helm_render")
        .map_err(wasm_err)?;
    let result_len = instance
        .get_typed_func::<i32, i32>(&mut store, "helm_result_len")
        .map_err(wasm_err)?;

    // Copy input into wasm memory
    let input_ptr = copy_in(&mut store, &malloc, memory, input).map_err(wasm_err)?;

    // Call render
    let result_ptr = render_fn
        .call(&mut store, (input_ptr, input.len() as i32))
        .map_err(wasm_err)?;
    let len = result_len.call(&mut store, result_ptr).map_err(wasm_err)?;
    let bytes = copy_out(&store, memory, result_ptr, len).map_err(wasm_err)?;

    // Best-effort cleanup — even if this fails, the wasm instance is dropped.
    let _ = free.call(&mut store, input_ptr);
    let _ = free.call(&mut store, result_ptr);

    Ok(bytes)
}

fn copy_in<T>(
    store: &mut Store<T>,
    malloc: &TypedFunc<i32, i32>,
    memory: Memory,
    bytes: &[u8],
) -> Result<i32, wasmtime::Error> {
    let ptr = malloc.call(&mut *store, bytes.len() as i32)?;
    let data = memory.data_mut(&mut *store);
    let start = ptr as usize;
    data[start..start + bytes.len()].copy_from_slice(bytes);
    Ok(ptr)
}

fn copy_out<T>(
    store: &Store<T>,
    memory: Memory,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, wasmtime::Error> {
    let data = memory.data(store);
    let start = ptr as usize;
    let end = start + len as usize;
    Ok(data[start..end].to_vec())
}

fn wasm_err<E: std::fmt::Display>(e: E) -> HelmEngineError {
    HelmEngineError::Wasm(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_chart(tmp: &Path) -> Vec<u8> {
        let chart = tmp.join("mychart");
        std::fs::create_dir_all(chart.join("templates")).unwrap();
        std::fs::write(
            chart.join("Chart.yaml"),
            "apiVersion: v2\nname: mychart\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(chart.join("values.yaml"), "greeting: hi\n").unwrap();
        std::fs::write(
            chart.join("templates/cm.yaml"),
            r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}-cm
data:
  greeting: {{ .Values.greeting | quote }}
"#,
        )
        .unwrap();
        super::tar_chart_dir(&chart, "mychart").unwrap()
    }

    /// When the real Go→wasip1 artifact isn't built, lib.rs emits a 0-byte
    /// placeholder via build.rs. These tests only exercise the real engine;
    /// gate them on the artifact's presence so workspace-wide `cargo test`
    /// stays green in dev envs that haven't run `task build:helm-engine-wasm`.
    fn engine_is_built() -> bool {
        HELM_ENGINE_CWASM.len() > 1_000_000
    }

    #[test]
    fn embedded_cwasm_bytes_present_or_placeholder() {
        // Either real (>1 MB) or explicit 0-byte placeholder. Anything in
        // between means build.rs produced a corrupted cwasm.
        assert!(
            HELM_ENGINE_CWASM.is_empty() || HELM_ENGINE_CWASM.len() > 1_000_000,
            "helm-engine.cwasm has suspicious size: {} bytes",
            HELM_ENGINE_CWASM.len()
        );
    }

    #[test]
    fn renders_minimal_chart() {
        if !engine_is_built() {
            eprintln!("skipping: helm-engine.wasm not built (run `task build:helm-engine-wasm`)");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let tar_gz = minimal_chart(tmp.path());
        let release = Release {
            name: "demo".to_string(),
            namespace: "default".to_string(),
            revision: 1,
            service: "Helm".to_string(),
        };
        let out = render(&tar_gz, "greeting: hello\n", &release).expect("render");
        let (_path, yaml) = out
            .iter()
            .find(|(k, _)| k.ends_with("templates/cm.yaml"))
            .expect("cm.yaml rendered");
        assert!(yaml.contains("demo-cm"), "rendered: {yaml}");
        assert!(yaml.contains("hello"), "rendered: {yaml}");
    }

    #[test]
    fn render_error_propagates_from_plugin() {
        if !engine_is_built() {
            eprintln!("skipping: helm-engine.wasm not built");
            return;
        }
        // Truncated tarball → engine returns an error.
        let result = render(&[0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0], "", &Release::default());
        match result {
            Err(HelmEngineError::Engine(_)) => {}
            other => panic!("expected Engine error, got {other:?}"),
        }
    }
}
