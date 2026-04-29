//! Worker-internal `tracing` subscriber.
//!
//! Emits JSON-lines on stderr (no timestamps — host adds parent span
//! context on replay; worker output stays deterministic). The host
//! reads the bytes back from the WASI stderr pipe after each invoke
//! and re-emits them under the live `worker.invoke` span via
//! `replay_worker_logs`.
//!
//! Filter directive comes from the `AKUA_WORKER_LOG` env var injected
//! by the host's `WasiCtxBuilder.env(...)`. Defaults silence
//! transitive crates (rustc_span, salsa, kcl_*) that would otherwise
//! flood debug runs; only `akua*` targets honor the user's level.

use std::io;
use std::sync::OnceLock;

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static INIT: OnceLock<()> = OnceLock::new();

/// Initialize the worker subscriber. Called once at the top of `main()`.
/// Idempotent — repeat calls are no-ops.
pub fn init() {
    INIT.get_or_init(|| {
        let directive = std::env::var("AKUA_WORKER_LOG")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "warn,akua=info,akua_core=info,akua_render_worker=info".to_string());
        let filter = EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new("warn"));
        let layer = fmt::layer()
            .json()
            .with_writer(io::stderr)
            .with_timer(())
            .with_current_span(true)
            .with_span_list(false);
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init();
    });
}
