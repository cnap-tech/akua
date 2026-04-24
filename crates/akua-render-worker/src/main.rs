//! `akua-render-worker` — the sandboxed render path.
//!
//! Compiled to `wasm32-wasip1`, AOT'd to `.cwasm`, embedded in the
//! `akua` CLI binary, instantiated per-render inside a wasmtime
//! `Store` with hard memory / fuel / epoch caps and capability-model
//! filesystem preopens. This is how CLAUDE.md's "sandboxed by
//! default" invariant is actually delivered (Phase 4).
//!
//! ## Scaffold state
//!
//! This commit ships the binary + its Cargo config but no render
//! logic yet. The body is a smoke harness:
//!
//! - Read a JSON request from stdin.
//! - Echo a JSON response on stdout.
//! - Exit 0 on success, non-zero on parse error.
//!
//! Task #410 grows the body into the real render dispatcher:
//! load `akua.toml` + `Package.k` from a preopened workspace,
//! resolve deps, eval KCL, write YAML into a preopened output dir,
//! return the `RenderSummary`. Task #412 adds the adversarial test
//! suite that validates the sandbox holds.
//!
//! ## Protocol
//!
//! Request on stdin (one JSON object):
//!
//! ```json
//! { "kind": "ping", "note": "optional" }
//! { "kind": "render", "package_filename": "package.k",
//!   "source": "...kcl...", "inputs": {...} }
//! ```
//!
//! Response on stdout — tagged by `kind` matching the request, plus a
//! `status` of `"ok" | "fail"`. Render carries `yaml` on success, a
//! `message` diagnostic on failure.
//!
//! Parse failure → exit 2 (UserError in the CLI contract). Other
//! I/O failure → exit 3 (SystemError). The host owns stderr for
//! structured errors; the worker uses stdout only for the response
//! envelope.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Request {
    /// Smoke request — kept in the protocol so host liveness tests
    /// can hit the worker without spinning up a real Package.
    Ping {
        #[serde(default)]
        note: Option<String>,
    },
    /// Evaluate a KCL source buffer and return its top-level YAML.
    /// Source lives in-band — no filesystem access. Multi-file
    /// Packages + chart imports land in a later slice via preopened
    /// workspace dirs + host-function plugin bridges.
    Render {
        #[serde(default = "default_package_filename")]
        package_filename: String,
        source: String,
        #[serde(default)]
        inputs: Option<serde_json::Value>,
    },
}

fn default_package_filename() -> String {
    "package.k".to_string()
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Response {
    Ping {
        status: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        echoed: Option<String>,
        worker_version: &'static str,
    },
    Render {
        status: &'static str,
        yaml: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        message: String,
        worker_version: &'static str,
    },
}

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            // Structured errors belong on stderr in the CLI
            // contract. The host peels these off and translates to
            // its own error envelope.
            let _ = writeln!(std::io::stderr(), "{{\"error\":\"{e}\"}}");
            e.exit_code()
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), WorkerError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|source| WorkerError::StdinRead { source })?;

    let req: Request = serde_json::from_str(buf.trim())
        .map_err(|source| WorkerError::ParseRequest { source })?;

    let resp = match req {
        Request::Ping { note } => Response::Ping {
            status: "ok",
            echoed: note,
            worker_version: env!("CARGO_PKG_VERSION"),
        },
        Request::Render {
            package_filename,
            source,
            inputs: _inputs,
        } => render(&package_filename, &source),
    };

    let out = serde_json::to_string(&resp).map_err(|source| WorkerError::EncodeResponse { source })?;
    std::io::stdout()
        .write_all(out.as_bytes())
        .map_err(|source| WorkerError::StdoutWrite { source })?;
    Ok(())
}

/// Evaluate the KCL source in-process (we're already inside the
/// per-render sandbox) and return the result as a `Response::Render`.
/// Any eval error becomes a `status: "fail"` with the diagnostic in
/// `message` — never a trap. Inputs are reserved for a future slice;
/// `eval_source` doesn't take them today.
fn render(package_filename: &str, source: &str) -> Response {
    use std::path::Path;
    match akua_core::eval_source(Path::new(package_filename), source) {
        Ok(yaml) => Response::Render {
            status: "ok",
            yaml,
            message: String::new(),
            worker_version: env!("CARGO_PKG_VERSION"),
        },
        Err(e) => Response::Render {
            status: "fail",
            yaml: String::new(),
            message: e.to_string(),
            worker_version: env!("CARGO_PKG_VERSION"),
        },
    }
}

#[derive(Debug)]
enum WorkerError {
    StdinRead { source: std::io::Error },
    ParseRequest { source: serde_json::Error },
    EncodeResponse { source: serde_json::Error },
    StdoutWrite { source: std::io::Error },
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::StdinRead { source } => write!(f, "stdin read: {source}"),
            WorkerError::ParseRequest { source } => write!(f, "parse request: {source}"),
            WorkerError::EncodeResponse { source } => write!(f, "encode response: {source}"),
            WorkerError::StdoutWrite { source } => write!(f, "stdout write: {source}"),
        }
    }
}

impl WorkerError {
    fn exit_code(&self) -> i32 {
        match self {
            WorkerError::ParseRequest { .. } => 2,
            _ => 3,
        }
    }
}
