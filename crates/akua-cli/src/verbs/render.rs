//! `akua render` — execute a Package against inputs and write raw YAML manifests.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua render` section.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::contract::{emit_output, Context};
use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::lock_file::{AkuaLock, LockLoadError, LockedPackage};
use akua_core::mod_file::ManifestLoadError;
use akua_core::{
    chart_resolver, chart_resolver::ResolverOptions, package_k::PackageKError, render,
    AkuaManifest, ChartResolveError, PackageK, PackageRenderError, RenderSummary, ResolvedCharts,
};

/// Substring of the `PathError::StrictRequiresTypedImport` Display
/// that's stable enough to sniff out of KCL's opaque plugin panic
/// envelope. Lives next to the error-conversion site so a change to
/// the PathError message stays colocated with its consumer.
const STRICT_MARKER: &str = "strict mode requires every chart";

/// Substring of the `PathError::Escape` Display, sniffed out of the
/// same KCL plugin-panic envelope as STRICT_MARKER. The sandboxed
/// render path collapses all errors to `KclEval(string)`, which is
/// why we can't pattern-match on the typed `PathError::Escape`
/// variant here — only the in-process render path keeps that typing.
const ESCAPE_MARKER: &str = "escapes the Package directory";

/// `pkg.render` rejected re-entry of a Package already on the
/// render stack. Stable substring of the prose error in
/// `crate::pkg_render`; sniffed out of the KCL plugin-panic envelope.
const CYCLE_MARKER: &str = "cycle detected";

/// `pkg.render` rejected because the inherited depth cap was hit.
const DEPTH_BUDGET_MARKER: &str = "render depth limit";

/// `pkg.render` rejected because the inherited wall-clock deadline
/// was already in the past.
const DEADLINE_BUDGET_MARKER: &str = "wall-clock budget exhausted";

/// User-facing remediation for `E_PATH_ESCAPE`. Emitted as the
/// `suggestion` field on the structured error so agents have a
/// machine-readable next-action without parsing the `docs/errors/`
/// page. Kept identical across the sandboxed (KclEval-string) and
/// in-process (typed PathError::Escape) match arms.
const PATH_ESCAPE_SUGGESTION: &str = "Two ways out: \
    (1) vendor the dependency as a subdirectory of this Package and reference it with a Package-relative path (e.g. `./vendor/<name>`); or \
    (2) declare it in `akua.toml` `[dependencies]` and reference the resolved alias (`charts.<name>.path` for Helm charts; `import <alias>` for KCL/Akua packages). \
    See docs/errors/E_PATH_ESCAPE.md.";

const STRICT_SUGGESTION: &str = "Declare the chart in `akua.toml` and `import charts.<name>`, then pass `chart = <name>.path` to `helm.template`.";

const CYCLE_SUGGESTION: &str =
    "Composition cycle in `pkg.render` calls. Break the dependency loop — a Package cannot directly or transitively render itself.";

const DEPTH_BUDGET_SUGGESTION: &str =
    "Recursive `pkg.render` exceeded the depth cap (default 16). Flatten the composition chain.";

const DEADLINE_BUDGET_SUGGESTION: &str =
    "The wall-clock deadline installed by the outer caller had already expired before the nested `pkg.render` could run. Raise the deadline or split the work.";

/// Marker → (code, suggestion) lookup for the `KclEval` arm of
/// [`RenderError::to_structured`]. Order is by selectivity, but the
/// `render_error_markers_are_substring_disjoint` test pins the
/// markers as pairwise-disjoint so any order would be correct.
const KCL_EVAL_MARKER_TABLE: &[(&str, &str, &str)] = &[
    (
        STRICT_MARKER,
        codes::E_STRICT_UNTYPED_CHART,
        STRICT_SUGGESTION,
    ),
    (ESCAPE_MARKER, codes::E_PATH_ESCAPE, PATH_ESCAPE_SUGGESTION),
    (CYCLE_MARKER, codes::E_RENDER_CYCLE, CYCLE_SUGGESTION),
    (
        DEPTH_BUDGET_MARKER,
        codes::E_RENDER_BUDGET_DEPTH,
        DEPTH_BUDGET_SUGGESTION,
    ),
    (
        DEADLINE_BUDGET_MARKER,
        codes::E_RENDER_BUDGET_DEADLINE,
        DEADLINE_BUDGET_SUGGESTION,
    ),
];

