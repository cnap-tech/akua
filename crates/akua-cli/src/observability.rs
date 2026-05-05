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

/// RAII guard. Holds the tokio runtime that hosts the OTel batch
/// processor; on drop, flushes the global tracer provider and shuts
/// the runtime down. Without the `otel` feature this is zero-sized.
pub struct ObservabilityHandle {
    #[cfg(feature = "otel")]
    _otel_runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for ObservabilityHandle {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        if self._otel_runtime.is_some() {
            // Flush + close all batch processors. shutdown_tracer_provider
            // blocks while the batch processor exports queued spans.
            opentelemetry::global::shutdown_tracer_provider();
        }
    }
}

/// Resolve the effective log filter directive from CLI args + env.
/// `rust_log` is the value of `RUST_LOG` (None if unset). Split out so
/// tests can inject a value without `std::env::set_var` racing with
/// other tests; production callers funnel through [`resolve_filter`].
fn resolve_filter_with(args: &UniversalArgs, rust_log: Option<&str>) -> String {
    if let Some(directive) = rust_log {
        return directive.to_string();
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

fn resolve_filter(args: &UniversalArgs) -> String {
    resolve_filter_with(args, std::env::var("RUST_LOG").ok().as_deref())
}

fn default_filter() -> String {
    // Leading `warn` silences transitive crates (wasmtime emits an
    // info span per syscall; kcl/rustc_span/salsa flood at debug);
    // only akua* targets stay at info.
    "warn,akua=info,akua_cli=info,akua_core=info,akua_render_worker=info".to_string()
}

/// Pure version of [`apply_bridge_trace`]. `bridge_trace` is the value
/// of `AKUA_BRIDGE_TRACE` (None if unset); only `Some("1")` activates.
fn apply_bridge_trace_with(directive: String, bridge_trace: Option<&str>) -> String {
    if bridge_trace == Some("1") {
        format!("{directive},akua::bridge=debug")
    } else {
        directive
    }
}

/// Apply `AKUA_BRIDGE_TRACE=1` back-compat: OR `akua::bridge=debug` into
/// the directive so the existing diagnostic shortcut keeps working.
fn apply_bridge_trace(directive: String) -> String {
    apply_bridge_trace_with(
        directive,
        std::env::var("AKUA_BRIDGE_TRACE").ok().as_deref(),
    )
}

/// Initialize the global tracing subscriber. Must be called exactly
/// once at process start. With the `otel` feature on AND either
/// `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
/// set, an OTLP exporter layer is stacked under the fmt layer. All
/// standard `OTEL_*` env vars (endpoint, headers, protocol, timeout,
/// `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`) are honored
/// natively by the underlying opentelemetry crates.
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
    let fmt_layer = if json_mode {
        // Determinism: no timestamps in JSON / agent mode (golden tests
        // diff stderr too; an embedded clock would defeat them).
        fmt::layer()
            .json()
            .with_writer(io::stderr)
            .with_current_span(true)
            .with_span_list(false)
            .with_timer(())
            .boxed()
    } else {
        fmt::layer()
            .with_writer(io::stderr)
            .with_target(false)
            .with_ansi(ctx.color)
            .boxed()
    };

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer);

    #[cfg(feature = "otel")]
    {
        if otel_endpoint_configured() {
            match build_otel_layer() {
                Ok((layer, runtime)) => {
                    registry.with(layer).init();
                    return ObservabilityHandle {
                        _otel_runtime: Some(runtime),
                    };
                }
                Err(err) => {
                    eprintln!("akua: OpenTelemetry init failed ({err}); continuing without OTel");
                }
            }
        }
        registry.init();
        ObservabilityHandle {
            _otel_runtime: None,
        }
    }
    #[cfg(not(feature = "otel"))]
    {
        registry.init();
        ObservabilityHandle {}
    }
}

#[cfg(feature = "otel")]
fn otel_endpoint_configured() -> bool {
    std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        || std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_some()
}

