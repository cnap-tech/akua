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
    // Trap symbolication. Without these, traps surface as
    // `wasm function 9837` indices; with them, plus the worker's
    // wasm `name` section preserved (see akua-render-worker's
    // build profile), `Trap::backtrace()` returns FrameInfo entries
    // whose `func_name()` resolves through the AOT address map.
    // Backtrace capture is on by default (max 20 frames); these two
    // turn on file/line resolution + the wasm-PC → name map.
    config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Enable);
    config.generate_address_map(true);
    config
}

/// Back-compat alias — kept so existing engine crates' `build.rs`
/// callers continue to compile. New callers should use
/// [`shared_config`] directly.
#[deprecated(note = "use shared_config — same Config, clearer name")]
pub fn engine_config() -> Config {
    shared_config()
}

/// Build-script variant of [`shared_config`]. When `cargo` is cross-
/// compiling (`TARGET != HOST`), tell wasmtime's Cranelift backend to
/// AOT for the binary's target arch instead of the build host's.
///
/// Without this, the macos-latest runner (now aarch64) produces a
/// cwasm baked for aarch64 inside an x86_64-apple-darwin binary, and
/// runtime `Module::deserialize` rejects it: `Module was compiled for
/// architecture 'aarch64'`. Same trap applies to any future cross-
/// compile combination.
pub fn build_script_config() -> Config {
    let mut config = shared_config();
    let target = std::env::var("TARGET").ok();
    let host = std::env::var("HOST").ok();
    if let (Some(t), Some(h)) = (target.as_deref(), host.as_deref()) {
        if t != h {
            // Cargo's TARGET is a target-triple in target_lexicon
            // shape — wasmtime's Config::target() takes the same
            // shape directly.
            config
                .target(t)
                .expect("wasmtime: Config::target rejected cargo TARGET triple");
        }
    }
    config
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
/// instead of a full Cranelift compile. Uses [`build_script_config`]
/// so cross-compiled cwasms target the binary's arch, not the build
/// host's.
pub fn precompile(wasm: &[u8]) -> Result<Vec<u8>, String> {
    let engine = Engine::new(&build_script_config()).map_err(|e| e.to_string())?;
    engine.precompile_module(wasm).map_err(|e| e.to_string())
}

/// Standard `build.rs` entry for an engine shim crate. Reads
/// `assets/<name>-engine.wasm` from the calling crate's
/// `CARGO_MANIFEST_DIR`, stages both the source `.wasm` and (when
/// the calling crate's `precompile` feature is on) the AOT-compiled
/// `.cwasm` into `OUT_DIR` so the crate's `lib.rs` can pick one via
/// `cfg(feature = "precompile")`.
///
pub fn build_engine_wasm(name: &str) {
    use std::path::PathBuf;
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set in build.rs");
    let wasm_path = PathBuf::from(&manifest_dir)
        .join("assets")
        .join(format!("{name}.wasm"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let cwasm_dest = out_dir.join(format!("{name}.cwasm"));
    let wasm_dest = out_dir.join(format!("{name}.wasm"));

    println!("cargo:rerun-if-changed={}", wasm_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    if !wasm_path.is_file() {
        println!(
            "cargo:warning={name}.wasm missing at {} — crate builds with a 0-byte placeholder. Run `task build:{name}-wasm` to produce the real artifact.",
            wasm_path.display()
        );
        std::fs::write(&cwasm_dest, []).expect("write empty cwasm placeholder");
        std::fs::write(&wasm_dest, []).expect("write empty wasm placeholder");
        return;
    }

    let wasm =
        std::fs::read(&wasm_path).unwrap_or_else(|e| panic!("read {}: {e}", wasm_path.display()));
    // Stage source `.wasm` regardless of feature — `lib.rs` picks
    // the right path via `cfg(feature = "precompile")` but the
    // unused `include_bytes!` slot still has to exist (cargo
    // doesn't propagate cfg to `include_bytes!` source-existence
    // checks).
    std::fs::write(&wasm_dest, &wasm).expect("stage source wasm");

    // Build-script cfgs work via `CARGO_FEATURE_<NAME>` env vars, not
    // `cfg!()` (build.rs is a separate compilation unit with its own
    // feature set). The shared helper is invoked from each engine's
    // `build.rs`, so the calling crate's `precompile` feature
    // controls behavior here.
    if std::env::var_os("CARGO_FEATURE_PRECOMPILE").is_some() {
        let cwasm = precompile(&wasm).unwrap_or_else(|e| panic!("precompile {name}: {e}"));
        std::fs::write(&cwasm_dest, &cwasm).expect("write cwasm");
        println!(
            "cargo:warning=precompiled {name}.wasm ({} MB) -> {} MB cwasm",
            wasm.len() / 1_048_576,
            cwasm.len() / 1_048_576
        );
    } else {
        std::fs::write(&cwasm_dest, []).expect("write empty cwasm slot");
        println!(
            "cargo:warning=precompile feature OFF — embedding source {name}.wasm ({} MB); wasmtime JIT-compiles at first call",
            wasm.len() / 1_048_576
        );
    }
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
        Self::init_with(cwasm, spec, true)
    }

    /// Instantiate the engine from a source `.wasm` slice — wasmtime
    /// JIT-compiles at first call. Used by build modes that ship the
    /// 3x-smaller source `.wasm` instead of the AOT'd `.cwasm` (e.g.
    /// `@akua-dev/sdk`'s npm distribution). Cold init pays the compile
    /// cost (~5–10s for helm-engine); subsequent renders are
    /// engine-call latency only.
    pub fn init_from_wasm(wasm: &[u8], spec: EngineSpec) -> Result<Self, EngineHostError> {
        Self::init_with(wasm, spec, false)
    }

    fn init_with(
        bytes: &[u8],
        spec: EngineSpec,
        precompiled: bool,
    ) -> Result<Self, EngineHostError> {
        if bytes.is_empty() {
            return Err(EngineHostError::Engine(format!(
                "{}.wasm not built. Run `task build:{}-wasm` to produce the Go→wasip1 artifact, then rebuild.",
                spec.name, spec.name
            )));
        }

        // Single process-wide Engine — helm, kustomize, the render
        // worker and every future engine share it. See `shared_engine`
        // doc for the why.
        let engine = shared_engine();
        // SAFETY (precompiled path): `cwasm` was produced by
        // `precompile()` against the same `shared_config()` shape and
        // embedded at compile time. Tampering requires tampering with
        // the akua binary itself.
        let module = if precompiled {
            unsafe { Module::deserialize(engine, bytes) }.map_err(wasm_err)?
        } else {
            Module::new(engine, bytes).map_err(wasm_err)?
        };

        let wasi = WasiCtxBuilder::new().arg(spec.name).build_p1();
        let mut store = Store::new(engine, wasi);
        // Shared Engine has `epoch_interruption` enabled (for the
        // render worker's wall-clock cap). Engine plugins (helm,
        // kustomize) don't want a deadline — the host-Rust caller
        // above us owns their whole-call timeouts. Pick a ceiling
        // high enough that the ticker never trips their Store: the
        // `set_epoch_deadline(delta)` API internally does
        // `current_epoch + delta`, so `u64::MAX` overflows once the
        // ticker has advanced past zero. `i64::MAX` (2^63-1) at the
        // 100ms ticker rate is ~29 billion years — comfortably
        // effectively-infinite.
        store.set_epoch_deadline(i64::MAX as u64);
        let mut linker: Linker<WasiP1Ctx> = Linker::new(engine);
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
    thread_local_call_with(slot, cwasm, spec, input, true)
}

/// `thread_local_call` variant that picks the JIT or AOT init path
/// based on `precompiled`. Allows engine shim crates to ship source
/// `.wasm` (smaller binary, JIT at first call) or AOT `.cwasm`
/// (larger binary, instant load) without parallel call sites.
pub fn thread_local_call_with(
    slot: &SessionSlot,
    bytes: &[u8],
    spec: EngineSpec,
    input: &[u8],
    precompiled: bool,
) -> Result<Vec<u8>, EngineHostError> {
    let mut borrow = slot.borrow_mut();
    if borrow.is_none() {
        *borrow = Some(if precompiled {
            Session::init(bytes, spec)?
        } else {
            Session::init_from_wasm(bytes, spec)?
        });
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
