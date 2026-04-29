//! Host for the `akua-render-worker` wasip1 module — the per-render
//! sandbox that delivers CLAUDE.md's "Sandboxed by default" invariant
//! at the process level.
//!
//! Each render invocation:
//!
//! 1. Builds a fresh `Store<HostState>` with `StoreLimitsBuilder`
//!    (memory cap, instance/table caps), `consume_fuel`, and an
//!    epoch deadline checked by a background-thread tick.
//! 2. Constructs a `WasiCtx` with **only** the preopens the request
//!    declared — workspace dir read-only, output dir writable, nothing
//!    else reachable. No inherited env, no inherited stdio beyond the
//!    JSON envelope pipes.
//! 3. Instantiates the worker module (deserialized once from the
//!    AOT `.cwasm`; reused across all `Store`s of the same `Engine`).
//! 4. Pipes a JSON request into the worker's stdin, invokes its
//!    `_start`, reads stdout for the response, decodes, returns to
//!    the calling verb.
//!
//! ## Why this crate and not `engine-host-wasm`?
//!
//! `engine-host-wasm` hosts the engine shims (helm, kustomize) —
//! memory-only C-ABI, no preopens, persistent thread-local `Session`.
//! The render worker wants full WASI (preopens + stdio pipes) and
//! per-render Stores. Same wasmtime version (43), different posture.
//!
//! Both host modules DO share an `Engine` today: a single wasmtime
//! Engine can host the render worker and delegate plugin callouts
//! to the engine shims via imported host functions.

use wasmtime::{AsContext, Engine, Linker, Module, Store};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::WasiCtxBuilder;

/// Embedded worker module. AOT `.cwasm` when `precompile-engines`
/// is on (default), source `.wasm` otherwise — wasmtime JIT-compiles
/// the second form at first call. The smaller-binary mode is used
/// by the napi distribution. Zero-length when the source `.wasm`
/// wasn't available at build time — see
/// [`WorkerError::SandboxUnavailable`].
#[cfg(feature = "precompile-engines")]
const WORKER_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/akua-render-worker.cwasm"));
#[cfg(not(feature = "precompile-engines"))]
const WORKER_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/akua-render-worker.wasm"));
const WORKER_PRECOMPILED: bool = cfg!(feature = "precompile-engines");

/// Per-render resource caps. Defaults documented in
/// [docs/security-model.md](../../../../docs/security-model.md) under
/// the sandbox-layers table — keep the two in sync when tuning.
///
/// Note: the shared wasmtime Engine today does not enable
/// `Config::consume_fuel` (would force every plugin-Engine caller
/// —helm, kustomize, future kro — to set_fuel before every call;
/// not worth the coupling for v0.1.0). Wall-clock deadline via
/// `epoch_interruption` is the active CPU cap. Fuel support can
/// flip back on later without breaking the ABI — it's Engine-level,
/// and only the worker Store would call `set_fuel`.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Hard cap on linear memory. Default 256 MiB.
    pub memory_bytes: usize,
    /// Wall-clock epoch ticks before the worker traps. Matched to the
    /// engine's background-thread tick (see [`spawn_epoch_ticker`]).
    /// Default 30 — a 30 × 100ms = 3s wall-clock deadline.
    pub epoch_deadline: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 256 * 1024 * 1024,
            epoch_deadline: 30,
        }
    }
}

/// One invocation of the worker. Kept in sync with the request
/// protocol in `crates/akua-render-worker/src/main.rs` — serialize
/// shape must match the worker's `Deserialize` shape exactly.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequest {
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Render {
        #[serde(default)]
        package_filename: String,
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        inputs: Option<serde_json::Value>,
        /// Guest-visible path to the preopened `charts` pkg dir.
        /// Set by [`RenderHost::invoke_with_deps`]; omit for
        /// Packages that don't `import charts.*`.
        #[serde(skip_serializing_if = "Option::is_none")]
        charts_pkg_path: Option<String>,
        /// Upstream KCL ecosystem deps the host has preopened: alias
        /// → guest-visible path. Empty when the Package has no
        /// KCL-OCI deps. Wire-compat with `akua-render-worker`.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        kcl_pkgs: std::collections::BTreeMap<String, String>,
    },
}

