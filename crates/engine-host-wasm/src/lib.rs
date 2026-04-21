//! Shared wasmtime host for akua's embedded Go→wasip1 rendering engines.
//!
//! Each engine (helm, kustomize, future kro/CEL/kyverno) is a Go program
//! compiled to `wasm32-wasip1` with the same C-ABI shape:
//!
//! - `<prefix>_malloc(size i32) -> i32`
//! - `<prefix>_free(ptr i32)`
//! - `<entry>(input_ptr i32, input_len i32) -> i32`
//! - `<prefix>_result_len(ptr i32) -> i32`
//!
//! This crate provides:
//!
//! 1. [`precompile`] — `.wasm` → `.cwasm` helper used by each engine
//!    crate's `build.rs` (so deserialize at runtime is a memcpy + fixup,
//!    not a Cranelift compile).
//! 2. [`engine_config`] — the `Config` that produced the `.cwasm`;
//!    runtime must use the identical shape or `Module::deserialize`
//!    fails the compatibility-hash check.
//! 3. [`Session`] — a persistent wasmtime `Store` + `Instance` + typed
//!    function handles. Thread-local in each engine crate; amortizes
//!    the Go runtime's `_initialize` across every plugin call in a
//!    process. See `docs/performance.md §5.1`.
//! 4. [`EngineSpec`] — the tuple of export names + WASI args specific
//!    to one engine, handed to [`Session::init`].
//! 5. [`EngineHostError`] — shared error enum; plugin crates wrap it
//!    in their own typed error via `#[from]`.
//!
//! ## Sandbox posture
//!
//! Per CLAUDE.md ("Sandboxed by default. No shell-out, ever."), every
//! session runs with:
//!
//! - No preopened filesystem (guest talks to the host only through the
//!   linear-memory JSON ABI).
//! - No inherited env, no socket syscalls (wasip1 has none).
//! - Dummy `argv[0]` only — `klog.init()` crashes on empty argv.

use std::cell::RefCell;

use wasmtime::{Config, Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

// --- Shared errors ---------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum EngineHostError {
    #[error("wasmtime: {0}")]
    Wasm(String),

    /// Returned when the engine crate's embedded `.cwasm` is empty — the
    /// `.wasm` artifact wasn't produced at build time. Each plugin
    /// crate's `Session::init` must check this before calling us.
    #[error("engine: {0}")]
    Engine(String),
}

fn wasm_err<E: std::fmt::Display>(e: E) -> EngineHostError {
    EngineHostError::Wasm(e.to_string())
}

// --- Build-time: precompile wasm → cwasm -----------------------------------

/// The `Config` used to precompile `.cwasm`. Build-time and runtime MUST
/// use the same shape or `Module::deserialize` rejects the artifact.
pub fn engine_config() -> Config {
    let mut config = Config::new();
    config.wasm_exceptions(true);
    config
}

/// Called from each engine crate's `build.rs`. Precompiles a `.wasm`
/// to a platform-specific `.cwasm`; deserialize at runtime is a fixup
/// instead of a full Cranelift compile.
pub fn precompile(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let engine = Engine::new(&engine_config()).map_err(|e| e.to_string())?;
    engine.precompile_module(wasm).map_err(|e| e.to_string())
}

// --- Engine spec -----------------------------------------------------------

/// The export-name tuple + WASI argv[0] specific to one engine. Passed
/// to [`Session::init`] by each plugin crate.
#[derive(Debug, Clone, Copy)]
pub struct EngineSpec {
    /// Human-readable name used for error messages + `argv[0]`.
    pub name: &'static str,
    /// Export symbol of the allocator function: `<prefix>_malloc`.
    pub malloc: &'static str,
    /// Export symbol of the deallocator function: `<prefix>_free`.
    pub free: &'static str,
    /// Export symbol of the entry-point function: `helm_render` / `kustomize_build` / etc.
    pub entry: &'static str,
    /// Export symbol of the result-length probe: `<prefix>_result_len`.
    pub result_len: &'static str,
}

// --- Persistent session ----------------------------------------------------

/// A wasmtime Instance with pre-looked-up typed-function handles. Built
/// once per thread (via the plugin crate's `thread_local! { SESSION }`);
/// every subsequent plugin call reuses it, so the Go `_initialize` chain
/// (klog, package inits) runs exactly once.
pub struct Session {
    store: Store<WasiP1Ctx>,
    memory: Memory,
    malloc: TypedFunc<i32, i32>,
    free: TypedFunc<i32, ()>,
    entry: TypedFunc<(i32, i32), i32>,
    result_len: TypedFunc<i32, i32>,
}

