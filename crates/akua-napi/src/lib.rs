//! `akua-napi` — Node.js native addon exposing `akua-core` via the
//! Node-API ABI. Loaded by `@akua-dev/sdk` per-platform; covers Node 22+,
//! Bun, and Deno (all three implement Node-API). The wasm32-unknown-
//! unknown bundle stays for browsers + pure-KCL fast path.
//!
//! Scope: thin pass-through bindings. Every function delegates to the
//! matching `akua_cli::verbs::*::run` entry, capturing the `--json`
//! envelope to stdout and parsing it back into a `serde_json::Value`
//! for the JS caller. Zero envelope divergence from the CLI: same
//! bytes, different transport.

#![deny(clippy::all)]

use std::io::Cursor;
use std::path::Path;

use akua_cli::contract::Context;
use akua_cli::verbs;
use akua_core::cli_contract::{ExitCode, StructuredError};
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Routes through `verbs::version::run` so the JSON envelope stays
/// byte-stable with the CLI (`akua version --json`). Picking up
/// future fields the verb adds is automatic — no per-binding shape
/// drift like a `String`-only return would invite.
#[napi]
pub fn version() -> Result<serde_json::Value> {
    invoke_verb(|ctx, stdout| verbs::version::run(ctx, stdout).map_err(into_napi_io))
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiRenderArgs {
    pub package: String,
    pub inputs: Option<String>,
    pub out: String,
    pub dry_run: Option<bool>,
    pub strict: Option<bool>,
    pub offline: Option<bool>,
    /// Wall-clock cap (Go duration, e.g. `"30s"`, `"5m"`). Maps to
    /// the universal `--timeout` flag; on the SDK side, exposed as
    /// `RenderOptions.timeout`.
    pub timeout: Option<String>,
    /// Hard cap on `pkg.render` composition depth. `BudgetSnapshot`
    /// default (16) when omitted.
    pub max_depth: Option<u32>,
}

#[napi]
pub fn render(args: NapiRenderArgs) -> Result<serde_json::Value> {
    let ctx = render_ctx(&args);
    let package_path = Path::new(&args.package);
    let inputs_path = args.inputs.as_deref().map(Path::new);
    let out_dir = Path::new(&args.out);
    let verb_args = verbs::render::RenderArgs {
        package_path,
        inputs_path,
        out_dir,
        dry_run: args.dry_run.unwrap_or(false),
        stdout_mode: false,
        strict: args.strict.unwrap_or(false),
        offline: args.offline.unwrap_or(false),
        debug: false,
        max_depth: args.max_depth.map(|n| n as usize),
    };
    invoke_verb_with(&ctx, |ctx, stdout| {
        verbs::render::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

/// Build the per-render Context. Forwards `timeout` from the
/// JS-side `RenderOptions`; everything else stays at `Context::json`
/// defaults (the SDK always wants structured output).
fn render_ctx(args: &NapiRenderArgs) -> Context {
    Context {
        timeout: args.timeout.clone(),
        ..Context::json()
    }
}

/// Render a Package and return the multi-document YAML directly,
/// bypassing the on-disk write + summary envelope. Mirrors
/// `akua render --stdout`. The SDK uses this for `renderSource()`
/// where the caller wants raw YAML, not a `RenderSummary`.
#[napi]
pub fn render_to_yaml(args: NapiRenderArgs) -> Result<String> {
    let package_path = Path::new(&args.package);
    let inputs_path = args.inputs.as_deref().map(Path::new);
    let out_dir = Path::new(&args.out);
    let verb_args = verbs::render::RenderArgs {
        package_path,
        inputs_path,
        out_dir,
        dry_run: args.dry_run.unwrap_or(false),
        // Critical: stdout_mode short-circuits the file-writing path
        // and emits raw multi-doc YAML to stdout. Same path
        // `akua render --stdout` uses.
        stdout_mode: true,
        strict: args.strict.unwrap_or(false),
        offline: args.offline.unwrap_or(false),
        debug: false,
        max_depth: args.max_depth.map(|n| n as usize),
    };
    let ctx = render_ctx(&args);
    let mut out = Cursor::new(Vec::new());
    verbs::render::run(&ctx, &verb_args, &mut out)
        .map_err(|e| into_napi(e.to_structured(), e.exit_code()))?;
    let bytes = out.into_inner();
    String::from_utf8(bytes)
        .map_err(|e| Error::from_reason(format!("render output not utf-8: {e}")))
}

// ---------------------------------------------------------------------------
// lint / fmt — single-file pure-compute verbs
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiPackageArgs {
    pub package: String,
}

#[napi]
pub fn lint(args: NapiPackageArgs) -> Result<serde_json::Value> {
    let path = Path::new(&args.package);
    let verb_args = verbs::lint::LintArgs { package_path: path };
    invoke_verb(|ctx, stdout| {
        verbs::lint::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

#[napi(object)]
pub struct NapiFmtArgs {
    pub package: String,
    /// `--check`: exit 1 if the file would change; do not write.
    pub check: Option<bool>,
    /// `--stdout`: print the formatted source instead of writing it.
    pub stdout: Option<bool>,
}

#[napi]
pub fn fmt(args: NapiFmtArgs) -> Result<serde_json::Value> {
    let path = Path::new(&args.package);
    let verb_args = verbs::fmt::FmtArgs {
        package_path: path,
        check: args.check.unwrap_or(false),
        stdout_mode: args.stdout.unwrap_or(false),
    };
    invoke_verb(|ctx, stdout| {
        verbs::fmt::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// check — workspace + package together
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiCheckArgs {
    pub workspace: String,
    pub package: Option<String>,
}

#[napi]
pub fn check(args: NapiCheckArgs) -> Result<serde_json::Value> {
    let workspace = Path::new(&args.workspace);
    let pkg_buf;
    let package_path = match &args.package {
        Some(p) => {
            pkg_buf = std::path::PathBuf::from(p);
            pkg_buf.as_path()
        }
        None => {
            pkg_buf = workspace.join("package.k");
            pkg_buf.as_path()
        }
    };
    let verb_args = verbs::check::CheckArgs {
        workspace,
        package_path,
    };
    invoke_verb(|ctx, stdout| {
        verbs::check::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// tree / diff — workspace + chart-comparison verbs
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiWorkspaceArgs {
    pub workspace: String,
}

#[napi]
pub fn tree(args: NapiWorkspaceArgs) -> Result<serde_json::Value> {
    let workspace = Path::new(&args.workspace);
    let verb_args = verbs::tree::TreeArgs { workspace };
    invoke_verb(|ctx, stdout| {
        verbs::tree::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

#[napi(object)]
pub struct NapiDiffArgs {
    pub before: String,
    pub after: String,
}

#[napi]
pub fn diff(args: NapiDiffArgs) -> Result<serde_json::Value> {
    let before = Path::new(&args.before);
    let after = Path::new(&args.after);
    let verb_args = verbs::diff::DiffArgs { before, after };
    invoke_verb(|ctx, stdout| {
        verbs::diff::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// export — JSON Schema / OpenAPI emit
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiExportArgs {
    pub package: String,
    /// `"json-schema"` (default) or `"openapi"`.
    pub format: Option<String>,
    /// When set, write the schema to this file instead of stdout. The
    /// CLI verb writes the file AND prints a confirmation; we capture
    /// only the JSON envelope.
    pub out: Option<String>,
}

#[napi]
pub fn export(args: NapiExportArgs) -> Result<serde_json::Value> {
    let package_path = Path::new(&args.package);
    let format = match args.format.as_deref() {
        None | Some("json-schema") => verbs::export::ExportFormat::JsonSchema,
        Some("openapi") => verbs::export::ExportFormat::Openapi,
        Some(other) => {
            return Err(Error::from_reason(format!(
                "unknown format `{other}` (expected `json-schema` or `openapi`)"
            )))
        }
    };
    let out_path;
    let out: Option<&Path> = match &args.out {
        Some(p) => {
            out_path = std::path::PathBuf::from(p);
            Some(out_path.as_path())
        }
        None => None,
    };
    let verb_args = verbs::export::ExportArgs {
        package_path,
        format,
        out,
    };
    invoke_verb(|ctx, stdout| {
        verbs::export::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// inspect — Package or tarball introspection
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct NapiInspectArgs {
    /// On-disk Package directory or `package.k`. Mutually exclusive
    /// with `tarball`.
    pub package: Option<String>,
    /// `.tar.gz` Package artifact (e.g. from `akua pack`). Mutually
    /// exclusive with `package`.
    pub tarball: Option<String>,
}

#[napi]
pub fn inspect(args: NapiInspectArgs) -> Result<serde_json::Value> {
    let target = match (args.package.as_deref(), args.tarball.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(Error::from_reason(
                "inspect: pass either `package` or `tarball`, not both",
            ));
        }
        (None, None) => {
            return Err(Error::from_reason(
                "inspect: pass either `package` or `tarball`",
            ));
        }
        (Some(p), None) => verbs::inspect::InspectTarget::Package(Path::new(p)),
        (None, Some(t)) => verbs::inspect::InspectTarget::Tarball(Path::new(t)),
    };
    let verb_args = verbs::inspect::InspectArgs { target };
    invoke_verb(|ctx, stdout| {
        verbs::inspect::run(ctx, &verb_args, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// verify — workspace lockfile ↔ manifest integrity
// ---------------------------------------------------------------------------

#[napi]
pub fn verify(args: NapiWorkspaceArgs) -> Result<serde_json::Value> {
    let workspace = Path::new(&args.workspace);
    invoke_verb(|ctx, stdout| {
        verbs::verify::run(ctx, workspace, stdout)
            .map_err(|e| into_napi(e.to_structured(), e.exit_code()))
    })
}

// ---------------------------------------------------------------------------
// whoami — agent-context introspection
// ---------------------------------------------------------------------------

#[napi]
pub fn whoami() -> Result<serde_json::Value> {
    invoke_verb(|ctx, stdout| verbs::whoami::run(ctx, stdout).map_err(into_napi_io))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn invoke_verb<F>(run: F) -> Result<serde_json::Value>
where
    F: FnOnce(&Context, &mut Cursor<Vec<u8>>) -> Result<ExitCode>,
{
    invoke_verb_with(&Context::json(), run)
}

fn invoke_verb_with<F>(ctx: &Context, run: F) -> Result<serde_json::Value>
where
    F: FnOnce(&Context, &mut Cursor<Vec<u8>>) -> Result<ExitCode>,
{
    let mut stdout = Cursor::new(Vec::new());
    let exit = run(ctx, &mut stdout)?;
    let bytes = stdout.into_inner();
    if bytes.is_empty() {
        // Every shipping verb writes a JSON envelope under
        // Context::json(). Empty stdout means a verb diverged from
        // that contract — fail loudly so the gap is visible to
        // tests and to JS consumers, not silently masked by a
        // synthetic envelope they couldn't parse anyway.
        return Err(Error::from_reason(format!(
            "akua verb returned exit={exit:?} with empty stdout — every json-mode verb must write an envelope"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        Error::from_reason(format!(
            "verb produced non-JSON output (exit={exit:?}): {e}\n\nbytes: {}",
            String::from_utf8_lossy(&bytes)
        ))
    })
}

/// Convert a verb's [`StructuredError`] + [`ExitCode`] into a napi
/// `Error` that preserves both the structured `code` (for fine-grain
/// matching) and the numeric exit code (for SDK error-class routing
/// — `AkuaUserError` / `AkuaSystemError` / etc.). Same envelope the
/// CLI emits to stderr, plus the `exit_code` numeric from the verb.
/// Without this, every JS-side error would collapse to the generic
/// `AkuaError` and lose typed routing.
fn into_napi(structured: StructuredError, exit_code: ExitCode) -> Error {
    let mut body = match serde_json::to_value(&structured) {
        Ok(v) => v,
        Err(_) => return Error::from_reason(structured.message),
    };
    if let Some(obj) = body.as_object_mut() {
        obj.insert("exit_code".to_string(), serde_json::json!(exit_code as i32));
    }
    Error::from_reason(body.to_string())
}

/// Fallback for verbs whose `run()` returns a non-structured error
/// (e.g. `whoami` and `version` return `std::io::Result<ExitCode>`).
/// No structured code to preserve; the message reaches JS as the
/// generic `Error.message`.
fn into_napi_io<E: std::fmt::Display>(err: E) -> Error {
    Error::from_reason(err.to_string())
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
//
// Bindings are thin pass-throughs to `verbs::*::run`. Tests here focus
// on what THIS layer does that the verb tests don't cover:
//   - Napi*Args → verb args translation
//   - JSON envelope produced by `invoke_verb` (non-empty, parseable)
//   - structured-error envelope (`into_napi`) augmented with `exit_code`
//
// Run with `cargo test -p akua-napi`. Fixtures use a tempdir + the
// minimal-workspace shape (akua.toml + package.k) the SDK tests use.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const MINIMAL_PACKAGE_K: &str = r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "smoke"
    data.count: str(input.replicas)
}]
"#;

    const MINIMAL_AKUA_TOML: &str = r#"[package]
name = "napi-test"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
"#;

    fn scratch_workspace() -> PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        fs::write(dir.join("akua.toml"), MINIMAL_AKUA_TOML).unwrap();
        fs::write(dir.join("package.k"), MINIMAL_PACKAGE_K).unwrap();
        dir
    }

    #[test]
    fn version_returns_object_with_version_field() {
        let v = version().unwrap();
        assert!(v.is_object(), "version must return an object envelope");
        assert!(v.get("version").is_some(), "envelope missing `version`");
    }

    #[test]
    fn whoami_returns_agent_context_envelope() {
        let v = whoami().unwrap();
        assert!(v.is_object());
        assert!(v.get("agent_context").is_some(), "missing agent_context");
        assert!(v.get("version").is_some(), "missing version");
    }

    #[test]
    fn lint_returns_issues_array() {
        let ws = scratch_workspace();
        let v = lint(NapiPackageArgs {
            package: ws.join("package.k").to_string_lossy().into_owned(),
        })
        .unwrap();
        assert!(v.is_object());
        assert!(
            v.get("issues").is_some_and(|x| x.is_array()),
            "lint envelope must carry an `issues` array"
        );
    }

    #[test]
    fn fmt_check_mode_does_not_modify_file() {
        let ws = scratch_workspace();
        let pkg = ws.join("package.k");
        let original = fs::read_to_string(&pkg).unwrap();
        let v = fmt(NapiFmtArgs {
            package: pkg.to_string_lossy().into_owned(),
            check: Some(true),
            stdout: Some(false),
        })
        .unwrap();
        assert!(v.is_object());
        assert!(v.get("files").is_some_and(|x| x.is_array()));
        // --check must not write back even if formatted form differs.
        assert_eq!(fs::read_to_string(&pkg).unwrap(), original);
    }

    #[test]
    fn check_returns_status_and_checks_array() {
        let ws = scratch_workspace();
        let v = check(NapiCheckArgs {
            workspace: ws.to_string_lossy().into_owned(),
            package: None,
        })
        .unwrap();
        assert!(v.is_object());
        let status = v.get("status").and_then(|s| s.as_str()).unwrap();
        assert!(matches!(status, "ok" | "fail"));
        assert!(v.get("checks").is_some_and(|x| x.is_array()));
    }

    #[test]
    fn tree_returns_package_and_dependencies_envelope() {
        let ws = scratch_workspace();
        let v = tree(NapiWorkspaceArgs {
            workspace: ws.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert!(v.is_object());
        assert!(v.get("package").is_some());
        let deps = v.get("dependencies").unwrap();
        assert!(deps.is_array());
        // Minimal workspace declares no deps.
        assert_eq!(deps.as_array().unwrap().len(), 0);
    }

    #[test]
    fn diff_two_empty_dirs_reports_no_changes() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let v = diff(NapiDiffArgs {
            before: a.path().to_string_lossy().into_owned(),
            after: b.path().to_string_lossy().into_owned(),
        })
        .unwrap();
        assert!(v.is_object());
        assert_eq!(v["added"].as_array().unwrap().len(), 0);
        assert_eq!(v["removed"].as_array().unwrap().len(), 0);
        assert_eq!(v["changed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn diff_added_file_surfaces_in_added_bucket() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(b.path().join("new.yaml"), "hi\n").unwrap();
        let v = diff(NapiDiffArgs {
            before: a.path().to_string_lossy().into_owned(),
            after: b.path().to_string_lossy().into_owned(),
        })
        .unwrap();
        let added: Vec<&str> = v["added"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(
            added.contains(&"new.yaml"),
            "added bucket missing new.yaml: {v}"
        );
    }

    #[test]
    fn export_returns_format_and_schema_envelope() {
        let ws = scratch_workspace();
        let v = export(NapiExportArgs {
            package: ws.join("package.k").to_string_lossy().into_owned(),
            format: None,
            out: None,
        })
        .unwrap();
        assert!(v.is_object());
        assert_eq!(v["format"].as_str().unwrap(), "json-schema");
        let schema = &v["schema"];
        assert!(schema.is_object(), "schema must be an object");
        // The default output is JSON Schema 2020-12.
        assert!(
            schema["$schema"]
                .as_str()
                .map(|s| s.contains("2020-12"))
                .unwrap_or(false),
            "expected JSON Schema 2020-12 dialect: {v}"
        );
    }

    #[test]
    fn export_openapi_format_yields_openapi_3_1() {
        let ws = scratch_workspace();
        let v = export(NapiExportArgs {
            package: ws.join("package.k").to_string_lossy().into_owned(),
            format: Some("openapi".to_string()),
            out: None,
        })
        .unwrap();
        assert_eq!(v["format"].as_str().unwrap(), "openapi");
        assert_eq!(v["schema"]["openapi"].as_str().unwrap(), "3.1.0");
    }

    #[test]
    fn inspect_package_mode_reports_kind_package() {
        let ws = scratch_workspace();
        let v = inspect(NapiInspectArgs {
            package: Some(ws.join("package.k").to_string_lossy().into_owned()),
            tarball: None,
        })
        .unwrap();
        assert_eq!(v["kind"].as_str().unwrap(), "package");
        assert!(v["options"].is_array());
    }

    #[test]
    fn verify_missing_lockfile_returns_structured_error() {
        let ws = scratch_workspace();
        let result = verify(NapiWorkspaceArgs {
            workspace: ws.to_string_lossy().into_owned(),
        });
        // Without a lockfile the verb returns E_LOCK_MISSING. The
        // binding routes this through `into_napi`, which embeds the
        // structured envelope into the napi error message and adds
        // `exit_code`. JS-side `parseNapiError` parses it back.
        let err = result.expect_err("expected E_LOCK_MISSING");
        let msg = err.reason.to_string();
        let envelope: serde_json::Value = serde_json::from_str(&msg)
            .unwrap_or_else(|e| panic!("napi error must be JSON: {e}; raw: {msg}"));
        assert_eq!(envelope["code"].as_str().unwrap(), "E_LOCK_MISSING");
        // The exit_code augmentation is what `into_napi` adds on top
        // of the verb's StructuredError — proves the binding ran.
        assert_eq!(envelope["exit_code"].as_i64().unwrap(), 1);
    }

    #[test]
    fn into_napi_carries_structured_envelope_plus_exit_code() {
        let structured = StructuredError::new("E_TEST", "synthetic");
        let err = into_napi(structured, ExitCode::UserError);
        let body: serde_json::Value = serde_json::from_str(&err.reason).unwrap();
        assert_eq!(body["code"].as_str().unwrap(), "E_TEST");
        assert_eq!(body["message"].as_str().unwrap(), "synthetic");
        assert_eq!(
            body["exit_code"].as_i64().unwrap(),
            ExitCode::UserError as i64
        );
    }
}