/// Success marker the worker sets on the protocol's `status` field.
/// Protocol contract with `akua-render-worker`; any other string is
/// treated as failure.
pub const WORKER_STATUS_OK: &str = "ok";

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerResponse {
    Ping {
        status: String,
        #[serde(default)]
        echoed: Option<String>,
        worker_version: String,
    },
    Render {
        status: String,
        yaml: String,
        #[serde(default)]
        message: String,
        worker_version: String,
    },
}

impl WorkerResponse {
    /// Unwrap a successful `Render` response into its YAML payload.
    /// `Render { status != "ok" }` yields the diagnostic message as
    /// an `Err`; a `Ping` response is a wire-protocol bug (caller
    /// sent a `Render` request) and likewise surfaces as `Err`.
    pub fn into_render_yaml(self) -> Result<String, String> {
        match self {
            WorkerResponse::Render {
                status,
                yaml,
                message,
                ..
            } => {
                if status == WORKER_STATUS_OK {
                    Ok(yaml)
                } else {
                    Err(message)
                }
            }
            WorkerResponse::Ping { .. } => {
                Err("render worker returned unexpected ping response".into())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// `build.rs` didn't find the worker .wasm at build time. Users
    /// need to run `task build:render-worker` before `cargo build`.
    #[error(
        "render sandbox unavailable — worker module wasn't compiled into this akua binary. \
         Run `task build:render-worker` and rebuild akua-cli."
    )]
    SandboxUnavailable,

    /// Any wasmtime-side setup or instantiation failure. The stage
    /// is embedded in the string (e.g. `"init: Module::deserialize:
    /// ..."` or `"instantiate: _start lookup: ..."`). Programmer-side
    /// bugs; consumers don't match on the specific stage.
    #[error("wasmtime {0}")]
    Wasmtime(String),

    #[error("worker trapped: {0}")]
    Trap(String),

    /// KCL plugin handler panicked; message preserved out-of-band
    /// around the wasip1 trap boundary so callers can classify the
    /// diagnostic (e.g. strict-mode chart rejections).
    #[error("plugin panic: {0}")]
    PluginPanic(String),

    #[error("encode request: {0}")]
    EncodeRequest(#[source] serde_json::Error),

    #[error("decode response: {source} — raw: {raw:?}")]
    DecodeResponse {
        #[source]
        source: serde_json::Error,
        raw: String,
    },

    #[error("worker stderr: {0}")]
    WorkerStderr(String),
}

/// Handle to the shared wasmtime Engine's precompiled worker module.
/// Construction is cheap after first call — the Engine itself is a
/// process-wide singleton owned by `engine-host-wasm::shared_engine`;
/// `RenderHost` just holds the deserialized Module so every
/// invocation skips the Cranelift pass.
pub struct RenderHost {
    engine: &'static Engine,
    module: Module,
}

impl RenderHost {
    /// Process-wide cached host. `RenderHost::new` does a multi-MB
    /// `Module::deserialize` memcpy of the embedded cwasm — in
    /// `akua dev`'s watch loop that adds up. Init once on first
    /// success, reuse forever. Init failures are not cached (they
    /// typically reflect a broken build and the caller may want
    /// to retry after rebuilding).
    pub fn shared() -> Result<&'static Self, WorkerError> {
        static HOST: std::sync::OnceLock<RenderHost> = std::sync::OnceLock::new();
        if let Some(host) = HOST.get() {
            return Ok(host);
        }
        let host = RenderHost::new()?;
        Ok(HOST.get_or_init(|| host))
    }

    pub fn new() -> Result<Self, WorkerError> {
        if WORKER_BYTES.is_empty() {
            return Err(WorkerError::SandboxUnavailable);
        }
        // Install the host-side plugin handlers so bridge callouts
        // from sandboxed KCL (`helm.template`, `kustomize.build`,
        // `pkg.render`, etc.) resolve to the engine-host-wasm-backed
        // implementations. Engines run in SEPARATE Stores of the
        // SAME Engine — per wasmtime docs ("one Engine, many
        // Stores"). Nested Engines trip process-global trap-handler
        // TLS and are not tested in wasmtime's suite; the unified
        // Engine is.
        //
        // Idempotent — safe to call once per RenderHost construction.
        akua_core::kcl_plugin::install_builtin_plugins();

        let engine = engine_host_wasm::shared_engine();
        // SAFETY: WORKER_BYTES was produced by the same wasmtime
        // version + `engine_host_wasm::shared_config()` shape in
        // build.rs; deserialize is the fast path (memcpy + fixup),
        // no Cranelift pass. Config-hash drift between build + run
        // is the only failure mode and is caught by
        // `Module::deserialize`.
        // SAFETY (precompiled path): WORKER_BYTES is a `.cwasm`
        // produced by build.rs against the same `shared_config()`
        // shape, embedded at compile time. Tampering requires
        // tampering with the akua binary itself.
        let module = if WORKER_PRECOMPILED {
            unsafe {
                Module::deserialize(engine, WORKER_BYTES)
                    .map_err(|e| WorkerError::Wasmtime(format!("init: Module::deserialize: {e}")))?
            }
        } else {
            // JIT path — wasmtime compiles at first call. ~5–10s on
            // the worker .wasm; subsequent renders share the Module.
            Module::new(engine, WORKER_BYTES)
                .map_err(|e| WorkerError::Wasmtime(format!("init: Module::new (JIT): {e}")))?
        };
        spawn_epoch_ticker(engine.clone());
        Ok(Self { engine, module })
    }

    /// Run one worker invocation with the given limits + request.
    /// Every call gets a fresh `Store`; caps are re-applied every time.
    /// No `charts.*` or KCL-pkg preopens — use
    /// [`invoke_with_deps`](Self::invoke_with_deps) for Packages that
    /// `import charts.*` or pull in upstream KCL ecosystem packages.
    pub fn invoke(
        &self,
        req: &WorkerRequest,
        limits: ResourceLimits,
    ) -> Result<WorkerResponse, WorkerError> {
        self.invoke_inner(req, limits, None, &[])
    }

    /// As [`invoke`](Self::invoke), but preopens any extra dirs the
    /// Package's deps need:
    ///
    /// - `charts_host_dir` (when set): the synthesized
    ///   [`akua_core::stdlib::materialize_charts`] tempdir, mounted
    ///   read-only at the guest path `/charts`. The Render request's
    ///   `charts_pkg_path` must be `Some("/charts".into())` for the
    ///   guest's evaluator to see the mount.
    /// - `kcl_pkgs`: each entry is `(host_dir, guest_path)` for one
    ///   upstream KCL ecosystem package. The Render request's
    ///   `kcl_pkgs` map carries `alias → guest_path` so the worker
    ///   registers a matching `ExternalPkg` per dep.
    ///
    /// All preopens stay alive until this call returns; the WASI
    /// layer holds its own file-descriptor handles while the guest
    /// runs.
    pub fn invoke_with_deps(
        &self,
        req: &WorkerRequest,
        limits: ResourceLimits,
        charts_host_dir: Option<&std::path::Path>,
        kcl_pkgs: &[(std::path::PathBuf, String)],
    ) -> Result<WorkerResponse, WorkerError> {
        self.invoke_inner(req, limits, charts_host_dir, kcl_pkgs)
    }

    /// Backwards-compatible wrapper. New callers should prefer
    /// [`invoke_with_deps`](Self::invoke_with_deps).
    pub fn invoke_with_charts(
        &self,
        req: &WorkerRequest,
        limits: ResourceLimits,
        charts_host_dir: &std::path::Path,
    ) -> Result<WorkerResponse, WorkerError> {
        self.invoke_inner(req, limits, Some(charts_host_dir), &[])
    }

    fn invoke_inner(
        &self,
        req: &WorkerRequest,
        limits: ResourceLimits,
        charts_preopen: Option<&std::path::Path>,
        kcl_pkg_preopens: &[(std::path::PathBuf, String)],
    ) -> Result<WorkerResponse, WorkerError> {
        let req_bytes = serde_json::to_vec(req).map_err(WorkerError::EncodeRequest)?;

        // WASI pipes as owned handles we can read back after the
        // guest finishes writing.
        let stdin_pipe = MemoryInputPipe::new(req_bytes);
        let stdout_pipe = MemoryOutputPipe::new(1 << 20); // 1 MiB cap on response
        let stderr_pipe = MemoryOutputPipe::new(64 * 1024);

        let mut wasi = WasiCtxBuilder::new();
        wasi.stdin(stdin_pipe);
        wasi.stdout(stdout_pipe.clone());
        wasi.stderr(stderr_pipe.clone());
        wasi.arg("akua-render-worker");
        // Always-on preopen: the akua KCL stdlib (`akua.helm`,
        // `akua.kustomize`, `akua.pkg`, `akua.ctx`). Materialized on
        // the host because `std::env::temp_dir()` panics on wasip1,
        // so the guest can't produce it itself. Mounted read-only at
        // `/akua-stdlib`; the worker registers a matching `ExternalPkg`
        // so `import akua.helm` resolves through the mount.
        wasi.preopened_dir(
            akua_core::stdlib::stdlib_root(),
            "/akua-stdlib",
            wasmtime_wasi::DirPerms::READ,
            wasmtime_wasi::FilePerms::READ,
        )
        .map_err(|e| WorkerError::Wasmtime(format!("preopen akua stdlib: {e}")))?;
        // Optional preopen: a host tempdir populated by
        // `akua_core::stdlib::materialize_charts`. Mounts read-only
        // at `/charts` inside the guest; KCL's import resolver
        // reads `charts.*.k` from here when the Package does
        // `import charts.<name>`.
        if let Some(dir) = charts_preopen {
            wasi.preopened_dir(
                dir,
                "/charts",
                wasmtime_wasi::DirPerms::READ,
                wasmtime_wasi::FilePerms::READ,
            )
            .map_err(|e| WorkerError::Wasmtime(format!("preopen charts: {e}")))?;
        }
        // KCL ecosystem packages — one preopen per upstream dep at
        // its guest path. The render request's `kcl_pkgs` map names
        // each as an ExternalPkg in the worker's KCL evaluator.
        for (host_dir, guest_path) in kcl_pkg_preopens {
            wasi.preopened_dir(
                host_dir,
                guest_path,
                wasmtime_wasi::DirPerms::READ,
                wasmtime_wasi::FilePerms::READ,
            )
            .map_err(|e| WorkerError::Wasmtime(format!("preopen kcl pkg `{guest_path}`: {e}")))?;
        }

        let wasi_ctx = wasi.build_p1();
        let host = HostState {
            wasi: wasi_ctx,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(limits.memory_bytes)
                .build(),
            last_plugin_panic: None,
        };

        let mut store = Store::new(self.engine, host);
        store.limiter(|h: &mut HostState| &mut h.limits);
        store.set_epoch_deadline(limits.epoch_deadline);

        let mut linker: Linker<HostState> = Linker::new(self.engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |h: &mut HostState| &mut h.wasi)
            .map_err(|e| WorkerError::Wasmtime(format!("init: add_to_linker: {e}")))?;
        install_kcl_plugin_bridge(&mut linker);

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| WorkerError::Wasmtime(format!("instantiate: {e}")))?;
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(|e| WorkerError::Wasmtime(format!("instantiate: _start lookup: {e}")))?;

        // A successful `std::process::exit(0)` on wasip1 surfaces as
        // an `I32Exit(0)` trap, which is NOT an error. Peel it off
        // here so the stdout pipe still gets read. Non-zero exits
        // are passed through as worker-side errors.
        let trap_result = start.call(&mut store, ());
        let plugin_panic = store.data_mut().last_plugin_panic.take();
        drop(store);
        let out_bytes = stdout_pipe.contents();
        let err_bytes = stderr_pipe.contents();
        match trap_result {
            Ok(()) => {}
            Err(e) => match e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                Some(wasmtime_wasi::I32Exit(0)) => {}
                Some(wasmtime_wasi::I32Exit(code)) => {
                    return Err(WorkerError::WorkerStderr(format!(
                        "worker exited with code {code} — stderr: {}",
                        String::from_utf8_lossy(&err_bytes)
                    )));
                }
                None => {
                    // If the bridge captured a plugin panic message
                    // during this invocation, the trap is KCL
                    // panicking on `__kcl_PanicInfo__`. Surface the
                    // typed diagnostic; callers need it to classify
                    // errors (e.g. strict-mode chart paths).
                    if let Some(msg) = plugin_panic {
                        return Err(WorkerError::PluginPanic(msg));
                    }
                    // Otherwise format with anyhow's Debug repr to
                    // walk the error chain + symbolicated backtrace
                    // (wasmtime attaches `WasmBacktrace` as a
                    // context that renders under `:?` only). Frame
                    // names resolve through the AOT address map
                    // enabled in `engine_host_wasm::shared_config`
                    // when the worker `.wasm` carries its `name`
                    // section (preserved via the build profile in
                    // Taskfile.yml::build:render-worker).
                    let stderr = String::from_utf8_lossy(&err_bytes);
                    let mut msg = format!("{e:?}");
                    if !stderr.is_empty() {
                        msg.push_str("\n\nworker stderr:\n");
                        msg.push_str(&stderr);
                    }
                    return Err(WorkerError::Trap(msg));
                }
            },
        }

