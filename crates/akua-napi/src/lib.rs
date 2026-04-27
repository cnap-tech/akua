//! `akua-napi` — Node.js native addon exposing `akua-core` via the
//! Node-API ABI. Loaded by `@akua/sdk` per-platform; covers Node 22+,
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
}

#[napi]
pub fn render(args: NapiRenderArgs) -> Result<serde_json::Value> {
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
    };
    invoke_verb(|ctx, stdout| verbs::render::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    };
    let ctx = Context::json();
    let mut out = Cursor::new(Vec::new());
    verbs::render::run(&ctx, &verb_args, &mut out)
        .map_err(|e| into_napi(e.to_structured(), e.exit_code()))?;
    let bytes = out.into_inner();
    String::from_utf8(bytes).map_err(|e| Error::from_reason(format!("render output not utf-8: {e}")))
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
    invoke_verb(|ctx, stdout| verbs::lint::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    invoke_verb(|ctx, stdout| verbs::fmt::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    invoke_verb(|ctx, stdout| verbs::check::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    invoke_verb(|ctx, stdout| verbs::tree::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    invoke_verb(|ctx, stdout| verbs::diff::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    invoke_verb(|ctx, stdout| verbs::export::run(ctx, &verb_args, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
}

// ---------------------------------------------------------------------------
// verify — workspace lockfile ↔ manifest integrity
// ---------------------------------------------------------------------------

#[napi]
pub fn verify(args: NapiWorkspaceArgs) -> Result<serde_json::Value> {
    let workspace = Path::new(&args.workspace);
    invoke_verb(|ctx, stdout| verbs::verify::run(ctx, workspace, stdout).map_err(|e| into_napi(e.to_structured(), e.exit_code())))
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
    let ctx = Context::json();
    let mut stdout = Cursor::new(Vec::new());
    let exit = run(&ctx, &mut stdout)?;
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
        obj.insert(
            "exit_code".to_string(),
            serde_json::json!(exit_code as i32),
        );
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
