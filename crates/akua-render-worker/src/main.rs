//! `akua-render-worker` — the sandboxed render path.
//!
//! Compiled to `wasm32-wasip1`, AOT'd to `.cwasm`, embedded in the
//! `akua` CLI binary, instantiated per-render inside a wasmtime
//! `Store` with hard memory / fuel / epoch caps and capability-model
//! filesystem preopens. This is how CLAUDE.md's "sandboxed by
//! default" invariant is actually delivered.
//!
//! Body: load `akua.toml` + `Package.k` from a preopened workspace,
//! resolve deps, eval KCL, write YAML into a preopened output dir,
//! return the `RenderSummary` over JSON-on-stdio.
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

mod observability;

// Host-plugin bridge allocators — exported so the wasmtime host can
// place the plugin response into guest linear memory, return its
// pointer to KCL's `kcl_plugin_invoke_json_wasm` extern. C-string
// shape matches what KCL's runtime expects: null-terminated on read,
// host-owned after alloc.
//
// Leak-allocation is intentional: KCL's upstream convention is that
// the plugin response pointer is leaked (never freed by the caller).
// Linear memory pressure is bounded by the per-render Store cap
// (256 MiB default) — any legitimate render's plugin traffic fits
// comfortably inside that budget.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn akua_bridge_alloc(size: u32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 1)
        .expect("akua_bridge_alloc: invalid layout");
    unsafe { std::alloc::alloc(layout) }
}

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
    /// Source lives in-band — no filesystem access needed for bare
    /// Packages. When the Package does `import charts.<name>`, the
    /// host materializes the `charts` KCL pkg into a tempdir,
    /// preopens it into the worker's WASI context at
    /// `charts_pkg_path`, and the evaluator resolves imports via
    /// that mount. Plugin callouts (`helm.template` / `kustomize.
    /// build`) still flow back to host-registered handlers via the
    /// `kcl_plugin_invoke_json_wasm` bridge.
    Render {
        #[serde(default = "default_package_filename")]
        package_filename: String,
        source: String,
        #[serde(default)]
        inputs: Option<serde_json::Value>,
        /// Guest-visible path to the preopened `charts` pkg dir.
        /// Absent → no `charts.*` imports resolvable; the Package
        /// must be bare-KCL + plugin-call-only.
        #[serde(default)]
        charts_pkg_path: Option<String>,
        /// Upstream KCL ecosystem deps the host has preopened: alias
        /// → guest-visible path inside the sandbox. Each entry maps
        /// directly to a kcl-lang ExternalPkg, so the Package can
        /// `import <alias>.<module>` (e.g. `import k8s.api.apps.v1`)
        /// without going through the synthetic `charts.*` umbrella.
        #[serde(default)]
        kcl_pkgs: std::collections::BTreeMap<String, String>,
        /// Guest-visible path to the preopened `pkgs` stub dir.
        /// Absent → no Akua-package deps; consumers can't
        /// `import pkgs.<alias>` to reach upstream's typed schemas.
        #[serde(default)]
        pkgs_pkg_path: Option<String>,
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
    observability::init();
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

    let req: Request =
        serde_json::from_str(buf.trim()).map_err(|source| WorkerError::ParseRequest { source })?;

    let resp = match req {
        Request::Ping { note } => Response::Ping {
            status: "ok",
            echoed: note,
            worker_version: env!("CARGO_PKG_VERSION"),
        },
        // Evaluate the KCL source in-process (already inside the
        // per-render sandbox) and package the outcome. Eval errors
        // become `status: "fail"` with the diagnostic in `message` —
        // never a trap.
        Request::Render {
            package_filename,
            source,
            inputs,
            charts_pkg_path,
            kcl_pkgs,
            pkgs_pkg_path,
        } => render_request(
            package_filename,
            source,
            inputs,
            charts_pkg_path,
            kcl_pkgs,
            pkgs_pkg_path,
        ),
    };

    let out =
        serde_json::to_string(&resp).map_err(|source| WorkerError::EncodeResponse { source })?;
    std::io::stdout()
        .write_all(out.as_bytes())
        .map_err(|source| WorkerError::StdoutWrite { source })?;
    Ok(())
}

#[tracing::instrument(
    target = "akua::worker",
    level = "info",
    skip_all,
    fields(
        package_filename = %package_filename,
        source_size = source.len(),
        kcl_pkg_count = kcl_pkgs.len(),
        has_charts = charts_pkg_path.is_some(),
        has_pkgs = pkgs_pkg_path.is_some(),
    ),
)]
fn render_request(
    package_filename: String,
    source: String,
    inputs: Option<serde_json::Value>,
    charts_pkg_path: Option<String>,
    kcl_pkgs: std::collections::BTreeMap<String, String>,
    pkgs_pkg_path: Option<String>,
) -> Response {
    let ver = env!("CARGO_PKG_VERSION");

    // Protocol accepts inputs as JSON, akua-core wants a
    // serde_yaml::Value. Null / absent → empty mapping so Package's
    // `option("input")` resolves to a map, not a missing-option trap.
    let inputs_value: serde_yaml::Value = match inputs {
        Some(json) => match serde_json::from_value(json) {
            Ok(y) => y,
            Err(e) => {
                tracing::error!(target: "akua::worker", error = %e, "inputs decode failed");
                return Response::Render {
                    status: "fail",
                    yaml: String::new(),
                    message: format!("inputs must be a JSON object: {e}"),
                    worker_version: ver,
                };
            }
        },
        None => serde_yaml::Value::Mapping(Default::default()),
    };

    let charts_path_buf = charts_pkg_path.map(std::path::PathBuf::from);
    let charts_ref = charts_path_buf.as_deref();

    let pkgs_path_buf = pkgs_pkg_path.map(std::path::PathBuf::from);
    let pkgs_ref = pkgs_path_buf.as_deref();

    let kcl_pkgs_paths: std::collections::BTreeMap<String, std::path::PathBuf> = kcl_pkgs
        .into_iter()
        .map(|(alias, p)| (alias, std::path::PathBuf::from(p)))
        .collect();

    match akua_core::package_k::eval_source_full_with_pkgs(
        std::path::Path::new(&package_filename),
        &source,
        &inputs_value,
        charts_ref,
        &kcl_pkgs_paths,
        pkgs_ref,
    ) {
        Ok(yaml) => {
            tracing::debug!(target: "akua::worker", yaml_size = yaml.len(), "kcl eval ok");
            Response::Render {
                status: "ok",
                yaml,
                message: String::new(),
                worker_version: ver,
            }
        }
        Err(e) => {
            tracing::error!(target: "akua::worker", error = %e, "kcl eval failed");
            Response::Render {
                status: "fail",
                yaml: String::new(),
                message: e.to_string(),
                worker_version: ver,
            }
        }
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
