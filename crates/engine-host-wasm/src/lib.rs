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

// --- Shared Engine ---------------------------------------------------------

/// The single `Config` every akua-side wasmtime Engine uses. One
/// process-global Engine built from this Config hosts the render
/// worker + every engine plugin (helm, kustomize, future kro/CEL).
/// Stores are still per-invocation with their own memory / epoch
/// budgets — per the wasmtime docs' "one Engine, many Stores"
/// pattern (see `docs/spikes/wasmtime-multi-engine.md`). Nested
/// Engines trip process-global trap-handler TLS; avoid.
///
/// Build-time + runtime MUST call the same function or
/// `Module::deserialize` rejects precompiled artefacts on a
/// Config-hash mismatch.
pub fn shared_config() -> Config {
    let mut config = Config::new();
    config.wasm_exceptions(true);
    // Wall-clock deadline enforcement. The render worker sets an
    // epoch deadline per-Store; engine plugins (helm, kustomize)
    // don't set one, so they run without a tick-level cap — the
    // host Rust caller enforces whole-call timeouts above them.
    config.epoch_interruption(true);
    config
}

/// Back-compat alias — kept so existing engine crates' `build.rs`
/// callers continue to compile. New callers should use
/// [`shared_config`] directly.
#[deprecated(note = "use shared_config — same Config, clearer name")]
pub fn engine_config() -> Config {
    shared_config()
}

/// The single Engine shared across every akua-side wasmtime
/// invocation. Lazy-initialized on first call; thereafter reused
/// for the life of the process. `Engine::clone` is cheap and
/// intended for sharing.
pub fn shared_engine() -> &'static Engine {
    use std::sync::OnceLock;
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| Engine::new(&shared_config()).expect("shared engine init"))
}

// --- Build-time: precompile wasm → cwasm -----------------------------------

/// Called from each engine crate's `build.rs`. Precompiles a `.wasm`
/// to a platform-specific `.cwasm`; deserialize at runtime is a fixup
/// instead of a full Cranelift compile.
pub fn precompile(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let engine = Engine::new(&shared_config()).map_err(|e| e.to_string())?;
    engine.precompile_module(wasm).map_err(|e| e.to_string())
}

// --- Engine spec -----------------------------------------------------------

/// Engine identity: the symbol prefix its wasip1 module exports its
/// malloc/free/result_len functions under, plus the entry-point
/// function name. By convention (matches the Go-side ABI shared by
/// helm-engine-wasm + kustomize-engine-wasm), the three allocator
/// exports are `<prefix>_malloc`, `<prefix>_free`, `<prefix>_result_len`.
/// `name` doubles as the WASI argv[0] and as the diagnostic tag in
/// error messages.
#[derive(Debug, Clone, Copy)]
pub struct EngineSpec {
    /// Human-readable name. Used for error messages + `argv[0]`.
    pub name: &'static str,
    /// Symbol prefix for allocator exports (`<prefix>_malloc` etc.).
    pub prefix: &'static str,
    /// Export symbol of the entry-point function: `helm_render` / `kustomize_build` / etc.
    pub entry: &'static str,
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

        // Single process-wide Engine — helm, kustomize, the render
        // worker and every future engine share it. See `shared_engine`
        // doc for the why.
        let engine = shared_engine();
        // SAFETY: `cwasm` was produced by `precompile()` against the
        // same `shared_config()` shape. Embedded at compile time, so
        // tampering requires tampering with the akua binary itself.
        let module = unsafe { Module::deserialize(&engine, cwasm) }.map_err(wasm_err)?;

        let wasi = WasiCtxBuilder::new().arg(spec.name).build_p1();
        let mut store = Store::new(&engine, wasi);
        // Shared Engine has `epoch_interruption` enabled (for the
        // render worker's wall-clock cap). Engine plugins (helm,
        // kustomize) don't want a deadline — the host-Rust caller
        // above us owns their whole-call timeouts. Set the highest
        // deadline so the epoch-ticker never trips their Store.
        store.set_epoch_deadline(u64::MAX);
        let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
        p1::add_to_linker_sync(&mut linker, |s: &mut WasiP1Ctx| s).map_err(wasm_err)?;

        let instance = linker.instantiate(&mut store, &module).map_err(wasm_err)?;
        // Reactor module: `_initialize` runs Go runtime + package init
        // chains (klog, sprig, helm/kustomize inits). Exports callable
        // after. Runs once per thread here.
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            init.call(&mut store, ()).map_err(wasm_err)?;
        }

        let malloc_name = format!("{}_malloc", spec.prefix);
        let free_name = format!("{}_free", spec.prefix);
        let result_len_name = format!("{}_result_len", spec.prefix);

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| wasm_err(format!("{}.wasm missing `memory` export", spec.name)))?;
        let malloc = instance
            .get_typed_func::<i32, i32>(&mut store, &malloc_name)
            .map_err(wasm_err)?;
        let free = instance
            .get_typed_func::<i32, ()>(&mut store, &free_name)
            .map_err(wasm_err)?;
        let entry = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, spec.entry)
            .map_err(wasm_err)?;
        let result_len = instance
            .get_typed_func::<i32, i32>(&mut store, &result_len_name)
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

/// Lazily initialize + invoke `SESSION` on the caller's thread.
/// Plugin crates wrap the returned [`EngineHostError`] in their own
/// typed error via `#[from]` at the call site.
pub fn thread_local_call(
    slot: &SessionSlot,
    cwasm: &[u8],
    spec: EngineSpec,
    input: &[u8],
) -> Result<Vec<u8>, EngineHostError> {
    let mut borrow = slot.borrow_mut();
    if borrow.is_none() {
        *borrow = Some(Session::init(cwasm, spec)?);
    }
    borrow.as_mut().expect("just initialized").call(input)
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