impl Session {
    /// Instantiate the engine from an embedded `.cwasm` slice. The slice
    /// being empty is the "artifact wasn't built" signal — returns
    /// [`EngineHostError::Engine`] with a pointer to the Taskfile target.
    pub fn init(cwasm: &[u8], spec: EngineSpec) -> Result<Self, EngineHostError> {
        if cwasm.is_empty() {
            return Err(EngineHostError::Engine(format!(
                "{}.wasm not built. Run `task build:{}-wasm` to produce the Go→wasip1 artifact, then rebuild.",
                spec.name, spec.name
            )));
        }

        let engine = Engine::new(&engine_config()).map_err(wasm_err)?;
        // SAFETY: `cwasm` was produced by `precompile()` against the
        // same `engine_config()` shape. Embedded at compile time, so
        // tampering requires tampering with the akua binary itself.
        let module = unsafe { Module::deserialize(&engine, cwasm) }.map_err(wasm_err)?;

        let wasi = WasiCtxBuilder::new().arg(spec.name).build_p1();
        let mut store = Store::new(&engine, wasi);
        let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
        p1::add_to_linker_sync(&mut linker, |s: &mut WasiP1Ctx| s).map_err(wasm_err)?;

        let instance = linker.instantiate(&mut store, &module).map_err(wasm_err)?;
        // Reactor module: `_initialize` runs Go runtime + package init
        // chains (klog, sprig, helm/kustomize inits). Exports callable
        // after. Runs once per thread here.
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            init.call(&mut store, ()).map_err(wasm_err)?;
        }

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| wasm_err(format!("{}.wasm missing `memory` export", spec.name)))?;
        let malloc = instance
            .get_typed_func::<i32, i32>(&mut store, spec.malloc)
            .map_err(wasm_err)?;
        let free = instance
            .get_typed_func::<i32, ()>(&mut store, spec.free)
            .map_err(wasm_err)?;
        let entry = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, spec.entry)
            .map_err(wasm_err)?;
        let result_len = instance
            .get_typed_func::<i32, i32>(&mut store, spec.result_len)
            .map_err(wasm_err)?;

        Ok(Session {
            store,
            memory,
            malloc,
            free,
            entry,
            result_len,
        })
    }

    /// Round-trip a JSON input through the engine. The guest's entry-point
    /// ABI: `(input_ptr, input_len) -> result_ptr`, result is
    /// NUL-terminated so we probe length via `<prefix>_result_len`.
    pub fn call(&mut self, input: &[u8]) -> Result<Vec<u8>, EngineHostError> {
        let input_ptr = copy_in(&mut self.store, &self.malloc, self.memory, input)?;
        let result_ptr = self
            .entry
            .call(&mut self.store, (input_ptr, input.len() as i32))
            .map_err(wasm_err)?;
        let len = self
            .result_len
            .call(&mut self.store, result_ptr)
            .map_err(wasm_err)?;
        let bytes = copy_out(&self.store, self.memory, result_ptr, len);

        // Best-effort: guest reuses freed pointers on the next alloc;
        // a dropped free here costs at most a bit of linear-memory
        // fragmentation.
        let _ = self.free.call(&mut self.store, input_ptr);
        let _ = self.free.call(&mut self.store, result_ptr);

        Ok(bytes)
    }
}

/// Convenience wrapper: `thread_local!` this in the plugin crate, call
/// [`thread_local_call`] from the plugin entry-point.
pub type SessionSlot = RefCell<Option<Session>>;

/// Lazily initialize + invoke `SESSION` on the caller's thread. Moves
/// the `cwasm` + `spec` borrow through the init branch only.
pub fn thread_local_call<F>(
    slot: &SessionSlot,
    cwasm: &[u8],
    spec: EngineSpec,
    input: &[u8],
    mut wrap_err: F,
) -> Result<Vec<u8>, EngineHostError>
where
    F: FnMut(EngineHostError) -> EngineHostError,
{
    let mut borrow = slot.borrow_mut();
    if borrow.is_none() {
        *borrow = Some(Session::init(cwasm, spec).map_err(&mut wrap_err)?);
    }
    borrow
        .as_mut()
        .expect("just initialized")
        .call(input)
        .map_err(wrap_err)
}

fn copy_in<T>(
    store: &mut Store<T>,
    malloc: &TypedFunc<i32, i32>,
    memory: Memory,
    bytes: &[u8],
) -> Result<i32, EngineHostError> {
    let ptr = malloc
        .call(&mut *store, bytes.len() as i32)
        .map_err(wasm_err)?;
    let data = memory.data_mut(&mut *store);
    let start = ptr as usize;
    data[start..start + bytes.len()].copy_from_slice(bytes);
    Ok(ptr)
}

fn copy_out<T>(store: &Store<T>, memory: Memory, ptr: i32, len: i32) -> Vec<u8> {
    let data = memory.data(store);
    let start = ptr as usize;
    let end = start + len as usize;
    data[start..end].to_vec()
}
