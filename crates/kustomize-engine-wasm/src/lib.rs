//! Rust host for the embedded kustomize engine.
//!
//! Parallel to `helm-engine-wasm`: a Go program (in `go-src/`) compiled to
//! `wasip1` and hosted via wasmtime, embedded via `include_bytes!`.
//!
//! ## Sandbox posture
//!
//! Per CLAUDE.md ("Sandboxed by default. No shell-out, ever."), the engine
//! runs inside a wasmtime WASI context with:
//!
//! - No preopened filesystem — the overlay tree travels as a tar.gz over
//!   linear memory, unpacked by the guest into kustomize's in-memory
//!   `filesys.FileSystem`. The guest never sees a host path.
//! - No network (`wasip1` has no socket syscalls).
//! - Dummy `argv` only.

use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use wasmtime::{Config, Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

const KUSTOMIZE_ENGINE_CWASM: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/kustomize-engine.cwasm"));

/// Config used at build time AND runtime. Must match exactly — the
/// deserialized artifact is tied to `precompile_compatibility_hash`.
pub fn engine_config() -> Config {
    let mut config = Config::new();
    config.wasm_exceptions(true);
    config
}

static COMPILED: OnceLock<(Engine, Module)> = OnceLock::new();

fn compiled_engine() -> Result<&'static (Engine, Module), KustomizeEngineError> {
    if let Some(x) = COMPILED.get() {
        return Ok(x);
    }
    if KUSTOMIZE_ENGINE_CWASM.is_empty() {
        return Err(KustomizeEngineError::Engine(
            "kustomize-engine.wasm not built. Run `task build:kustomize-engine-wasm` to produce the Go→wasip1 artifact at crates/kustomize-engine-wasm/assets/kustomize-engine.wasm, then rebuild.".to_string(),
        ));
    }
    let engine = Engine::new(&engine_config()).map_err(wasm_err)?;
    // SAFETY: the cwasm bytes were produced by build.rs against an engine
    // with the same `engine_config()`. They're embedded at compile time.
    let module = unsafe { Module::deserialize(&engine, KUSTOMIZE_ENGINE_CWASM) }
        .map_err(wasm_err)?;
    let _ = COMPILED.set((engine, module));
    Ok(COMPILED.get().expect("just set"))
}

#[derive(Debug, thiserror::Error)]
pub enum KustomizeEngineError {
    #[error("wasmtime: {0}")]
    Wasm(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("serializing input: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Serialize)]
struct BuildRequest<'a> {
    overlay_tar_gz_b64: String,
    entrypoint: &'a str,
}

#[derive(Debug, Deserialize)]
struct BuildResponse {
    #[serde(default)]
    yaml: String,
    #[serde(default)]
    error: String,
}

/// Render a kustomize overlay directory. `overlay_dir` is tar.gz'd into
/// the guest under `entrypoint/`, then `kustomize build entrypoint` runs
/// over the in-memory filesystem. Returns the rendered multi-doc YAML.
pub fn render_dir(overlay_dir: &Path) -> Result<String, KustomizeEngineError> {
    let entrypoint = overlay_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("overlay")
        .to_string();
    // We need the overlay dir PLUS any parent overlays referenced via
    // `resources: [../base]`. Simplest approach: tar the parent dir and
    // hand `<parent-name>/<overlay-name>` as the entrypoint. That way
    // the guest has access to both `../base` (sibling dir) and the
    // overlay itself.
    let parent = overlay_dir.parent().ok_or_else(|| {
        KustomizeEngineError::Engine(format!(
            "overlay dir has no parent: {}",
            overlay_dir.display()
        ))
    })?;
    let parent_name = parent
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pkg")
        .to_string();
    let tar_gz = tar_dir(parent, &parent_name)?;
    let guest_entrypoint = format!("{parent_name}/{entrypoint}");

    render_tar(&tar_gz, &guest_entrypoint)
}

/// Render from an already-tar.gz'd overlay tree. `entrypoint` is the
/// path (inside the tarball) of the directory containing
/// `kustomization.yaml`.
pub fn render_tar(tar_gz: &[u8], entrypoint: &str) -> Result<String, KustomizeEngineError> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(tar_gz);
    let req = BuildRequest {
        overlay_tar_gz_b64: b64,
        entrypoint,
    };
    let input = serde_json::to_vec(&req)?;
    let output = call_wasm(&input)?;
    let resp: BuildResponse = serde_json::from_slice(&output)?;
    if !resp.error.is_empty() {
        return Err(KustomizeEngineError::Engine(resp.error));
    }
    Ok(resp.yaml)
}

fn tar_dir(dir: &Path, name_in_archive: &str) -> Result<Vec<u8>, KustomizeEngineError> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(name_in_archive, dir)?;
        tar.finish()?;
    }
    gz.flush()?;
    Ok(gz.finish()?)
}