#[cfg(feature = "otel")]
fn build_otel_layer<S>() -> Result<
    (
        tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>,
        tokio::runtime::Runtime,
    ),
    Box<dyn std::error::Error>,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider as _;

    // Tonic and the batch span processor both poll inside a tokio
    // runtime; akua is otherwise sync, so we host one current-thread
    // Runtime that lives as long as ObservabilityHandle.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // install_batch needs a tokio context to spawn the processor's
    // worker task. The pipeline reads endpoint, headers, timeout, and
    // protocol from standard OTEL_* env vars.
    let provider = runtime.block_on(async {
        opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(opentelemetry_otlp::new_exporter().tonic())
            .install_batch(opentelemetry_sdk::runtime::TokioCurrentThread)
    })?;

    let tracer = provider.tracer("akua");
    opentelemetry::global::set_tracer_provider(provider);
    let layer = tracing_opentelemetry::layer().with_tracer(tracer);
    Ok((layer, runtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_includes_akua_at_info() {
        let f = default_filter();
        assert!(f.contains("akua=info"));
        assert!(f.contains("akua_render_worker=info"));
        assert!(f.starts_with("warn"));
    }

    /// `RUST_LOG` wins outright — full directive returned verbatim,
    /// neither `--verbose` nor `--log-level` overrides it.
    #[test]
    fn resolve_filter_rust_log_takes_precedence_over_args() {
        let args = UniversalArgs {
            verbose: true,
            log_level: Some("trace".into()),
            ..Default::default()
        };
        let f = resolve_filter_with(&args, Some("info,my_crate=trace"));
        assert_eq!(f, "info,my_crate=trace");
    }

    /// `--log-level=<x>` (no `--verbose`, no RUST_LOG) → akua* targets
    /// at <x>, leading `warn` for transitive crates.
    #[test]
    fn resolve_filter_log_level_arg_drives_akua_targets() {
        let args = UniversalArgs {
            log_level: Some("trace".into()),
            ..Default::default()
        };
        let f = resolve_filter_with(&args, None);
        assert!(f.starts_with("warn,"), "got: {f}");
        assert!(f.contains("akua=trace"));
        assert!(f.contains("akua_cli=trace"));
        assert!(f.contains("akua_core=trace"));
        assert!(f.contains("akua_render_worker=trace"));
    }

    /// `-v` / `--verbose` (no `--log-level`, no RUST_LOG) forces debug
    /// on akua* targets.
    #[test]
    fn resolve_filter_verbose_forces_debug() {
        let args = UniversalArgs {
            verbose: true,
            ..Default::default()
        };
        let f = resolve_filter_with(&args, None);
        assert!(f.contains("akua=debug"), "got: {f}");
        assert!(f.contains("akua_render_worker=debug"), "got: {f}");
        assert!(f.starts_with("warn"), "leading level should be warn: {f}");
    }

    /// `--log-level` wins over `--verbose` when both are set (explicit
    /// flag beats the verbose shorthand).
    #[test]
    fn resolve_filter_log_level_beats_verbose() {
        let args = UniversalArgs {
            verbose: true,
            log_level: Some("warn".into()),
            ..Default::default()
        };
        let f = resolve_filter_with(&args, None);
        assert!(f.contains("akua=warn"));
        assert!(!f.contains("akua=debug"));
    }

    /// No flags + no env → the default filter.
    #[test]
    fn resolve_filter_no_flags_no_env_uses_default() {
        let args = UniversalArgs::default();
        assert_eq!(resolve_filter_with(&args, None), default_filter());
    }

    #[test]
    fn bridge_trace_appends_when_env_is_one() {
        let out = apply_bridge_trace_with("warn".into(), Some("1"));
        assert_eq!(out, "warn,akua::bridge=debug");
    }

    /// `AKUA_BRIDGE_TRACE` set to anything other than `"1"` is a no-op
    /// (legacy convention — empty / "true" / "0" all stay quiet).
    #[test]
    fn bridge_trace_only_activates_on_literal_one() {
        assert_eq!(apply_bridge_trace_with("warn".into(), Some("0")), "warn");
        assert_eq!(apply_bridge_trace_with("warn".into(), Some("true")), "warn");
        assert_eq!(apply_bridge_trace_with("warn".into(), Some("")), "warn");
    }

    #[test]
    fn bridge_trace_unset_env_is_passthrough() {
        let out = apply_bridge_trace_with("warn,akua=info".into(), None);
        assert_eq!(out, "warn,akua=info");
    }
}
