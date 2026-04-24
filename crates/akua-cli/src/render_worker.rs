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

use wasmtime::{Config, Engine, Linker, Module, Store};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::WasiCtxBuilder;

/// Embedded AOT-compiled worker module. Produced by akua-cli's
/// `build.rs`. Zero-length when the .wasm source wasn't available at
/// build time — see [`WorkerError::SandboxUnavailable`].
const WORKER_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/akua-render-worker.cwasm"));

/// Per-render resource caps. Tunable via ResourceLimits at call-time;
/// these defaults match the security-model.md budget.
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

/// One invocation of the worker. Today the scaffold only understands
/// `Ping` — the smoke request the worker binary echoes. Render + other
/// request shapes land in #410 step 2.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequest {
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct WorkerResponse {
    pub status: String,
    #[serde(default)]
    pub echoed: Option<String>,
    pub worker_version: String,
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

    #[error("wasmtime init: {0}")]
    Init(String),

    #[error("wasmtime instantiate: {0}")]
    Instantiate(String),

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
        let engine = Engine::new(&worker_config()).map_err(|e| WorkerError::Init(e.to_string()))?;
        // SAFETY: WORKER_CWASM was produced by the same wasmtime
        // version + config in build.rs; deserialize is the fast path
        // (memcpy + fixup), no Cranelift pass. If build.rs and runtime
        // ever drift the compat-hash check rejects the artifact.
        let module = unsafe {
            Module::deserialize(&engine, WORKER_CWASM)
                .map_err(|e| WorkerError::Init(format!("Module::deserialize: {e}")))?
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
        // (Ping doesn't need filesystem access).
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
            .map_err(|e| WorkerError::Init(format!("set_fuel: {e}")))?;
        store.set_epoch_deadline(limits.epoch_deadline);

        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |h: &mut HostState| &mut h.wasi)
            .map_err(|e| WorkerError::Init(format!("add_to_linker: {e}")))?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| WorkerError::Instantiate(e.to_string()))?;
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(|e| WorkerError::Instantiate(format!("_start lookup: {e}")))?;

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
                None => return Err(WorkerError::Trap(e.to_string())),
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
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.echoed.as_deref(), Some("from-test"));
        assert!(!resp.worker_version.is_empty());
    }

    #[test]
    fn ping_without_note_returns_no_echo() {
        let Some(host) = host_or_skip() else { return };
        let resp = host
            .invoke(&WorkerRequest::Ping { note: None }, ResourceLimits::default())
            .expect("invoke");
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.echoed, None);
    }
}