        if out_bytes.is_empty() {
            let stderr_str = String::from_utf8_lossy(&err_bytes).into_owned();
            return Err(WorkerError::WorkerStderr(stderr_str));
        }
        let raw = String::from_utf8_lossy(&out_bytes).into_owned();
        serde_json::from_slice(&out_bytes)
            .map_err(|source| WorkerError::DecodeResponse { source, raw })
    }
}

struct HostState {
    wasi: WasiP1Ctx,
    limits: wasmtime::StoreLimits,
    /// First `__kcl_PanicInfo__` the bridge saw during this
    /// invocation. Out-of-band channel around KCL's wasip1 panic →
    /// wasm-trap loss. First-wins: a nested plugin callout that also
    /// panics must not clobber the outer diagnostic.
    last_plugin_panic: Option<String>,
}

/// KCL declares `extern "C-unwind" fn kcl_plugin_invoke_json_wasm(...)`
/// on wasm32 — the host must provide it or `linker.instantiate` fails
/// with an unresolved-import error. The plugin bridge: host function
/// reads three C-strings (method, args JSON, kwargs JSON) from guest
/// memory, dispatches through akua_core's plugin registry, allocates
/// guest memory for the response via the worker's exported
/// `akua_bridge_alloc`, copies the response in, returns the guest
/// pointer to KCL.
///
/// The worker-side engine bridges (helm/kustomize) live as
/// host-registered plugin handlers in akua-cli; the render worker
/// itself stays engine-free. Invariants:
///
/// - Guest never sees host addresses — every pointer is a guest
///   linear-memory offset.
/// - Panics in the plugin handler are converted to the
///   `__kcl_PanicInfo__` JSON envelope KCL already treats as a
///   runtime panic, never unwound across the wasmtime boundary.
/// - Unresolved plugin names come back as the same envelope; KCL
///   surfaces them as standard eval diagnostics.
fn install_kcl_plugin_bridge(linker: &mut Linker<HostState>) {
    linker
        .func_wrap(
            "env",
            "kcl_plugin_invoke_json_wasm",
            |mut caller: wasmtime::Caller<'_, HostState>,
             method_ptr: i32,
             args_ptr: i32,
             kwargs_ptr: i32|
             -> i32 {
                plugin_bridge_call(&mut caller, method_ptr, args_ptr, kwargs_ptr).unwrap_or(0)
            },
        )
        .expect("install kcl_plugin_invoke_json_wasm");
}

