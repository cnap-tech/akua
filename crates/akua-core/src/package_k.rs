//! `Package.k` loader — read a KCL Package, inject inputs, execute it,
//! parse the rendered YAML into typed Rust data.
//!
//! Spec: [`docs/package-format.md`](../../../docs/package-format.md).
//!
//! Inputs flow through KCL's built-in `option()` mechanism (the
//! in-process equivalent of `kcl -D input=<json>`): the caller's
//! inputs are JSON-encoded and passed as an `ExecProgramArgs.args`
//! entry keyed by [`INPUT_OPTION_KEY`]. Packages bind it with
//!
//! ```kcl
//! input: Input = option("input") or Input {}
//! ```
//!
//! so a Package is standalone-valid KCL (`kcl fmt` / `kcl lint` / IDE
//! LSPs all work without akua-specific preprocessing).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml::Value;

/// The `option()` key every Package uses for its `input` binding.
const INPUT_OPTION_KEY: &str = "input";

/// Probe-order for Package inputs auto-discovery. When the caller
/// doesn't pass `--inputs`, we look alongside the `package.k` at
/// `inputs.yaml` first, then `inputs.example.yaml`. Returns `None`
/// if neither file exists. Shared between `akua render` + `akua dev`
/// so the auto-discovery path can't drift between them.
pub fn resolve_inputs_path(package_path: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let package_dir = package_path.parent().unwrap_or(Path::new("."));
    for candidate in ["inputs.yaml", "inputs.example.yaml"] {
        let probe = package_dir.join(candidate);
        if probe.is_file() {
            return Some(probe);
        }
    }
    None
}

/// A loaded `Package.k` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageK {
    pub path: PathBuf,
    pub source: String,
}

