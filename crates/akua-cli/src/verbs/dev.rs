//! `akua dev` — file-watch + hot re-render loop.
//!
//! Runs until Ctrl-C. Each debounced save batch triggers one
//! re-render of the target Package; the verdict streams to stdout
//! as JSON lines (agent mode) or a colored one-line status
//! (human mode).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::dev::{self, DevEvent};
use akua_core::PackageK;

use crate::contract::{Context, OutputMode};

#[derive(Debug, Clone)]
pub struct DevArgs<'a> {
    pub workspace: &'a Path,

    /// Path to the Package.k. Default: `<workspace>/package.k`.
    pub package_path: PathBuf,

    /// Inputs file. When absent the render uses schema defaults,
    /// matching `akua render` auto-discovery.
    pub inputs_path: Option<PathBuf>,

    /// Render output dir. `./deploy` by default.
    pub out_dir: PathBuf,

    /// Debounce window for batching rapid saves.
    pub debounce: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error("setting up Ctrl-C handler: {0}")]
    SignalHandler(#[from] ctrlc::Error),

    #[error(transparent)]
    Watcher(#[from] dev::DevError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl DevError {
    pub fn to_structured(&self) -> StructuredError {
        StructuredError::new(codes::E_IO, self.to_string()).with_default_docs()
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::SystemError
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &DevArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, DevError> {
    // Shutdown flag shared with the Ctrl-C handler + the watch loop.
    // AtomicBool rather than a channel because the watcher polls this
    // predicate directly — no cross-thread wake needed, just "did
    // the user press Ctrl-C since we last looked?"
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_handler = stop.clone();
    ctrlc::set_handler(move || {
        stop_for_handler.store(true, Ordering::SeqCst);
    })?;

    let emit_json = matches!(ctx.output, OutputMode::Json);
    let mut stdout_ref = stdout;
    let stop_for_loop = stop.clone();

    akua_core::dev::watch_and_render(
        args.workspace,
        args.debounce,
        |changed| render_once(args, changed),
        |event| {
            // Broken-pipe handling: if the user pipes `akua dev |
            // head`, the first failed write must stop the loop —
            // otherwise we burn CPU re-rendering to a closed fd.
            // Setting `stop` here makes the next `should_stop`
            // check unwind cleanly.
            let write_result = if emit_json {
                let line = serde_json::to_string(event).unwrap_or_default();
                writeln!(&mut stdout_ref, "{line}")
            } else {
                write_human(&mut stdout_ref, event)
            };
            if write_result.is_err() || stdout_ref.flush().is_err() {
                stop.store(true, Ordering::SeqCst);
            }
        },
        || stop_for_loop.load(Ordering::SeqCst),
    )?;

    Ok(ExitCode::Success)
}

/// Load + render the Package once, returning a short one-line
/// summary. Called for both the startup seed and each debounced
/// change batch.
fn render_once(args: &DevArgs<'_>, _changed: &[PathBuf]) -> Result<String, String> {
    let pkg = PackageK::load(&args.package_path).map_err(|e| e.to_string())?;
    let inputs = load_inputs(args)?;
    // `akua dev` today doesn't resolve `[dependencies]` or expose
    // strict mode — defer both until the watch loop gets flags for
    // them. Rendering still runs in the sandbox.
    let charts = akua_core::chart_resolver::ResolvedCharts::default();
    let rendered = crate::verbs::render::render_in_worker(&pkg, &inputs, &charts, false)
        .map_err(|e| e.to_string())?;
    let summary = akua_core::package_render::render(&rendered, &args.out_dir, false)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{} manifest(s) → {} ({})",
        summary.manifests,
        summary.target.display(),
        summary.hash
    ))
}

fn load_inputs(args: &DevArgs<'_>) -> Result<serde_yaml::Value, String> {
    let path = match resolve_inputs_path(args) {
        Some(p) => p,
        None => return Ok(serde_yaml::Value::Mapping(Default::default())),
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_yaml::from_slice(&bytes).map_err(|e| format!("parsing {}: {e}", path.display()))
}

/// Shared probe order lives in akua-core.
fn resolve_inputs_path(args: &DevArgs<'_>) -> Option<PathBuf> {
    akua_core::package_k::resolve_inputs_path(&args.package_path, args.inputs_path.as_deref())
}

fn write_human<W: Write>(w: &mut W, event: &DevEvent) -> std::io::Result<()> {
    match event {
        DevEvent::Started { workspace, summary } => {
            writeln!(w, "watching {} ...", workspace.display())?;
            writeln!(w, "  initial render: {summary}")?;
        }
        DevEvent::Rendered {
            changed,
            took_ms,
            summary,
        } => {
            let trigger = changed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(w, "re-render ({trigger}) [{took_ms}ms]")?;
            writeln!(w, "  {summary}")?;
        }
        DevEvent::RenderError { changed, message } => {
            if changed.is_empty() {
                writeln!(w, "render error:")?;
            } else {
                let trigger = changed
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(w, "render error ({trigger}):")?;
            }
            for line in message.lines() {
                writeln!(w, "  {line}")?;
            }
        }
        DevEvent::Stopped => {
            writeln!(w, "stopped.")?;
        }
    }
    Ok(())
}