#[derive(Debug, Clone)]
pub struct RenderArgs<'a> {
    pub package_path: &'a Path,

    /// Optional inputs file. Parsed via `serde_yaml`, which accepts
    /// both YAML and JSON.
    pub inputs_path: Option<&'a Path>,

    /// Root directory for the rendered YAML files (`--out`).
    pub out_dir: &'a Path,

    /// `--dry-run`: compute the summary without writing files.
    pub dry_run: bool,

    /// `--stdout`: emit rendered manifests as multi-document YAML to
    /// stdout instead of writing files.
    pub stdout_mode: bool,

    /// `--strict`: reject raw-string plugin paths. Every chart must
    /// come from a typed `import charts.<name>` — i.e. must be
    /// declared in `akua.toml` and reachable via the resolver. Flips
    /// the render path from "best effort" to "every dep accounted
    /// for," which is what CI pipelines want.
    pub strict: bool,

    /// `--offline`: the resolver may not touch the network. OCI deps
    /// must be fully satisfied from the content-addressed cache. A
    /// missing cache entry fails the render with `E_CHART_RESOLVE`
    /// (distinct from a HTTP failure). Path + replace deps always
    /// resolve locally, so offline renders keep working for them.
    /// Designed for air-gapped CI runners where the `akua add` step
    /// happened elsewhere.
    pub offline: bool,

    /// `--debug`: under `--json`, emit `evalResult` (the post-eval
    /// resources list pre-YAML-normalization) alongside the summary.
    /// Useful for inspecting what `pkg.render` / `helm.template` /
    /// `kustomize.build` actually produced before the render writes
    /// hit disk. Best-effort surface — schema may change between
    /// releases.
    pub debug: bool,

    /// `--max-depth=<N>`: hard cap on the `pkg.render` composition
    /// depth. `None` → use [`BudgetSnapshot::DEFAULT_MAX_DEPTH`] (16).
    /// Reaching the cap yields a structured `E_RENDER_KCL` error with
    /// the runaway target named.
    pub max_depth: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    PackageK(#[from] PackageKError),

    #[error("failed to read inputs file {path}: {source}")]
    InputsIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse inputs file {path}: {source}")]
    InputsParse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error(transparent)]
    Render(#[from] PackageRenderError),

    #[error("akua.toml at {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: ManifestLoadError,
    },

    #[error("resolving charts.*: {0}")]
    Charts(#[from] ChartResolveError),

    #[error("invalid --timeout `{raw}`: {reason}")]
    InvalidTimeout { raw: String, reason: String },

    #[error("reading cosign public key at {path}: {source}")]
    CosignKeyIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl RenderError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            RenderError::PackageK(PackageKError::Missing { path }) => {
                StructuredError::new(codes::E_PACKAGE_MISSING, "Package.k not found")
                    .with_path(path.display().to_string())
                    .with_suggestion("pass the Package.k path explicitly or cd to its directory")
                    .with_default_docs()
            }
            RenderError::PackageK(PackageKError::Io { path, source }) => {
                StructuredError::new(codes::E_IO, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            RenderError::PackageK(PackageKError::KclEval(msg)) => {
                // Plugin errors flow back through KCL's `__kcl_PanicInfo__`
                // envelope as an opaque string — we can't pattern-match
                // on the typed `PathError` variant here. Sniff for the
                // strict-mode marker the `resolve_in_package` error
                // carries so the CLI surfaces a distinct code + hint
                // instead of the generic `E_RENDER_KCL`.
                match KCL_EVAL_MARKER_TABLE
                    .iter()
                    .find(|(marker, _, _)| msg.contains(marker))
                {
                    Some((_, code, suggestion)) => StructuredError::new(*code, msg.clone())
                        .with_suggestion(*suggestion)
                        .with_default_docs(),
                    None => StructuredError::new(codes::E_RENDER_KCL, msg.clone())
                        .with_default_docs(),
                }
            }
            RenderError::PackageK(PackageKError::InputJson(e)) => {
                StructuredError::new(codes::E_INPUTS_PARSE, e.to_string()).with_default_docs()
            }
            RenderError::PackageK(PackageKError::PathEscape(
                inner @ akua_core::kcl_plugin::PathError::StrictRequiresTypedImport(_),
            )) => StructuredError::new(codes::E_STRICT_UNTYPED_CHART, inner.to_string())
                .with_suggestion(STRICT_SUGGESTION)
                .with_default_docs(),
            RenderError::PackageK(PackageKError::PathEscape(
                inner @ akua_core::kcl_plugin::PathError::Escape { .. },
            )) => StructuredError::new(codes::E_PATH_ESCAPE, inner.to_string())
                .with_suggestion(PATH_ESCAPE_SUGGESTION)
                .with_default_docs(),
            RenderError::PackageK(PackageKError::PathEscape(inner)) => StructuredError::new(
                codes::E_PATH_ESCAPE,
                inner.to_string(),
            )
            .with_suggestion("Plugin paths must resolve inside the Package directory — no absolute paths, no `..` escape, no symlink escape. See docs/security-model.md.")
            .with_default_docs(),
            RenderError::PackageK(other) => {
                StructuredError::new(codes::E_PACKAGE_PARSE, other.to_string()).with_default_docs()
            }
            RenderError::InputsIo { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_INPUTS_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            RenderError::InputsParse { path, source } => {
                StructuredError::new(codes::E_INPUTS_PARSE, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            RenderError::Render(PackageRenderError::Io { path, source }) => {
                StructuredError::new(codes::E_IO, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            RenderError::Render(PackageRenderError::Yaml { index, source }) => {
                StructuredError::new(codes::E_RENDER_YAML, format!("resource #{index}: {source}"))
                    .with_default_docs()
            }
            RenderError::ManifestParse { path, source } => source
                .to_structured()
                .with_path(path.display().to_string()),
            RenderError::Charts(inner) => {
                // Distinguish the two cosign failure modes so agents
                // (and humans) branch on the right thing: consumer-
                // vs publisher-actionable.
                let code = match inner {
                    ChartResolveError::OciFetch {
                        source: akua_core::oci_fetcher::OciFetchError::CosignVerify { .. },
                        ..
                    } => codes::E_COSIGN_VERIFY,
                    ChartResolveError::OciFetch {
                        source: akua_core::oci_fetcher::OciFetchError::CosignSignatureMissing { .. },
                        ..
                    } => codes::E_COSIGN_SIG_MISSING,
                    _ => codes::E_CHART_RESOLVE,
                };
                StructuredError::new(code, inner.to_string()).with_default_docs()
            }
            RenderError::CosignKeyIo { path, source } => {
                StructuredError::new(codes::E_COSIGN_VERIFY, source.to_string())
                    .with_path(path.display().to_string())
                    .with_suggestion(
                        "akua.toml [signing].cosign_public_key must resolve to a PEM-encoded \
                         public key file, relative to the workspace.",
                    )
                    .with_default_docs()
            }
            RenderError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
            RenderError::InvalidTimeout { raw, reason } => {
                StructuredError::new(codes::E_INVALID_FLAG, format!("--timeout `{raw}`: {reason}"))
                    .with_suggestion(
                        "--timeout takes a Go-duration string: 30s, 5m, 1h, 250ms.",
                    )
                    .with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            RenderError::PackageK(PackageKError::Io { .. }) => ExitCode::SystemError,
            RenderError::InputsIo { source, .. }
                if source.kind() != std::io::ErrorKind::NotFound =>
            {
                ExitCode::SystemError
            }
            RenderError::Render(PackageRenderError::Io { .. }) => ExitCode::SystemError,
            RenderError::ManifestParse { source, .. } if source.is_system() => {
                ExitCode::SystemError
            }
            RenderError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &RenderArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, RenderError> {
    let package = PackageK::load(args.package_path)?;
    let resolved_inputs = resolve_inputs_path(args);
    let inputs = load_inputs(resolved_inputs.as_deref())?;
    let charts = resolve_package_charts(args.package_path, args.offline, ctx)?;
    let budget = build_budget(ctx, args.max_depth)?;
    let rendered = render_in_worker(&package, &inputs, &charts, args.strict, budget)?;

    if args.stdout_mode {
        write_multi_doc_yaml(stdout, &rendered.resources).map_err(RenderError::StdoutWrite)?;
        return Ok(ExitCode::Success);
    }

    let summary = render(&rendered, args.out_dir, args.dry_run)?;

    if args.debug {
        // Emit summary + the post-eval resources list as a wrapper
        // envelope. Text mode falls through to the normal rendering
        // (the eval-result dump is JSON-only).
        let envelope = DebugEnvelope {
            summary: &summary,
            eval_result: &rendered.resources,
        };
        emit_output(stdout, ctx, &envelope, |w| {
            write_text(w, &summary, args.dry_run)
        })
        .map_err(RenderError::StdoutWrite)?;
    } else {
        emit_output(stdout, ctx, &summary, |w| {
            write_text(w, &summary, args.dry_run)
        })
        .map_err(RenderError::StdoutWrite)?;
    }
    Ok(ExitCode::Success)
}

/// JSON-only envelope shape under `--debug`. The CLI contract reserves
/// `summary` as the canonical field so JSON consumers can keep their
/// existing parser. `evalResult` is best-effort and may shift between
/// releases — callers shouldn't lock down its schema.
#[derive(serde::Serialize)]
struct DebugEnvelope<'a> {
    summary: &'a RenderSummary,
    #[serde(rename = "evalResult")]
    eval_result: &'a [serde_yaml::Value],
}

/// Thin adapter around `akua_core::package_k::resolve_inputs_path`
/// so `akua render` + `akua dev` share the probe order.
fn resolve_inputs_path(args: &RenderArgs<'_>) -> Option<PathBuf> {
    akua_core::package_k::resolve_inputs_path(args.package_path, args.inputs_path)
}

/// Resolve `[dependencies]` from the Package's sibling `akua.toml`.
/// No `akua.toml` → empty `ResolvedCharts` (Package renders as if it
/// had no deps, matches the pre-Phase-2 behavior). Parse / resolve
/// errors surface as typed CLI errors so agents branch.
///
/// OCI pulls go through a content-addressed cache under
/// `$XDG_CACHE_HOME/akua/oci`; the second render reuses the same
/// unpacked tree. Lockfile digests flow in as `expected_digests` so a
/// drifted tag at the registry fails the render instead of silently
/// picking up new bytes.
fn resolve_package_charts(
    package_path: &Path,
    offline: bool,
    ctx: &Context,
) -> Result<ResolvedCharts, RenderError> {
    let workspace = package_path.parent().unwrap_or(Path::new("."));
    let manifest = match AkuaManifest::load(workspace) {
        Ok(m) => m,
        Err(ManifestLoadError::Missing { .. }) => return Ok(ResolvedCharts::default()),
        Err(source) => {
            return Err(RenderError::ManifestParse {
                path: workspace.join("akua.toml"),
                source,
            });
        }
    };

    let expected_digests = match AkuaLock::load(workspace) {
        Ok(lock) => lock
            .packages
            .into_iter()
            .filter(LockedPackage::is_oci)
            .map(|p| (p.name, p.digest))
            .collect(),
        Err(LockLoadError::Missing { .. }) => Default::default(),
        Err(_) => Default::default(), // lock corruption surfaces via `akua verify`
    };

    let cosign_public_key_pem = load_cosign_public_key(&manifest, workspace)?;
    let opts = ResolverOptions {
        offline,
        cache_root: None,
        expected_digests,
        cosign_public_key_pem,
        // Production-mode replace gate: agent context auto-enables it
        // (CI / container / agent invocation must not honor `replace =
        // { path = "..." }`); humans can also opt in via env var. See
        // CLAUDE.md "`replace` and `path` deps are workspace-local".
        reject_replace: ctx.agent.detected || chart_resolver::replace_rejected_from_env(),
    };
    Ok(chart_resolver::resolve_with_options(
        &manifest, workspace, &opts,
    )?)
}

/// Read the cosign public key referenced by `[signing].cosign_public_key`
/// off disk, relative to `workspace`. Returns `None` when no signing
/// section or no key is configured — signing stays opt-in for
/// back-compat.
fn load_cosign_public_key(
    manifest: &AkuaManifest,
    workspace: &Path,
) -> Result<Option<String>, RenderError> {
    let Some(signing) = manifest.signing.as_ref() else {
        return Ok(None);
    };
    let Some(rel) = signing.cosign_public_key.as_deref() else {
        return Ok(None);
    };
    let key_path = workspace.join(rel);
    let body = std::fs::read_to_string(&key_path).map_err(|source| RenderError::CosignKeyIo {
        path: key_path.clone(),
        source,
    })?;
    Ok(Some(body))
}

/// Build the render's [`BudgetSnapshot`] from `--timeout` (universal
/// flag, parsed via [`akua_core::duration_parse::parse_go_duration`])
/// and `--max-depth` (render-specific). `None` for either field falls
/// back to the renderer's defaults.
fn build_budget(
    ctx: &Context,
    max_depth: Option<usize>,
) -> Result<akua_core::kcl_plugin::BudgetSnapshot, RenderError> {
    let deadline = match ctx.timeout.as_deref() {
        None => None,
        Some(raw) => {
            let dur = akua_core::duration_parse::parse_go_duration(raw).map_err(|reason| {
                RenderError::InvalidTimeout {
                    raw: raw.to_string(),
                    reason,
                }
            })?;
            Some(std::time::Instant::now() + dur)
        }
    };
    Ok(akua_core::kcl_plugin::BudgetSnapshot {
        deadline,
        max_depth: max_depth.unwrap_or(akua_core::kcl_plugin::BudgetSnapshot::DEFAULT_MAX_DEPTH),
    })
}

/// Drive the sandboxed render path. A plugin panic surfaces as
/// `WorkerError::PluginPanic(msg)` and is lifted into
/// `PackageKError::KclEval(msg)` so the existing strict-marker
/// substring match in `to_structured` picks up
/// `E_STRICT_UNTYPED_CHART` — this stringly-typed contract is why
/// every worker failure is collapsed through
/// [`worker_to_render_err`] rather than a typed `From` impl.
pub fn render_in_worker(
    package: &PackageK,
    inputs: &serde_yaml::Value,
    charts: &ResolvedCharts,
    strict: bool,
    budget: akua_core::kcl_plugin::BudgetSnapshot,
) -> Result<akua_core::RenderedPackage, RenderError> {
    use crate::render_worker::{RenderHost, ResourceLimits, WorkerRequest};

    let _scope = akua_core::kcl_plugin::RenderScope::enter_for_render_with_budget(
        &package.path,
        charts,
        strict,
        budget,
    );

    // Helm deps go through the synthetic `charts.<name>` umbrella —
    // materialize as a single tempdir keyed at /charts.
    let charts_tmp = akua_core::stdlib::materialize_charts_if_any(charts).map_err(|e| {
        RenderError::PackageK(PackageKError::KclEval(format!(
            "materializing charts pkg: {e}"
        )))
    })?;

    // Akua-package deps go through a stub umbrella `pkgs.<alias>` —
    // import-only schema re-exports synthesized from upstream's
    // `package.k`. Mirrors the `charts` shape; importing pkgs.<alias>
    // does not run upstream's body (which would otherwise fire its
    // ctx.input() against the consumer's option("input") and panic on
    // schema-shape mismatch).
    let pkgs_tmp = akua_core::stdlib::materialize_pkg_stubs_if_any(charts).map_err(|e| {
        RenderError::PackageK(PackageKError::KclEval(format!(
            "materializing akua-pkg stubs: {e}"
        )))
    })?;

    // KCL ecosystem deps mount as their own ExternalPkg per alias.
    // No tempdir indirection — preopen each resolved root directly at
    // `/kcl-pkgs/<alias>` and tell the worker the alias→path mapping.
    // Akua-package deps are skipped here; they reach the consumer via
    // the `pkgs.<alias>` stub umbrella above instead of their own
    // alias mount (which would expose upstream's full body).
    let mut kcl_preopens: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut kcl_pkgs_request: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (alias, c) in charts.kcl_pkgs() {
        if c.is_akua_package() {
            continue;
        }
        let guest_path = format!("/kcl-pkgs/{alias}");
        kcl_preopens.push((c.abs_path.clone(), guest_path.clone()));
        kcl_pkgs_request.insert(alias.to_string(), guest_path);
    }

    let host = RenderHost::shared().map_err(worker_to_render_err)?;

    let inputs_json: serde_json::Value = serde_json::to_value(inputs)
        .map_err(|e| RenderError::PackageK(PackageKError::KclEval(format!("inputs→json: {e}"))))?;

    let request = WorkerRequest::Render {
        package_filename: package
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "package.k".to_string()),
        source: package.source.clone(),
        inputs: Some(inputs_json),
        charts_pkg_path: charts_tmp.as_ref().map(|_| "/charts".to_string()),
        kcl_pkgs: kcl_pkgs_request,
        pkgs_pkg_path: pkgs_tmp.as_ref().map(|_| "/akua-pkgs".to_string()),
    };

    let response = host
        .invoke_with_deps(
            &request,
            ResourceLimits::default(),
            charts_tmp.as_ref().map(|d| d.path()),
            &kcl_preopens,
            pkgs_tmp.as_ref().map(|d| d.path()),
        )
        .map_err(worker_to_render_err)?;

    let yaml = response
        .into_render_yaml()
        .map_err(|msg| RenderError::PackageK(PackageKError::KclEval(msg)))?;

    let parsed = akua_core::parse_rendered_yaml(&yaml)?;
    // `pkg.render` is a synchronous engine plugin: the worker
    // resolves nested renders inline before returning, so
    // `parsed.resources` is already final.
    Ok(akua_core::RenderedPackage {
        resources: parsed.resources,
    })
}

fn worker_to_render_err(e: crate::render_worker::WorkerError) -> RenderError {
    RenderError::PackageK(PackageKError::KclEval(e.to_string()))
}

/// Thin sandboxed equivalent of `akua_core::eval_source`. Runs
/// inline KCL through the render worker with no inputs and no
/// `charts/` preopen — for REPL-style evaluation where the user
/// hasn't declared dependencies.
pub(crate) fn eval_source_in_worker(
    package_filename: &str,
    source: &str,
) -> Result<String, PackageKError> {
    use crate::render_worker::{RenderHost, ResourceLimits, WorkerRequest};

    let host = RenderHost::shared().map_err(|e| PackageKError::KclEval(e.to_string()))?;
    let request = WorkerRequest::Render {
        package_filename: package_filename.to_string(),
        source: source.to_string(),
        inputs: None,
        charts_pkg_path: None,
        kcl_pkgs: std::collections::BTreeMap::new(),
        pkgs_pkg_path: None,
    };
    let response = host
        .invoke(&request, ResourceLimits::default())
        .map_err(|e| PackageKError::KclEval(e.to_string()))?;
    response.into_render_yaml().map_err(PackageKError::KclEval)
}

fn load_inputs(path: Option<&Path>) -> Result<serde_yaml::Value, RenderError> {
    let Some(path) = path else {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    };
    let bytes = std::fs::read(path).map_err(|e| RenderError::InputsIo {
        path: path.to_path_buf(),
        source: e,
    })?;
    serde_yaml::from_slice(&bytes).map_err(|e| RenderError::InputsParse {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Emit `resources` as a single YAML stream with `---` separators.
/// Used by `--stdout`.
fn write_multi_doc_yaml<W: Write>(
    writer: &mut W,
    resources: &[serde_yaml::Value],
) -> std::io::Result<()> {
    for (i, resource) in resources.iter().enumerate() {
        if i > 0 {
            writeln!(writer, "---")?;
        }
        let yaml = serde_yaml::to_string(resource).map_err(std::io::Error::other)?;
        writer.write_all(yaml.as_bytes())?;
    }
    Ok(())
}

fn write_text<W: Write>(
    writer: &mut W,
    summary: &RenderSummary,
    dry_run: bool,
) -> std::io::Result<()> {
    let verb = if dry_run { "would render" } else { "rendered" };
    writeln!(
        writer,
        "{verb}: {manifests} manifest(s) → {target} ({hash})",
        manifests = summary.manifests,
        target = summary.target.display(),
        hash = summary.hash,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// `to_structured`'s `KclEval` arm sniffs the message for several
    /// markers to recover a typed error code from KCL's opaque plugin-
    /// panic envelope. If any marker were a substring of another, the
    /// order-dependent `if/else if` chain would silently misroute one
    /// of them. Pin all of them as pairwise-disjoint here so a future
    /// edit to any can't break this without tripping the test.
    #[test]
    fn render_error_markers_are_substring_disjoint() {
        let markers = [
            ("STRICT_MARKER", STRICT_MARKER),
            ("ESCAPE_MARKER", ESCAPE_MARKER),
            ("CYCLE_MARKER", CYCLE_MARKER),
            ("DEPTH_BUDGET_MARKER", DEPTH_BUDGET_MARKER),
            ("DEADLINE_BUDGET_MARKER", DEADLINE_BUDGET_MARKER),
        ];
        for (i, (a_name, a)) in markers.iter().enumerate() {
            for (b_name, b) in markers.iter().skip(i + 1) {
                assert!(
                    !a.contains(b) && !b.contains(a),
                    "{a_name} and {b_name} must not be substrings of each other; \
                     got {a_name}={a:?} {b_name}={b:?}"
                );
            }
        }
    }

    /// Round-trip: each marker in `KCL_EVAL_MARKER_TABLE` must map a
    /// `KclEval(prose)` to the table's code via `to_structured`.
    /// Pins the table-driven dispatch against silent drift — if the
    /// matching logic ever inverts or ignores the table, this test
    /// fails.
    #[test]
    fn kcl_eval_markers_route_to_their_codes() {
        for (marker, expected_code, _suggestion) in KCL_EVAL_MARKER_TABLE {
            let err = RenderError::PackageK(PackageKError::KclEval(format!(
                "pkg.render(/tmp/x.k): {marker} (synthetic test prose)"
            )));
            let structured = err.to_structured();
            assert_eq!(
                structured.code, *expected_code,
                "marker {marker:?} should route to {expected_code}, got {}",
                structured.code
            );
        }
    }

    const MINIMAL_PACKAGE: &str = r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "demo"
    data.count: str(input.replicas)
}]
"#;

    fn write_package(tmp: &TempDir, body: &str) -> PathBuf {
        let p = tmp.path().join("Package.k");
        fs::write(&p, body).expect("write");
        p
    }

    fn ctx_json() -> Context {
        Context::json()
    }

    fn args<'a>(pkg: &'a Path, out: &'a Path) -> RenderArgs<'a> {
        RenderArgs {
            package_path: pkg,
            inputs_path: None,
            out_dir: out,
            dry_run: false,
            stdout_mode: false,
            strict: false,
            offline: false,
            debug: false,
            max_depth: None,
        }
    }

    #[test]
    fn run_writes_manifests_and_emits_json_summary() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let out = tmp.path().join("deploy");
        let mut stdout = Vec::new();
        let code = run(&ctx_json(), &args(&pkg, &out), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);

        assert!(out.join("000-configmap-demo.yaml").is_file());

        let text = String::from_utf8(stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(parsed["manifests"], 1);
        assert_eq!(parsed["format"], "raw-manifests");
        assert!(parsed["hash"].as_str().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn dry_run_does_not_write_any_files() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let out = tmp.path().join("deploy");
        let a = RenderArgs {
            dry_run: true,
            ..args(&pkg, &out)
        };
        let mut stdout = Vec::new();
        let code = run(&Context::human(), &a, &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);
        assert!(!out.exists());
        assert!(String::from_utf8(stdout).unwrap().contains("would render"));
    }

    #[test]
    fn stdout_mode_prints_multi_document_yaml() {
        let tmp = TempDir::new().unwrap();
        let body = r#"
input = option("input") or {}

resources = [
    { apiVersion: "v1", kind: "ConfigMap", metadata.name: "a" },
    { apiVersion: "v1", kind: "Service",   metadata.name: "b" },
]
"#;
        let pkg = write_package(&tmp, body);
        let a = RenderArgs {
            stdout_mode: true,
            ..args(&pkg, tmp.path())
        };
        let mut stdout = Vec::new();
        run(&Context::human(), &a, &mut stdout).expect("run");
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("kind: ConfigMap"), "{text}");
        assert!(text.contains("kind: Service"), "{text}");
        assert!(text.contains("---"), "{text}");
        assert!(!tmp.path().join("000-configmap-a.yaml").exists());
    }

    #[test]
    fn inputs_file_threaded_through_to_kcl() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let inputs = tmp.path().join("inputs.yaml");
        fs::write(&inputs, "replicas: 7\n").unwrap();

        let out = tmp.path().join("deploy");
        let a = RenderArgs {
            inputs_path: Some(&inputs),
            ..args(&pkg, &out)
        };
        let mut stdout = Vec::new();
        run(&ctx_json(), &a, &mut stdout).expect("run");

        let cm = fs::read_to_string(out.join("000-configmap-demo.yaml")).unwrap();
        assert!(
            cm.contains("count: '7'") || cm.contains("count: \"7\"") || cm.contains("count: 7"),
            "{cm}"
        );
    }

    #[test]
    fn inputs_file_accepts_json_syntax() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let inputs = tmp.path().join("inputs.json");
        fs::write(&inputs, r#"{"replicas": 5}"#).unwrap();

        let out = tmp.path().join("deploy");
        let a = RenderArgs {
            inputs_path: Some(&inputs),
            ..args(&pkg, &out)
        };
        let mut stdout = Vec::new();
        run(&Context::human(), &a, &mut stdout).expect("run");
        assert!(fs::read_to_string(out.join("000-configmap-demo.yaml"))
            .unwrap()
            .contains('5'));
    }

    #[test]
    fn missing_package_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing.k");
        let a = RenderArgs {
            dry_run: true,
            ..args(&missing, tmp.path())
        };
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn missing_inputs_file_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let missing = tmp.path().join("no-such.yaml");
        let a = RenderArgs {
            inputs_path: Some(&missing),
            dry_run: true,
            ..args(&pkg, tmp.path())
        };
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_INPUTS_MISSING);
    }

    #[test]
    fn malformed_inputs_surfaces_typed_parse_error() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let inputs = tmp.path().join("inputs.yaml");
        fs::write(&inputs, ":::: not yaml ::::").unwrap();
        let a = RenderArgs {
            inputs_path: Some(&inputs),
            dry_run: true,
            ..args(&pkg, tmp.path())
        };
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_INPUTS_PARSE);
    }

    #[test]
    fn kcl_eval_error_surfaces_typed() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, "this is not valid kcl");
        let a = RenderArgs {
            dry_run: true,
            ..args(&pkg, tmp.path())
        };
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_RENDER_KCL);
    }

    // --- inputs auto-discovery --------------------------------------------

    #[test]
    fn auto_discovers_inputs_yaml_when_flag_absent() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        fs::write(tmp.path().join("inputs.yaml"), "replicas: 4\n").unwrap();

        let out_dir = tmp.path().join("out");
        run(&ctx_json(), &args(&pkg, &out_dir), &mut Vec::new()).expect("run");

        let cm = fs::read_to_string(out_dir.join("000-configmap-demo.yaml")).unwrap();
        assert!(
            cm.contains("count: '4'") || cm.contains("count: 4") || cm.contains("count: \"4\""),
            "expected replicas from inputs.yaml; got:\n{cm}"
        );
    }

    #[test]
    fn inputs_yaml_wins_over_inputs_example_yaml() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        fs::write(tmp.path().join("inputs.yaml"), "replicas: 7\n").unwrap();
        fs::write(tmp.path().join("inputs.example.yaml"), "replicas: 99\n").unwrap();

        let out_dir = tmp.path().join("out");
        run(&ctx_json(), &args(&pkg, &out_dir), &mut Vec::new()).expect("run");

        let cm = fs::read_to_string(out_dir.join("000-configmap-demo.yaml")).unwrap();
        assert!(cm.contains('7') && !cm.contains("99"), "{cm}");
    }

    #[test]
    fn falls_back_to_inputs_example_yaml_when_inputs_yaml_absent() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        fs::write(tmp.path().join("inputs.example.yaml"), "replicas: 5\n").unwrap();

        let out_dir = tmp.path().join("out");
        run(&ctx_json(), &args(&pkg, &out_dir), &mut Vec::new()).expect("run");

        let cm = fs::read_to_string(out_dir.join("000-configmap-demo.yaml")).unwrap();
        assert!(cm.contains('5'), "{cm}");
    }

    #[test]
    fn explicit_inputs_flag_overrides_auto_discovery() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        fs::write(tmp.path().join("inputs.yaml"), "replicas: 7\n").unwrap();
        let explicit = tmp.path().join("other.yaml");
        fs::write(&explicit, "replicas: 11\n").unwrap();

        let out_dir = tmp.path().join("out");
        let a = RenderArgs {
            inputs_path: Some(&explicit),
            ..args(&pkg, &out_dir)
        };
        run(&ctx_json(), &a, &mut Vec::new()).expect("run");

        let cm = fs::read_to_string(out_dir.join("000-configmap-demo.yaml")).unwrap();
        assert!(cm.contains("11") && !cm.contains(": 7"), "{cm}");
    }

    #[test]
    fn malformed_auto_discovered_inputs_errors_instead_of_falling_through() {
        // Regression guard: if `inputs.yaml` exists but is malformed,
        // the verb must surface the parse error — not silently fall
        // through to `inputs.example.yaml`. Precedence is "first
        // match wins," not "first valid wins."
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        fs::write(tmp.path().join("inputs.yaml"), ":::: not yaml ::::").unwrap();
        fs::write(tmp.path().join("inputs.example.yaml"), "replicas: 5\n").unwrap();

        let err = run(&Context::human(), &args(&pkg, tmp.path()), &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_INPUTS_PARSE);
    }

    #[test]
    fn offline_flag_threads_through_to_resolver() {
        // Offline mode with no OCI dep in the manifest → render
        // succeeds on the minimal package (no deps to resolve).
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        let a = RenderArgs {
            offline: true,
            dry_run: true,
            ..args(&pkg, tmp.path())
        };
        let code = run(&Context::human(), &a, &mut Vec::new()).expect("render");
        assert_eq!(code, ExitCode::Success);
    }

    #[test]
    fn strict_mode_rejects_raw_string_chart_path() {
        // Package uses helm.template with a raw "./chart" literal —
        // allowed by default (path resolves under Package dir) but
        // must be rejected in strict mode.
        let tmp = TempDir::new().unwrap();
        let chart_dir = tmp.path().join("chart");
        std::fs::create_dir_all(&chart_dir).unwrap();
        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();

        let body = r#"
import akua.helm
resources = helm.template(helm.Template { chart = "./chart" })
"#;
        let pkg = write_package(&tmp, body);
        let a = RenderArgs {
            strict: true,
            dry_run: true,
            ..args(&pkg, tmp.path())
        };
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_STRICT_UNTYPED_CHART);
    }

    #[test]
    fn no_inputs_file_at_all_uses_schema_defaults() {
        let tmp = TempDir::new().unwrap();
        let pkg = write_package(&tmp, MINIMAL_PACKAGE);
        // No inputs.yaml, no inputs.example.yaml, no --inputs.
        let out_dir = tmp.path().join("out");
        run(&Context::human(), &args(&pkg, &out_dir), &mut Vec::new()).expect("run");

        // Schema default `replicas: int = 2` wins.
        let cm = fs::read_to_string(out_dir.join("000-configmap-demo.yaml")).unwrap();
        assert!(cm.contains("count: '2'") || cm.contains("count: 2"), "{cm}");
    }

    #[test]
    fn build_budget_no_timeout_no_max_depth_yields_default() {
        let ctx = Context::human();
        let budget = build_budget(&ctx, None).expect("build_budget");
        assert!(budget.deadline.is_none());
        assert_eq!(
            budget.max_depth,
            akua_core::kcl_plugin::BudgetSnapshot::DEFAULT_MAX_DEPTH
        );
    }

    #[test]
    fn build_budget_propagates_max_depth() {
        let ctx = Context::human();
        let budget = build_budget(&ctx, Some(3)).expect("build_budget");
        assert_eq!(budget.max_depth, 3);
    }

    #[test]
    fn build_budget_parses_timeout_into_deadline() {
        let ctx = Context {
            timeout: Some("100ms".to_string()),
            ..Context::human()
        };
        let before = std::time::Instant::now();
        let budget = build_budget(&ctx, None).expect("build_budget");
        let deadline = budget.deadline.expect("deadline set");
        // 100ms in the future, give or take a few ms for test scheduling.
        let dur = deadline.saturating_duration_since(before);
        assert!(dur.as_millis() >= 90 && dur.as_millis() <= 200, "{dur:?}");
    }

    #[test]
    fn build_budget_invalid_timeout_returns_e_invalid_flag() {
        let ctx = Context {
            timeout: Some("5min".to_string()), // unknown unit
            ..Context::human()
        };
        let err = build_budget(&ctx, None).expect_err("should reject");
        assert_eq!(err.to_structured().code, codes::E_INVALID_FLAG);
    }
}
