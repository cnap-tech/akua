//! `akua render` — execute a Package against inputs and write raw YAML manifests.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua render` section.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::contract::{emit_output, Context};
use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::mod_file::ManifestLoadError;
use akua_core::{
    chart_resolver, package_k::PackageKError, render, AkuaManifest, ChartResolveError, PackageK,
    PackageRenderError, RenderSummary, ResolvedCharts,
};

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
                StructuredError::new(codes::E_RENDER_KCL, msg.clone()).with_default_docs()
            }
            RenderError::PackageK(PackageKError::InputJson(e)) => {
                StructuredError::new(codes::E_INPUTS_PARSE, e.to_string()).with_default_docs()
            }
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
                StructuredError::new(codes::E_CHART_RESOLVE, inner.to_string())
                    .with_default_docs()
            }
            RenderError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
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
            RenderError::ManifestParse { source, .. } if source.is_system() => ExitCode::SystemError,
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
    let charts = resolve_package_charts(args.package_path)?;
    let rendered = package.render_with_charts(&inputs, &charts)?;

    if args.stdout_mode {
        write_multi_doc_yaml(stdout, &rendered.resources).map_err(RenderError::StdoutWrite)?;
        return Ok(ExitCode::Success);
    }

    let summary = render(&rendered, args.out_dir, args.dry_run)?;

    emit_output(stdout, ctx, &summary, |w| {
        write_text(w, &summary, args.dry_run)
    })
    .map_err(RenderError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

/// When the caller passes `--inputs`, honour it verbatim. Otherwise
/// probe for the two conventional defaults (alongside the
/// `package.k`): `inputs.yaml` first, then `inputs.example.yaml` as a
/// fallback so the freshly-scaffolded `akua init` output renders
/// without the user having to pass `--inputs` manually. Returns
/// `None` when nothing is found; `load_inputs` then produces an empty
/// mapping.
fn resolve_inputs_path(args: &RenderArgs<'_>) -> Option<PathBuf> {
    if let Some(p) = args.inputs_path {
        return Some(p.to_path_buf());
    }
    let package_dir = args.package_path.parent().unwrap_or(Path::new("."));
    for candidate in ["inputs.yaml", "inputs.example.yaml"] {
        let probe = package_dir.join(candidate);
        if probe.is_file() {
            return Some(probe);
        }
    }
    None
}

/// Resolve `[dependencies]` from the Package's sibling `akua.toml`.
/// No `akua.toml` → empty `ResolvedCharts` (Package renders as if it
/// had no deps, matches the pre-Phase-2 behavior). Parse / resolve
/// errors surface as typed CLI errors so agents branch.
fn resolve_package_charts(package_path: &Path) -> Result<ResolvedCharts, RenderError> {
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
    Ok(chart_resolver::resolve(&manifest, workspace)?)
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

        let err = run(&Context::human(), &args(&pkg, tmp.path()), &mut Vec::new())
            .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_INPUTS_PARSE);
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
}