/// Persistent wasmtime Instance for the life of the thread. See
/// `crates/helm-engine-wasm/src/lib.rs::Session` for the rationale —
/// amortizes `_initialize` (Go runtime + kustomize package inits)
/// across multiple `kustomize.build` calls inside one akua render.
struct Session {
    store: Store<WasiP1Ctx>,
    memory: Memory,
    malloc: TypedFunc<i32, i32>,
    free: TypedFunc<i32, ()>,
    build_fn: TypedFunc<(i32, i32), i32>,
    result_len: TypedFunc<i32, i32>,
}

impl Session {
    fn init() -> Result<Self, KustomizeEngineError> {
        let (engine, module) = compiled_engine()?;
        let wasi = WasiCtxBuilder::new().arg("kustomize-engine").build_p1();
        let mut store = Store::new(engine, wasi);
        let mut linker: Linker<WasiP1Ctx> = Linker::new(engine);
        p1::add_to_linker_sync(&mut linker, |s: &mut WasiP1Ctx| s).map_err(wasm_err)?;
        let instance = linker.instantiate(&mut store, module).map_err(wasm_err)?;
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            init.call(&mut store, ()).map_err(wasm_err)?;
        }
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| wasm_err("kustomize-engine.wasm missing `memory` export".to_string()))?;
        let malloc = instance
            .get_typed_func::<i32, i32>(&mut store, "kustomize_malloc")
            .map_err(wasm_err)?;
        let free = instance
            .get_typed_func::<i32, ()>(&mut store, "kustomize_free")
            .map_err(wasm_err)?;
        let build_fn = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "kustomize_build")
            .map_err(wasm_err)?;
        let result_len = instance
            .get_typed_func::<i32, i32>(&mut store, "kustomize_result_len")
            .map_err(wasm_err)?;
        Ok(Session { store, memory, malloc, free, build_fn, result_len })
    }

    fn call(&mut self, input: &[u8]) -> Result<Vec<u8>, KustomizeEngineError> {
        let input_ptr = copy_in(&mut self.store, &self.malloc, self.memory, input)
            .map_err(wasm_err)?;
        let result_ptr = self
            .build_fn
            .call(&mut self.store, (input_ptr, input.len() as i32))
            .map_err(wasm_err)?;
        let len = self
            .result_len
            .call(&mut self.store, result_ptr)
            .map_err(wasm_err)?;
        let bytes = copy_out(&self.store, self.memory, result_ptr, len).map_err(wasm_err)?;
        let _ = self.free.call(&mut self.store, input_ptr);
        let _ = self.free.call(&mut self.store, result_ptr);
        Ok(bytes)
    }
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

fn call_wasm(input: &[u8]) -> Result<Vec<u8>, KustomizeEngineError> {
    SESSION.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            *borrow = Some(Session::init()?);
        }
        borrow.as_mut().expect("just initialized").call(input)
    })
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

fn wasm_err<E: std::fmt::Display>(e: E) -> KustomizeEngineError {
    KustomizeEngineError::Wasm(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_is_built() -> bool {
        KUSTOMIZE_ENGINE_CWASM.len() > 1_000_000
    }

    #[test]
    fn embedded_cwasm_bytes_present_or_placeholder() {
        assert!(
            KUSTOMIZE_ENGINE_CWASM.is_empty() || KUSTOMIZE_ENGINE_CWASM.len() > 1_000_000,
            "kustomize-engine.cwasm has suspicious size: {} bytes",
            KUSTOMIZE_ENGINE_CWASM.len()
        );
    }

    #[test]
    fn renders_minimal_overlay() {
        if !engine_is_built() {
            eprintln!("skipping: kustomize-engine.wasm not built");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        let base = pkg.join("base");
        let overlay = pkg.join("overlay");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(base.join("kustomization.yaml"), "resources:\n  - configmap.yaml\n").unwrap();
        std::fs::write(
            base.join("configmap.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: hello\ndata:\n  greeting: hi\n",
        )
        .unwrap();
        std::fs::write(
            overlay.join("kustomization.yaml"),
            "resources:\n  - ../base\nnamePrefix: prod-\n",
        )
        .unwrap();

        let yaml = render_dir(&overlay).expect("render");
        assert!(yaml.contains("prod-hello"), "rendered: {yaml}");
        assert!(yaml.contains("greeting: hi"), "rendered: {yaml}");
    }
}