/// Result of evaluating a `Package.k` with concrete inputs.
///
/// `resources` is the flat list of Kubernetes-shaped dicts the Package
/// emitted — opaque to this module; a reconciler or policy engine
/// parses them. The renderer writes them as raw YAML under `--out`.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPackage {
    pub resources: Vec<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageKError {
    #[error("Package.k not found at {path}")]
    Missing { path: PathBuf },

    #[error("i/o error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize inputs to JSON: {0}")]
    InputJson(#[from] serde_json::Error),

    #[error("kcl eval failed: {0}")]
    KclEval(String),

    #[error("kcl output is not valid YAML: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("rendered Package must set top-level `resources`; got no such key")]
    MissingResources,

    #[error("rendered Package must be a top-level mapping; got {got}")]
    TopLevelWrongShape { got: &'static str },

    #[error("`resources` must be a sequence; got {got}")]
    ResourcesWrongShape { got: &'static str },

    #[error("cycle detected while expanding pkg.render — `{path}` is already on the render stack")]
    Cycle { path: PathBuf },

    #[error("plugin path escape: {0}")]
    PathEscape(#[from] crate::kcl_plugin::PathError),
}

impl PackageK {
    /// Read the file from disk. Maps `NotFound` to [`PackageKError::Missing`]
    /// so callers can distinguish "workspace not set up" from "disk broke."
    /// The stored `path` is canonicalized so consumers (RenderScope, plugin
    /// path resolution) can rely on `path.parent()` returning the package
    /// directory, even when the caller passed a bare filename.
    pub fn load(path: &Path) -> Result<Self, PackageKError> {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PackageKError::Missing {
                    path: path.to_path_buf(),
                });
            }
            Err(e) => {
                return Err(PackageKError::Io {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        Ok(PackageK {
            path: canon,
            source,
        })
    }

    /// In-process render against the given inputs, returning the
    /// typed resource list.
    ///
    /// **Private to akua-core.** Production render paths run inside
    /// the wasmtime sandbox (see `akua_cli::verbs::render::render_in_worker`)
    /// per CLAUDE.md's "Sandboxed by default. No shell-out, ever"
    /// invariant. This method stays crate-private for akua-core's
    /// own unit tests — it provides the same KCL + plugin-bridge
    /// semantics the sandbox path uses, minus the wasmtime isolation.
    ///
    /// `Package.k` files that `import charts.<name>` fail to parse
    /// on this path unless resolved charts are supplied — callers
    /// use [`render_with_charts`](Self::render_with_charts).
    pub(crate) fn render(&self, inputs: &Value) -> Result<RenderedPackage, PackageKError> {
        self.render_with_charts(inputs, &crate::chart_resolver::ResolvedCharts::default())
    }

    /// Like [`render`](Self::render), but also registers a per-render
    /// `charts` KCL package containing one module per resolved dep.
    /// Crate-private; see [`render`](Self::render) for the rationale.
    pub(crate) fn render_with_charts(
        &self,
        inputs: &Value,
        charts: &crate::chart_resolver::ResolvedCharts,
    ) -> Result<RenderedPackage, PackageKError> {
        self.render_opts(inputs, charts, false)
    }

    /// Full-option render: charts + `strict` mode. `strict=true`
    /// rejects plugin paths that don't come from a typed
    /// `charts.*` import. Crate-private; see [`render`](Self::render)
    /// for the rationale.
    pub(crate) fn render_opts(
        &self,
        inputs: &Value,
        charts: &crate::chart_resolver::ResolvedCharts,
        strict: bool,
    ) -> Result<RenderedPackage, PackageKError> {
        // Register every engine callable whose feature flag is on.
        // Idempotent per invocation — safe to call every render.
        crate::kcl_plugin::install_builtin_plugins();

        // Push self onto the render stack so plugin handlers can
        // resolve user-supplied relative paths (helm chart dirs,
        // nested package refs) against this Package's directory.
        // Resolved chart paths are registered as allowed absolute
        // roots so `helm.template(nginx.path, ...)` survives the
        // path-escape guard. Dropped on return — nested renders
        // (pkg.render) stack naturally.
        let allowed_roots: Vec<PathBuf> = charts
            .entries
            .values()
            .map(|c| c.abs_path.clone())
            .collect();
        let _scope = crate::kcl_plugin::RenderScope::enter_with(&self.path, &allowed_roots, strict);

        // Materialize `charts/` alongside the static `akua/` stdlib.
        // TempDir dropped at end of scope, after `exec_program` has
        // finished loading + executing the Package.
        let charts_tmp = crate::stdlib::materialize_charts_if_any(charts)
            .map_err(|e| PackageKError::KclEval(format!("materializing charts pkg: {e}")))?;

        // KCL ecosystem deps need standalone ExternalPkg entries so
        // imports like `import k8s.api.apps.v1` resolve against the
        // upstream module tree rather than the synthetic `charts.*`
        // umbrella. Path stays the resolved on-disk root — KCL
        // reads it directly.
        let kcl_pkgs: std::collections::BTreeMap<String, std::path::PathBuf> = charts
            .kcl_pkgs()
            .map(|(alias, c)| (alias.to_string(), c.abs_path.clone()))
            .collect();

        let json = serde_json::to_string(inputs)?;
        let yaml = eval_kcl(
            &self.path,
            &self.source,
            &json,
            charts_tmp.as_ref().map(|d| d.path()),
            &kcl_pkgs,
        )?;
        let parsed = parse_rendered(&yaml)?;

        // `pkg.render` resolves inline inside its plugin handler, so
        // every nested call is already part of `parsed.resources`.
        Ok(RenderedPackage {
            resources: parsed.resources,
        })
    }
}

/// Parse a rendered Package's top-level YAML (the `yaml_result` KCL
/// produced, or an equivalent string the sandboxed render worker
/// returns) into a typed [`RenderedPackage`]. Exposed so the
/// wasmtime-hosted render path in `akua-cli` can share the same
/// parse + validation rules as the native in-process path.
pub fn parse_rendered_yaml(yaml: &str) -> Result<RenderedPackage, PackageKError> {
    parse_rendered(yaml)
}

fn parse_rendered(yaml: &str) -> Result<RenderedPackage, PackageKError> {
    let top: Value = serde_yaml::from_str(yaml)?;
    let got = value_kind(&top);
    let Value::Mapping(map) = top else {
        return Err(PackageKError::TopLevelWrongShape { got });
    };

    let resources = match map.get(Value::String("resources".into())) {
        None => return Err(PackageKError::MissingResources),
        Some(Value::Sequence(s)) => s.clone(),
        Some(other) => {
            return Err(PackageKError::ResourcesWrongShape {
                got: value_kind(other),
            });
        }
    };

    Ok(RenderedPackage { resources })
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged",
    }
}

crate::contract_type! {
/// Structured issue reported by [`lint_kcl`]. Mirrors KCL's own
/// `Error.messages[*]` shape, flattened one-row-per-message so
/// consumers don't need to walk a two-level tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LintIssue {
    /// `"Error"` or `"Warning"` as reported by KCL.
    pub level: String,

    /// KCL error code (e.g. `"E1001"`). Empty string when KCL emits no
    /// code — preserved verbatim.
    pub code: String,

    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
}
}

/// Parse a KCL source buffer and return any parse / load errors. Pure;
/// no execution, no filesystem read. Pair entry point for
/// `wasm32-unknown-unknown` consumers and `@akua-dev/sdk`'s in-process
/// `lint` verb. `filename` is used for diagnostic rendering only —
/// KCL doesn't touch it on disk when `sources` is populated.
pub fn lint_kcl_source(filename: &str, source: &str) -> Result<Vec<LintIssue>, PackageKError> {
    use kcl_lang::{ParseProgramArgs, API};

    let api = API::default();
    let args = ParseProgramArgs {
        paths: vec![filename.to_string()],
        sources: vec![source.to_string()],
        external_pkgs: akua_external_pkgs(),
    };
    run_parse_program(&api, &args)
}

/// Parse a KCL file via kcl_lang and return any parse / load errors.
/// Thin wrapper that reads the file and defers to
/// [`lint_kcl_source`]. Unavailable on `wasm32-unknown-unknown`;
/// SDK consumers read files on the JS side and call the `_source`
/// variant through the WASM bridge.
pub fn lint_kcl(path: &Path) -> Result<Vec<LintIssue>, PackageKError> {
    let source = std::fs::read_to_string(path).map_err(|source| PackageKError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    lint_kcl_source(&path.to_string_lossy(), &source)
}

fn run_parse_program(
    api: &kcl_lang::API,
    args: &kcl_lang::ParseProgramArgs,
) -> Result<Vec<LintIssue>, PackageKError> {
    match api.parse_program(args) {
        Ok(result) => Ok(result
            .errors
            .into_iter()
            .flat_map(|e| {
                let level = e.level;
                let code = e.code;
                e.messages.into_iter().map(move |m| {
                    let pos = m.pos.unwrap_or_default();
                    LintIssue {
                        level: level.clone(),
                        code: code.clone(),
                        message: m.msg,
                        file: (!pos.filename.is_empty()).then_some(pos.filename),
                        line: (pos.line > 0).then_some(pos.line),
                        column: (pos.column > 0).then_some(pos.column),
                    }
                })
            })
            .collect()),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

/// ExternalPkg registration for parse-time `import akua.*` resolution.
/// Host targets (CLI + render worker) materialize the stdlib via
/// `stdlib::stdlib_root()`; wasm32-unknown-unknown has no filesystem,
/// so Packages that import the akua stdlib fail to parse there. The
/// in-process SDK consumers get a clear diagnostic naming the missing
/// stdlib module — same shape `lint` already produces today.
pub(crate) fn akua_external_pkgs() -> Vec<kcl_lang::ExternalPkg> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        vec![kcl_lang::ExternalPkg {
            pkg_name: "akua".to_string(),
            pkg_path: crate::stdlib::stdlib_root().to_string_lossy().into_owned(),
        }]
    }
    #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
    {
        vec![kcl_lang::ExternalPkg {
            pkg_name: "akua".to_string(),
            pkg_path: "/akua-stdlib".to_string(),
        }]
    }
    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
    {
        Vec::new()
    }
}

crate::contract_type! {
/// A single `option()` call-site in a parsed Package, surfaced for
/// inspection without executing the program. Mirrors kcl_lang's
/// `OptionHelp` shape with idiomatic Rust optionals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionInfo {
    /// The option key (first argument to `option("…")`).
    pub name: String,

    /// The declared type of the binding receiving the option — e.g.
    /// `"Input"` for `input: Input = option("input") or Input {}`.
    /// Empty when the option is used without a type annotation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub r#type: String,

    pub required: bool,

    /// Default value (literal form) when the option call includes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// `help="…"` text attached to the option call, surfaced in docs
    /// tooling; absent when the authoring site didn't provide any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}
}

/// List every `option()` call-site declared in an in-memory KCL
/// source buffer. Parse-only, pure; see [`lint_kcl_source`] for the
/// filesystem-free story.
pub fn list_options_kcl_source(
    filename: &str,
    source: &str,
) -> Result<Vec<OptionInfo>, PackageKError> {
    use kcl_lang::{ParseProgramArgs, API};

    let api = API::default();
    let args = ParseProgramArgs {
        paths: vec![filename.to_string()],
        sources: vec![source.to_string()],
        external_pkgs: akua_external_pkgs(),
    };
    match api.list_options(&args) {
        Ok(result) => Ok(result
            .options
            .into_iter()
            .map(|o| OptionInfo {
                name: o.name,
                r#type: o.r#type,
                required: o.required,
                default: (!o.default_value.is_empty()).then_some(o.default_value),
                help: (!o.help.is_empty()).then_some(o.help),
            })
            .collect()),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

/// List every `option()` call-site at `path`. Thin file-reading
/// wrapper over [`list_options_kcl_source`].
pub fn list_options_kcl(path: &Path) -> Result<Vec<OptionInfo>, PackageKError> {
    let source = std::fs::read_to_string(path).map_err(|source| PackageKError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    list_options_kcl_source(&path.to_string_lossy(), &source)
}

/// Format a KCL source string via kcl_lang's formatter. Used by
/// `akua fmt` — pure function, no filesystem access.
pub fn format_kcl(source: &str) -> Result<String, PackageKError> {
    use kcl_lang::{FormatCodeArgs, API};

    let api = API::default();
    match api.format_code(&FormatCodeArgs {
        source: source.to_string(),
    }) {
        Ok(result) => String::from_utf8(result.formatted)
            .map_err(|e| PackageKError::KclEval(format!("format output not utf-8: {e}"))),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

/// Evaluate a KCL source buffer and return the top-level YAML dict
/// as a string. Unlike [`PackageK::render`], this doesn't require
/// `resources = [...]` — it returns whatever top-level bindings the
/// program produces. Used by `akua repl` where operators type
/// arbitrary expressions, not fully-formed Packages.
///
/// `path` is a display-only filename for error messages; no file is
/// read. KCL's upstream `rustc_span` rejects paths ending in `>`, so
/// use `repl.k` / `line-N.k` rather than `<repl:N>`. No plugins are
/// installed + no `charts` pkg — the repl is a pure KCL evaluator.
/// Engine callables (helm/kustomize/pkg.render) belong inside a
/// workspace that `akua render` picks up.
pub fn eval_source(path: &Path, source: &str) -> Result<String, PackageKError> {
    eval_source_with_inputs(
        path,
        source,
        &serde_yaml::Value::Mapping(Default::default()),
    )
}

/// Like [`eval_source`] but injects the given inputs as the KCL
/// `option("input")` value. Used by the render worker to carry
/// `inputs.yaml` / `--inputs` into the sandbox — `eval_source` stays
/// for the REPL + tests that don't need inputs.
pub fn eval_source_with_inputs(
    path: &Path,
    source: &str,
    inputs: &Value,
) -> Result<String, PackageKError> {
    eval_source_full(
        path,
        source,
        inputs,
        None,
        &std::collections::BTreeMap::new(),
    )
}

/// Full-surface eval: inputs + a generated `charts` KCL pkg dir +
/// any KCL ecosystem pkg mounts (`kcl_pkgs`).
///
/// The render worker calls this with a preopened path where the host
/// has dropped the output of [`crate::stdlib::materialize_charts`].
/// KCL's import resolver sees the `charts` ExternalPkg and resolves
/// `import charts.<name>` to the files there. Plugin callouts from
/// those imports still flow through the host-side plugin bridge
/// (helm / kustomize handlers live on akua-cli's side, not in the
/// worker).
///
/// `kcl_pkgs` is an alias→guest-path map of upstream KCL packages
/// (e.g. `oci://ghcr.io/kcl-lang/k8s`). Each entry registers as its
/// own `ExternalPkg` so the Package can write
/// `import k8s.api.apps.v1` directly.
///
/// `charts_pkg_dir = None` and `kcl_pkgs.is_empty()` is equivalent
/// to [`eval_source_with_inputs`] — no extern resolution, bare-KCL
/// only.
pub fn eval_source_full(
    path: &Path,
    source: &str,
    inputs: &Value,
    charts_pkg_dir: Option<&Path>,
    kcl_pkgs: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Result<String, PackageKError> {
    let json = serde_json::to_string(inputs)?;
    eval_kcl(path, source, &json, charts_pkg_dir, kcl_pkgs)
}

/// Strip akua-extension decorators (`@ui(...)`) from KCL source
/// before handing it to the resolver. KCL's resolver only knows
/// `@deprecated` / `@info`; any other decorator name surfaces as
/// `UnKnown decorator …` and aborts compilation. `akua export`
/// extracts these directly from the parsed AST, so render doesn't
/// need them — strip and continue.
///
/// Strips any line whose first non-whitespace token is `@ui(` and
/// continues consuming until the parenthesis balance returns to
/// zero, so multi-line decorator forms are also removed. The blank
/// line is left in place to preserve KCL line numbers in diagnostics.
fn strip_akua_decorators(source: &str) -> String {
    const PREFIXES: &[&str] = &["@ui("];
    // Fast-path: most Packages don't carry `@ui(...)`. Skip the
    // line-by-line scan unless the substring actually appears.
    if !PREFIXES.iter().any(|p| source.contains(p)) {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Consume until paren balance returns to zero, tracking
        // string-literal context so `@ui(label="(foo)")` works.
        // Replaced lines become blank to preserve KCL line numbers
        // in diagnostics.
        let mut depth: i32 = 0;
        let mut current = line;
        loop {
            depth += paren_balance(current);
            out.push('\n');
            if depth <= 0 {
                break;
            }
            match lines.next() {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    out
}

/// Net paren balance of `line`, treating characters inside `"..."` /
/// `'...'` / `"""..."""` / `'''...'''` string literals as inert.
/// Backslash-escaped quotes inside a single/double quoted string are
/// honoured; KCL doesn't allow escapes inside triple-quoted strings.
fn paren_balance(line: &str) -> i32 {
    #[derive(PartialEq, Eq)]
    enum S {
        Code,
        Str1,
        Str2,
        Str1x3,
        Str2x3,
    }
    let bytes = line.as_bytes();
    let mut state = S::Code;
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            S::Code => {
                if i + 2 < bytes.len() && &bytes[i..i + 3] == b"\"\"\"" {
                    state = S::Str2x3;
                    i += 3;
                    continue;
                }
                if i + 2 < bytes.len() && &bytes[i..i + 3] == b"'''" {
                    state = S::Str1x3;
                    i += 3;
                    continue;
                }
                match b {
                    b'"' => state = S::Str2,
                    b'\'' => state = S::Str1,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b'#' => break, // line-comment runs to EOL
                    _ => {}
                }
            }
            S::Str1 => match b {
                b'\\' => i += 1, // skip escaped char
                b'\'' => state = S::Code,
                _ => {}
            },
            S::Str2 => match b {
                b'\\' => i += 1,
                b'"' => state = S::Code,
                _ => {}
            },
            S::Str1x3 => {
                if i + 2 < bytes.len() && &bytes[i..i + 3] == b"'''" {
                    state = S::Code;
                    i += 3;
                    continue;
                }
            }
            S::Str2x3 => {
                if i + 2 < bytes.len() && &bytes[i..i + 3] == b"\"\"\"" {
                    state = S::Code;
                    i += 3;
                    continue;
                }
            }
        }
        i += 1;
    }
    depth
}

fn eval_kcl(
    path: &Path,
    code: &str,
    option_json: &str,
    charts_pkg_dir: Option<&Path>,
    kcl_pkgs: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Result<String, PackageKError> {
    use kcl_lang::{Argument, ExecProgramArgs, ExternalPkg, API};

    // A non-zero plugin_agent installs the akua-side plugin dispatcher
    // so `kcl_plugin.<module>.<fn>` calls inside the Package resolve
    // to handlers registered via `kcl_plugin::register`. Zero (default)
    // leaves the dispatcher disabled — matches upstream KCL.
    let api = API {
        plugin_agent: crate::kcl_plugin::plugin_agent_ptr(),
    };
    // On host we materialize the akua KCL stdlib under $TMPDIR and
    // hand its absolute path to KCL as an ExternalPkg. On wasip1 we
    // can't do that — `std::env::temp_dir()` + `std::fs::write` are
    // unconditional panics. The host instead preopens its own
    // materialized stdlib into the worker's WasiCtx at `/akua-stdlib`
    // (see `akua_cli::render_worker::invoke_inner`), and we hand
    // KCL that guest-visible path. Identical import shape on both
    // sides: `import akua.helm` resolves either way.
    let mut external_pkgs: Vec<ExternalPkg> = Vec::new();
    #[cfg(target_arch = "wasm32")]
    external_pkgs.push(ExternalPkg {
        pkg_name: "akua".to_string(),
        pkg_path: "/akua-stdlib".to_string(),
    });
    #[cfg(not(target_arch = "wasm32"))]
    external_pkgs.push(ExternalPkg {
        pkg_name: "akua".to_string(),
        pkg_path: crate::stdlib::stdlib_root().to_string_lossy().into_owned(),
    });
    if let Some(dir) = charts_pkg_dir {
        external_pkgs.push(ExternalPkg {
            pkg_name: "charts".to_string(),
            pkg_path: dir.to_string_lossy().into_owned(),
        });
    }
    // Upstream KCL ecosystem deps — one ExternalPkg per alias. The
    // host has preopened each at the matching guest path.
    for (alias, guest_path) in kcl_pkgs {
        external_pkgs.push(ExternalPkg {
            pkg_name: alias.clone(),
            pkg_path: guest_path.to_string_lossy().into_owned(),
        });
    }
    let stripped = strip_akua_decorators(code);
    let args = ExecProgramArgs {
        k_filename_list: vec![path.to_string_lossy().into_owned()],
        k_code_list: vec![stripped],
        args: vec![Argument {
            name: INPUT_OPTION_KEY.to_string(),
            value: option_json.to_string(),
        }],
        // Expose the bundled akua KCL stdlib as the `akua` package,
        // so Packages can write `import akua.helm` / `import akua.pkg`
        // instead of reaching into `kcl_plugin.*` directly.
        external_pkgs,
        ..Default::default()
    };
    match api.exec_program(&args) {
        Ok(result) => {
            if !result.err_message.is_empty() {
                return Err(PackageKError::KclEval(result.err_message));
            }
            Ok(result.yaml_result)
        }
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Mapping;

    #[test]
    fn strip_decorators_removes_single_line_at_ui() {
        let src = "schema Input:\n    @ui(order=10)\n    name: str\n";
        let stripped = strip_akua_decorators(src);
        assert_eq!(stripped, "schema Input:\n\n    name: str\n");
    }

    #[test]
    fn strip_decorators_preserves_line_numbers_for_multiline() {
        let src = "schema Input:\n    @ui(\n        order=10,\n        group=\"x\",\n    )\n    name: str\n";
        let stripped = strip_akua_decorators(src);
        // Four blank lines for the four-line `@ui(...)` invocation —
        // line numbers in KCL diagnostics still match the original.
        assert_eq!(stripped, "schema Input:\n\n\n\n\n    name: str\n");
    }

    #[test]
    fn strip_decorators_handles_quoted_parens() {
        let src = "schema Input:\n    @ui(label=\"foo()bar\")\n    name: str\n";
        let stripped = strip_akua_decorators(src);
        // Without string-aware paren counting, the `)` inside the
        // string literal would close the decorator early and leak the
        // trailing `\")` onto the next line.
        assert_eq!(stripped, "schema Input:\n\n    name: str\n");
    }

    #[test]
    fn strip_decorators_fast_paths_when_no_at_ui() {
        let src = "schema Input:\n    name: str\n";
        // Fast-path: returns input string unchanged.
        assert_eq!(strip_akua_decorators(src), src);
    }

    /// Pure-KCL fixture: no engine imports, no external charts. Emits
    /// one ConfigMap whose `data.count` reflects `input.replicas`. Uses
    /// the spec's canonical `option("input")` binding so it runs under
    /// vanilla `kcl` as well as through this loader.
    const MINIMAL_FIXTURE: &str = r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "test"
    data.count: str(input.replicas)
}]
"#;

    fn write_fixture(source: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("tmp");
        let path = dir.path().join("package.k");
        std::fs::write(&path, source).expect("write fixture");
        (dir, path)
    }

    fn inputs(pairs: &[(&str, Value)]) -> Value {
        let mut m = Mapping::new();
        for (k, v) in pairs {
            m.insert(Value::String((*k).to_string()), v.clone());
        }
        Value::Mapping(m)
    }

    fn empty_inputs() -> Value {
        Value::Mapping(Mapping::new())
    }

    #[test]
    fn load_reads_file_content() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        // load canonicalizes; on macOS that prepends `/private/` to the
        // tempdir. Compare against the canonical form.
        assert_eq!(pkg.path, path.canonicalize().unwrap());
        assert_eq!(pkg.source, MINIMAL_FIXTURE);
    }

    #[test]
    fn load_missing_file_returns_typed_error() {
        let err = PackageK::load(Path::new("/does/not/exist.k")).unwrap_err();
        assert!(matches!(err, PackageKError::Missing { .. }));
    }

    #[test]
    fn render_minimal_configmap_fixture() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg
            .render(&inputs(&[("replicas", Value::Number(3.into()))]))
            .expect("render");

        assert_eq!(rendered.resources.len(), 1, "one ConfigMap emitted");
        let cm = &rendered.resources[0];
        assert_eq!(cm["kind"], Value::String("ConfigMap".into()));
        assert_eq!(cm["data"]["count"], Value::String("3".into()));
    }

    #[test]
    fn render_uses_default_when_input_absent() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&empty_inputs()).expect("render");
        // Default is `replicas: int = 2`.
        assert_eq!(
            rendered.resources[0]["data"]["count"],
            Value::String("2".into())
        );
    }

    #[test]
    fn render_is_deterministic() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let ins = inputs(&[("replicas", Value::Number(5.into()))]);
        let a = pkg.render(&ins).expect("a");
        let b = pkg.render(&ins).expect("b");
        assert_eq!(a, b, "same inputs must produce byte-identical output");
    }

    #[test]
    fn render_kcl_syntax_error_surfaces_typed() {
        let (_tmp, path) = write_fixture("this is not valid kcl !!!\n");
        let pkg = PackageK::load(&path).expect("load");
        let err = pkg.render(&empty_inputs()).unwrap_err();
        assert!(
            matches!(err, PackageKError::KclEval(_)),
            "expected KclEval, got {err:?}"
        );
    }

    #[test]
    fn render_missing_resources_typed() {
        let bad = r#"
schema Input:
    x: int = 0

input: Input = option("input") or Input {}
"#;
        let (_tmp, path) = write_fixture(bad);
        let pkg = PackageK::load(&path).expect("load");
        let err = pkg.render(&empty_inputs()).unwrap_err();
        assert!(
            matches!(err, PackageKError::MissingResources),
            "expected MissingResources, got {err:?}"
        );
    }

    #[test]
    fn render_with_charts_exposes_import_charts() {
        use crate::chart_resolver::{ResolvedChart, ResolvedCharts};
        use std::collections::BTreeMap;

        // Simulate a resolved `nginx` dep. Its `path` + `sha256`
        // should flow into the generated `charts/nginx.k` and be
        // reachable from the Package as `nginx.path` / `nginx.sha256`.
        let mut entries = BTreeMap::new();
        entries.insert(
            "nginx".to_string(),
            ResolvedChart {
                name: "nginx".to_string(),
                abs_path: PathBuf::from("/opt/charts/nginx"),
                sha256: "sha256:deadbeef".to_string(),
                kind: crate::chart_resolver::PackageKind::HelmChart,
                source: crate::chart_resolver::ResolvedSource::Path {
                    declared: "./charts/nginx".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        let fixture = r#"
import charts.nginx as nginx

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "chart-demo"
    data.chartPath:   nginx.path
    data.chartDigest: nginx.sha256
}]
"#;
        let (_tmp, path) = write_fixture(fixture);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg
            .render_with_charts(&empty_inputs(), &resolved)
            .expect("render");

        assert_eq!(rendered.resources.len(), 1);
        let cm = &rendered.resources[0];
        assert_eq!(
            cm["data"]["chartPath"],
            Value::String("/opt/charts/nginx".into())
        );
        assert_eq!(
            cm["data"]["chartDigest"],
            Value::String("sha256:deadbeef".into())
        );
    }

    #[test]
    fn render_without_charts_still_works() {
        // Back-compat: no-dep Package must not require charts wiring.
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&empty_inputs()).expect("render");
        assert_eq!(rendered.resources.len(), 1);
    }

    #[test]
    fn render_threads_nested_input_through_schema() {
        let fixture = r#"
schema Database:
    user: str = "app"
    port: int = 5432

schema Input:
    appName: str = "demo"
    database: Database = Database {}

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.user: input.database.user
    data.port: str(input.database.port)
}]
"#;
        let (_tmp, path) = write_fixture(fixture);
        let pkg = PackageK::load(&path).expect("load");

        // Nested input value across two schema levels.
        let mut db = Mapping::new();
        db.insert(
            Value::String("user".into()),
            Value::String("checkout_app".into()),
        );
        db.insert(Value::String("port".into()), Value::Number(6543.into()));

        let rendered = pkg
            .render(&inputs(&[
                ("appName", Value::String("checkout".into())),
                ("database", Value::Mapping(db)),
            ]))
            .expect("render");

        let cm = &rendered.resources[0];
        assert_eq!(cm["metadata"]["name"], Value::String("checkout".into()));
        assert_eq!(cm["data"]["user"], Value::String("checkout_app".into()));
        assert_eq!(cm["data"]["port"], Value::String("6543".into()));
    }

    #[test]
    fn eval_source_returns_top_level_bindings_without_resources() {
        // Bare bindings — no `resources = [...]`. `PackageK::render`
        // would reject this with MissingResources; eval_source
        // doesn't care.
        let yaml =
            eval_source(Path::new("repl.k"), "x = 42\ny = \"hello\"\n").expect("eval_source");
        let top: Value = serde_yaml::from_str(&yaml).unwrap();
        let map = top.as_mapping().expect("top-level mapping");
        assert_eq!(
            map.get(Value::String("x".into())),
            Some(&Value::Number(42.into()))
        );
        assert_eq!(
            map.get(Value::String("y".into())),
            Some(&Value::String("hello".into()))
        );
    }

    #[test]
    fn eval_source_surfaces_kcl_syntax_errors_verbatim() {
        let err = eval_source(Path::new("repl.k"), "this is not kcl")
            .expect_err("malformed KCL should error");
        assert!(matches!(err, PackageKError::KclEval(_)), "got {err:?}");
    }
}