/// Inner bridge body — fail-soft on any guest-memory access issue
/// (returns `None` → host function returns 0, KCL treats as
/// `__kcl_PanicInfo__`-style null). `None` return paths are reserved
/// for programmer-side bugs (memory handle missing, allocator export
/// missing) — plugin-handler errors take the panic-envelope path.
fn plugin_bridge_call(
    caller: &mut wasmtime::Caller<'_, HostState>,
    method_ptr: i32,
    args_ptr: i32,
    kwargs_ptr: i32,
) -> Option<i32> {
    let memory = caller.get_export("memory")?.into_memory()?;
    let alloc = caller
        .get_export("akua_bridge_alloc")?
        .into_func()?
        .typed::<u32, i32>(&mut *caller)
        .ok()?;

    let method = read_c_str_required(caller.as_context(), memory, method_ptr)?;
    let args = read_c_str_or_empty(caller.as_context(), memory, args_ptr, "[]");
    let kwargs = read_c_str_or_empty(caller.as_context(), memory, kwargs_ptr, "{}");

    // Trace enabled via `AKUA_BRIDGE_TRACE=1` for debugging plugin-
    // callout issues. Captures the whole round-trip in one emission —
    // method, handler response, and the guest pointer allocated for
    // the response bytes.
    let trace = std::env::var("AKUA_BRIDGE_TRACE").ok().as_deref() == Some("1");
    if trace {
        eprintln!("[bridge] method={method} args={args} kwargs={kwargs}");
    }

    // Two envelope sources: a handler returning `Err(...)` produces
    // one via `invoke_bridge`'s `Ok(s)` (s starts with the envelope
    // shape); a handler that panics outright lands in `Err(payload)`
    // and we know the message here without a round-trip. Both paths
    // stash the message on HostState so `invoke_inner` can promote
    // the resulting wasip1 trap to `WorkerError::PluginPanic`.
    let (response, panic_msg) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        akua_core::kcl_plugin::invoke_bridge(&method, &args, &kwargs)
    })) {
        Ok(s) => {
            let msg = akua_core::kcl_plugin::extract_panic_info(&s);
            (s, msg)
        }
        Err(payload) => {
            let msg = akua_core::kcl_plugin::panic_message(payload);
            let envelope = akua_core::kcl_plugin::panic_envelope(&msg);
            (envelope, Some(msg))
        }
    };

    if let Some(msg) = panic_msg {
        let state = caller.data_mut();
        if state.last_plugin_panic.is_none() {
            state.last_plugin_panic = Some(msg);
        }
    }

    // C-string: response bytes + one NUL terminator. KCL's
    // c_str_required/c_str_or_default on the guest side expects
    // a null-terminated buffer.
    let total = response.len() + 1;
    let dest = alloc.call(&mut *caller, total as u32).ok()?;
    if trace {
        eprintln!("[bridge] response_len={} alloc_ptr={dest}", response.len());
    }
    if dest <= 0 {
        return None;
    }
    let data = memory.data_mut(&mut *caller);
    let start = dest as usize;
    data.get_mut(start..start + response.len())?
        .copy_from_slice(response.as_bytes());
    *data.get_mut(start + response.len())? = 0;
    Some(dest)
}

