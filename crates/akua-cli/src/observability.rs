//! Host-side `tracing` subscriber wiring.
//!
//! One entry: [`init_subscriber`]. Called once at the top of `main()`
//! before any verb runs. Returns an [`ObservabilityHandle`] whose `Drop`
//! flushes the OpenTelemetry batch processor when the `otel` feature is
//! active.
//!
//! Filter precedence (first match wins):
//! 1. `RUST_LOG` env var — escape hatch, full `EnvFilter` syntax.
//! 2. `--log-level=…`
//! 3. `-v` / `--verbose` (forces `debug`)
//! 4. Default: `warn,akua=info,akua_cli=info,akua_core=info,akua_render_worker=info`
//!
//! The legacy `AKUA_BRIDGE_TRACE=1` shortcut is honored by OR-ing
//! `akua::bridge=debug` into the resolved filter.
//!
//! Format: text by default; `--log=json` (auto in agent context) emits
//! JSON-lines on stderr. Timestamps are dropped in JSON/agent mode so
//! `--json` runs stay byte-deterministic — the same invariant golden
//! tests rely on for stdout.

use std::io;

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use crate::contract::{Context, OutputMode, UniversalArgs};

/// RAII guard. Holds OTel state for shutdown on drop. Today the only
/// resource is the OTel global tracer provider; without the `otel`
/// feature this is a zero-sized type.
pub struct ObservabilityHandle {
    #[cfg(feature = "otel")]
    otel_active: bool,
}

impl Drop for ObservabilityHandle {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        if self.otel_active {
            opentelemetry::global::shutdown_tracer_provider();
        }
    }
}

/// Resolve the effective log filter directive from CLI args + env.
fn resolve_filter(args: &UniversalArgs) -> String {
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        return rust_log;
    }
    let level = match args.log_level.as_deref() {
        Some(l) => l,
        None if args.verbose => "debug",
        None => return default_filter(),
    };
    // Leading `warn` keeps transitive crates (wasmtime, rustc_span,
    // salsa, kcl_*) quiet even at --log-level=debug; only akua* targets
    // honor the requested level. RUST_LOG remains the escape hatch for
    // anyone who needs to peek at internals.
    format!("warn,akua={level},akua_cli={level},akua_core={level},akua_render_worker={level}")
}

fn default_filter() -> String {
    // Leading `warn` silences transitive crates (wasmtime emits an
    // info span per syscall; kcl/rustc_span/salsa flood at debug);
    // only akua* targets stay at info.
    "warn,akua=info,akua_cli=info,akua_core=info,akua_render_worker=info".to_string()
}

/// Apply `AKUA_BRIDGE_TRACE=1` back-compat: OR `akua::bridge=debug` into
/// the directive so the existing diagnostic shortcut keeps working.
fn apply_bridge_trace(directive: String) -> String {
    if std::env::var("AKUA_BRIDGE_TRACE").is_ok_and(|v| v == "1") {
        format!("{directive},akua::bridge=debug")
    } else {
        directive
    }
}

/// Initialize the global tracing subscriber. Must be called exactly
/// once at process start.
pub fn init_subscriber(args: &UniversalArgs, ctx: &Context) -> ObservabilityHandle {
    let directive = apply_bridge_trace(resolve_filter(args));

    // Propagate to the worker subscriber. The render-worker
    // wasmtime store reads this back via WasiCtxBuilder.env(...) so
    // worker spans honor the host's --log-level / -v.
    if std::env::var_os("AKUA_WORKER_LOG").is_none() {
        std::env::set_var("AKUA_WORKER_LOG", &directive);
    }

    let env_filter = EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new("warn"));

    let json_mode = matches!(ctx.output, OutputMode::Json);
    let registry = tracing_subscriber::registry().with(env_filter);

    if json_mode {
        // Determinism: no timestamps in JSON / agent mode (golden tests
        // diff stderr too; an embedded clock would defeat them).
        let layer = fmt::layer()
            .json()
            .with_writer(io::stderr)
            .with_current_span(true)
            .with_span_list(false)
            .with_timer(())
            .boxed();
        let subscriber = registry.with(layer);
        init_with_otel(subscriber)
    } else {
        let layer = fmt::layer()
            .with_writer(io::stderr)
            .with_target(false)
            .with_ansi(ctx.color)
            .boxed();
        let subscriber = registry.with(layer);
        init_with_otel(subscriber)
    }
}

/// Install the configured subscriber. With the `otel` feature on, also
/// records whether `OTEL_EXPORTER_OTLP_ENDPOINT` is set so the handle's
/// `Drop` can flush the global tracer provider on exit.
fn init_with_otel<S>(subscriber: S) -> ObservabilityHandle
where
    S: tracing::Subscriber + Send + Sync + 'static,
    for<'a> S: tracing_subscriber::registry::LookupSpan<'a>,
{
    subscriber.init();
    #[cfg(feature = "otel")]
    {
        let otel_active = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok()
            || std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_ok();
        ObservabilityHandle { otel_active }
    }
    #[cfg(not(feature = "otel"))]
    {
        ObservabilityHandle {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_includes_akua_at_info() {
        let f = default_filter();
        assert!(f.contains("akua=info"));
        assert!(f.contains("akua_render_worker=info"));
    }

    #[test]
    fn bridge_trace_appends_directive() {
        std::env::set_var("AKUA_BRIDGE_TRACE", "1");
        let out = apply_bridge_trace("warn".to_string());
        assert!(out.contains("akua::bridge=debug"));
        std::env::remove_var("AKUA_BRIDGE_TRACE");
    }

    #[test]
    fn verbose_forces_debug_on_akua_targets() {
        let args = UniversalArgs {
            verbose: true,
            ..Default::default()
        };
        // RUST_LOG must not be set for this test; ensure it isn't.
        std::env::remove_var("RUST_LOG");
        let f = resolve_filter(&args);
        assert!(f.contains("akua=debug"), "got: {f}");
        assert!(f.contains("akua_render_worker=debug"), "got: {f}");
        assert!(f.starts_with("warn"), "leading level should be warn: {f}");
    }
}
