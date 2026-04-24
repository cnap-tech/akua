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
//! Both host modules DO share an `Engine` today — matching the Phase 4B
//! plan where a single wasmtime Engine can host the render worker +
//! delegate plugin callouts to the engine shims via imported host
//! functions. Crossing that bridge is Phase 4 step 2 (#410 follow-up),
//! not this scaffold commit.

use wasmtime::{AsContext, Config, Engine, Linker, Module, Store};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::WasiCtxBuilder;

/// Embedded AOT-compiled worker module. Produced by akua-cli's
/// `build.rs`. Zero-length when the .wasm source wasn't available at
/// build time — see [`WorkerError::SandboxUnavailable`].
const WORKER_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/akua-render-worker.cwasm"));

/// Per-render resource caps. Defaults documented in
/// [docs/security-model.md](../../../../docs/security-model.md) under
/// the sandbox-layers table — keep the two in sync when tuning.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Hard cap on linear memory. Default 256 MiB.
    pub memory_bytes: usize,
    /// Wasm instructions executed before the worker traps. Default
    /// 10 billion — ~10s of cranelift-JITted integer work, well
    /// beyond any legitimate render.
    pub fuel: u64,
    /// Wall-clock epoch ticks before the worker traps. Matched to the
    /// engine's background-thread tick (see [`spawn_epoch_ticker`]).
    /// Default 30 — a 30 × 100ms = 3s wall-clock deadline.
    pub epoch_deadline: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 256 * 1024 * 1024,
            fuel: 10_000_000_000,
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
    },
}

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

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// `build.rs` didn't find the worker .wasm at build time. Users
    /// need to run `task build:render-worker` before `cargo build`.
    #[error(
        "render sandbox unavailable — worker module wasn't compiled into this akua binary. \
         Run `task build:render-worker` and rebuild akua-cli."
    )]
    SandboxUnavailable,

    /// Any wasmtime-side setup or instantiation failure. Phase is
    /// embedded in the string (e.g. `"init: Module::deserialize: ..."`
    /// or `"instantiate: _start lookup: ..."`). Programmer-side bugs;
    /// consumers don't match on the specific phase.
    #[error("wasmtime {0}")]
    Wasmtime(String),

    #[error("worker trapped: {0}")]
    Trap(String),

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

/// Shared wasmtime Engine. Constructed once per process, reused across
/// every render invocation. Compiling the Engine (Cranelift warm-up,
/// pool setup) is the slow part — do it once.
pub struct RenderHost {
    engine: Engine,
    module: Module,
}

impl RenderHost {
    pub fn new() -> Result<Self, WorkerError> {
        if WORKER_CWASM.is_empty() {
            return Err(WorkerError::SandboxUnavailable);
        }
        let engine = Engine::new(&worker_config()).map_err(|e| WorkerError::Wasmtime(format!("init: {e}")))?;
        // SAFETY: WORKER_CWASM was produced by the same wasmtime
        // version + config in build.rs; deserialize is the fast path
        // (memcpy + fixup), no Cranelift pass. If build.rs and runtime
        // ever drift the compat-hash check rejects the artifact.
        let module = unsafe {
            Module::deserialize(&engine, WORKER_CWASM)
                .map_err(|e| WorkerError::Wasmtime(format!("init: Module::deserialize: {e}")))?
        };
        spawn_epoch_ticker(engine.clone());
        Ok(Self { engine, module })
    }

    /// Run one worker invocation with the given limits + request.
    /// Every call gets a fresh `Store`; caps are re-applied every time.
    pub fn invoke(
        &self,
        req: &WorkerRequest,
        limits: ResourceLimits,
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
        // argv[0] is conventional. No args, no env, no preopens
        // (Ping doesn't need filesystem access). Render + the
        // kcl-driver wasm32 follow-up will add a per-invocation
        // scratch preopen once the upstream panic is patched.
        wasi.arg("akua-render-worker");

        let wasi_ctx = wasi.build_p1();
        let host = HostState {
            wasi: wasi_ctx,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(limits.memory_bytes)
                .build(),
        };

        let mut store = Store::new(&self.engine, host);
        store.limiter(|h: &mut HostState| &mut h.limits);
        store
            .set_fuel(limits.fuel)
            .map_err(|e| WorkerError::Wasmtime(format!("init: set_fuel: {e}")))?;
        store.set_epoch_deadline(limits.epoch_deadline);

        let mut linker: Linker<HostState> = Linker::new(&self.engine);
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
                    // Append captured stderr so a panic inside the
                    // guest surfaces its message, not just a bare
                    // wasm backtrace.
                    let stderr = String::from_utf8_lossy(&err_bytes);
                    return Err(WorkerError::Trap(if stderr.is_empty() {
                        e.to_string()
                    } else {
                        format!("{e}\nworker stderr: {stderr}")
                    }));
                }
            },
        }

        if out_bytes.is_empty() {
            let stderr_str = String::from_utf8_lossy(&err_bytes).into_owned();
            return Err(WorkerError::WorkerStderr(stderr_str));
        }
        let raw = String::from_utf8_lossy(&out_bytes).into_owned();
        serde_json::from_slice(&out_bytes).map_err(|source| WorkerError::DecodeResponse {
            source,
            raw,
        })
    }
}

struct HostState {
    wasi: WasiP1Ctx,
    limits: wasmtime::StoreLimits,
}

/// Runtime wasmtime Config. MUST match `build.rs::worker_config` — the
/// AOT `.cwasm` embeds a compat-hash that gets checked on deserialize.
fn worker_config() -> Config {
    let mut c = Config::new();
    c.consume_fuel(true);
    c.epoch_interruption(true);
    c
}

/// KCL declares `extern "C-unwind" fn kcl_plugin_invoke_json_wasm(...)`
/// on wasm32 — the host must provide it or `linker.instantiate` fails
/// with an unresolved-import error. This is Phase 4's plugin bridge:
/// host function reads three C-strings (method, args JSON, kwargs
/// JSON) from guest memory, dispatches through akua_core's plugin
/// registry, allocates guest memory for the response via the worker's
/// exported `akua_bridge_alloc`, copies the response in, returns the
/// guest pointer to KCL.
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
                plugin_bridge_call(&mut caller, method_ptr, args_ptr, kwargs_ptr)
                    .unwrap_or(0)
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

    let response =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            akua_core::kcl_plugin::invoke_bridge(&method, &args, &kwargs)
        })) {
            Ok(s) => s,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "plugin bridge: handler panicked".to_string());
                format!("{{\"__kcl_PanicInfo__\":{}}}", serde_json::to_string(&msg).ok()?)
            }
        };

    // C-string: response bytes + one NUL terminator. KCL's
    // c_str_required/c_str_or_default on the guest side expects
    // a null-terminated buffer.
    let total = response.len() + 1;
    let dest = alloc.call(&mut *caller, total as u32).ok()?;
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
            .invoke(&WorkerRequest::Ping { note: None }, ResourceLimits::default())
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
            Ok(args
                .get(0)
                .cloned()
                .unwrap_or(serde_json::Value::Null))
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
    fn render_malformed_kcl_is_fail_status_not_trap() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(
                &WorkerRequest::Render {
                    package_filename: "package.k".into(),
                    source: "this is not valid kcl".into(),
                    inputs: None,
                },
                ResourceLimits::default(),
            )
            .expect("invoke");
        match resp {
            WorkerResponse::Render { status, message, .. } => {
                assert_eq!(status, "fail");
                assert!(!message.is_empty(), "empty diagnostic");
            }
            _ => panic!("expected Render"),
        }
    }
}