fn read_c_str_required(
    store: impl wasmtime::AsContext,
    memory: wasmtime::Memory,
    ptr: i32,
) -> Option<String> {
    if ptr <= 0 {
        return None;
    }
    let data = memory.data(&store);
    let start = ptr as usize;
    let slice = data.get(start..)?;
    let end = slice.iter().position(|b| *b == 0)?;
    Some(String::from_utf8_lossy(&slice[..end]).into_owned())
}

fn read_c_str_or_empty(
    store: impl wasmtime::AsContext,
    memory: wasmtime::Memory,
    ptr: i32,
    default: &str,
) -> String {
    if ptr <= 0 {
        return default.to_string();
    }
    match read_c_str_required(store, memory, ptr) {
        Some(s) if !s.is_empty() => s,
        _ => default.to_string(),
    }
}

/// Background thread that ticks the engine's epoch at a fixed rate.
/// `Store::set_epoch_deadline(N)` then traps the guest after ~N ticks
/// of wall-clock time. 100ms per tick keeps ticker overhead negligible
/// while giving sub-second granularity on deadline enforcement.
fn spawn_epoch_ticker(engine: Engine) {
    // Idempotent: the shared Engine is a process singleton, we may
    // be called >1× as RenderHost is constructed for the first and
    // any subsequent time. Second call just spawns another ticker —
    // cheap, but let's guard with a OnceLock anyway.
    use std::sync::OnceLock;
    static TICKER: OnceLock<()> = OnceLock::new();
    if TICKER.set(()).is_err() {
        return;
    }
    let _ = engine;
    let engine = engine_host_wasm::shared_engine().clone();
    std::thread::Builder::new()
        .name("akua-epoch-ticker".to_string())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            engine.increment_epoch();
        })
        .expect("spawn epoch ticker thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skips when the sandbox wasn't compiled in. Run
    /// `task build:render-worker && cargo test -p akua-cli` to get
    /// real coverage.
    fn host_or_skip() -> Option<RenderHost> {
        match RenderHost::new() {
            Ok(h) => Some(h),
            Err(WorkerError::SandboxUnavailable) => {
                eprintln!("skipping: render worker .wasm not built");
                None
            }
            Err(e) => panic!("unexpected init error: {e}"),
        }
    }

    #[test]
    fn ping_round_trips() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(
                &WorkerRequest::Ping {
                    note: Some("from-test".into()),
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Ping {
                status,
                echoed,
                worker_version,
            } => {
                assert_eq!(status, "ok");
                assert_eq!(echoed.as_deref(), Some("from-test"));
                assert!(!worker_version.is_empty());
            }
            _ => panic!("expected Ping"),
        }
    }

    #[test]
    fn ping_without_note_returns_no_echo() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(
                &WorkerRequest::Ping { note: None },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Ping { echoed, .. } => assert_eq!(echoed, None),
            _ => panic!("expected Ping"),
        }
    }

    #[test]
    fn render_pure_kcl_returns_yaml_end_to_end() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: "x = 42\ngreeting = \"hello\"\n".into(),
                    inputs: None,
                    charts_pkg_path: None,
                    kcl_pkgs: std::collections::BTreeMap::new(),
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Render {
                status,
                yaml,
                message,
                ..
            } => {
                assert_eq!(status, "ok", "diagnostic: {message}");
                assert!(yaml.contains("x: 42"), "yaml missing x: {yaml}");
                assert!(yaml.contains("hello"), "yaml missing greeting: {yaml}");
            }
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn plugin_bridge_invokes_host_registered_handler_from_guest() {
        let Some(host) = host_or_skip() else { return };

        // Register a trivial handler host-side: "bridge_echo.say"
        // returns its first positional arg unmodified. The KCL source
        // below calls it via the `kcl_plugin.<name>` discovery shape,
        // which is exactly what engine stdlibs (helm / kustomize) use.
        akua_core::kcl_plugin::register("bridge_echo.say", |args, _kwargs| {
            Ok(args.get(0).cloned().unwrap_or(serde_json::Value::Null))
        });

        // KCL plugin invocation requires `import kcl_plugin.<pkg>`
        // before the dotted call.
        let source = "import kcl_plugin.bridge_echo\n\
                      greeting = bridge_echo.say(\"hello\")\n";
        let resp = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: source.into(),
                    inputs: None,
                    charts_pkg_path: None,
                    kcl_pkgs: std::collections::BTreeMap::new(),
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Render {
                status,
                yaml,
                message,
                ..
            } => {
                assert_eq!(status, "ok", "diagnostic: {message}");
                assert!(
                    yaml.contains("greeting: hello"),
                    "expected handler result in YAML: {yaml}"
                );
            }
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn plugin_panic_message_survives_wasip1_trap() {
        let Some(host) = host_or_skip() else { return };

        // `Err(msg)` from a handler goes through `panic_envelope(msg)`
        // in `invoke_bridge`, which produces the `__kcl_PanicInfo__`
        // JSON shape. KCL then panics on that envelope inside the
        // guest — on wasip1 the unwind surfaces as a wasm trap whose
        // message is lost. Without the bridge's out-of-band capture
        // we'd only see a backtrace; with it, the original marker
        // round-trips as `WorkerError::PluginPanic`.
        const MARKER: &str = "strict mode requires every chart to be declared in akua.toml";
        akua_core::kcl_plugin::register("strict_fail.trigger", |_args, _kwargs| {
            Err(MARKER.to_string())
        });

        let source = "import kcl_plugin.strict_fail\n\
                      _ignored = strict_fail.trigger({})\n";
        let err = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: source.into(),
                    inputs: None,
                    charts_pkg_path: None,
                    kcl_pkgs: std::collections::BTreeMap::new(),
                },
                ResourceLimits::default(),
            )
            .expect_err("handler should trap");

        match err {
            WorkerError::PluginPanic(msg) => {
                assert!(
                    msg.contains(MARKER),
                    "captured panic should carry handler marker, got: {msg}"
                );
            }
            other => panic!("expected PluginPanic, got: {other:?}"),
        }
    }

    #[test]
    fn render_inputs_reach_the_kcl_option_input() {
        let Some(host) = host_or_skip() else { return };
        // KCL's `option("input")` returns whatever the host injects
        // under the `input` argument. We pass { name: "alpha" } and
        // assert the binding round-trips into the output YAML.
        let source = "greeting = option(\"input\").name\n";
        let inputs = serde_json::json!({ "name": "alpha" });
        let resp = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: source.into(),
                    inputs: Some(inputs),
                    charts_pkg_path: None,
                    kcl_pkgs: std::collections::BTreeMap::new(),
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Render {
                status,
                yaml,
                message,
                ..
            } => {
                assert_eq!(status, "ok", "diagnostic: {message}");
                assert!(
                    yaml.contains("greeting: alpha"),
                    "inputs didn't reach KCL: {yaml}"
                );
            }
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn render_malformed_kcl_is_fail_status_not_trap() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: "this is not valid kcl".into(),
                    inputs: None,
                    charts_pkg_path: None,
                    kcl_pkgs: std::collections::BTreeMap::new(),
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Render {
                status, message, ..
            } => {
                assert_eq!(status, "fail");
                assert!(!message.is_empty(), "empty diagnostic");
            }
            _ => panic!("expected Render"),
        }
    }
}
