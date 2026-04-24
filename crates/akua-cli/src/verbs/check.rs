//! `akua check` — fast workspace check: parse akua.toml + akua.lock,
//! lint the Package.k. No execution, no writes.
//!
//! Pure logic lives in `akua_core::check`; this verb is a thin CLI
//! envelope that reads files + delegates + emits JSON. The SDK
//! reaches the same `akua_core::check::check_from_sources` through
//! the WASM bindings — shared single source of truth.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::check_from_sources;

use crate::contract::{emit_output, Context};

/// Re-export so external callers that were importing
/// `akua_cli::verbs::check::{CheckOutput, CheckResult}` (e.g. the
/// SDK bundle export test) keep compiling.
pub use akua_core::check::{CheckOutput, CheckResult};

#[derive(Debug, Clone)]
pub struct CheckArgs<'a> {
    /// Workspace root. `akua.toml` (required) and `akua.lock`
    /// (optional) are looked up here.
    pub workspace: &'a Path,

    /// Path to the Package.k to lint. Relative paths resolve against
    /// `workspace`.
    pub package_path: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl CheckError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            CheckError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::SystemError
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &CheckArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, CheckError> {
    // Read whatever files exist on disk; delegate the gates to
    // akua-core. `akua.lock` is optional (a fresh workspace may have
    // zero deps); `akua.toml` + `package.k` are required — the CLI
    // surfaces their absence as an explicit failing CheckResult so
    // the verdict is `fail` not `ok`.
    let manifest_path = args.workspace.join("akua.toml");
    let lock_path = args.workspace.join("akua.lock");
    let pkg_path = resolve_package_path(args.workspace, args.package_path);

    let manifest_source = read_opt(&manifest_path);
    let lock_source = read_opt(&lock_path);
    let pkg_source = read_opt(&pkg_path);
    let pkg_filename = pkg_path.to_string_lossy().into_owned();

    let mut output = check_from_sources(
        manifest_source.as_deref(),
        lock_source.as_deref(),
        pkg_source
            .as_deref()
            .map(|src| (pkg_filename.as_str(), src)),
    );

    // Required-file gates the CLI adds on top of the core check:
    if manifest_source.is_none() {
        output.checks.insert(
            0,
            CheckResult {
                name: "manifest",
                ok: false,
                error: Some(format!("{} not found", manifest_path.display())),
                issues: Vec::new(),
            },
        );
    }
    if pkg_source.is_none() {
        output.checks.push(CheckResult {
            name: "package",
            ok: false,
            error: Some(format!("{} not found", pkg_path.display())),
            issues: Vec::new(),
        });
    }
    output.status = if output.checks.iter().all(|c| c.ok) {
        "ok"
    } else {
        "fail"
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(CheckError::StdoutWrite)?;

    Ok(if output.status == "ok" {
        ExitCode::Success
    } else {
        ExitCode::UserError
    })
}

fn read_opt(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn resolve_package_path(workspace: &Path, package: &Path) -> PathBuf {
    if package.is_absolute() {
        package.to_path_buf()
    } else {
        workspace.join(package)
    }
}

fn write_text<W: Write>(writer: &mut W, output: &CheckOutput) -> std::io::Result<()> {
    for c in &output.checks {
        let marker = if c.ok { "✓" } else { "✗" };
        match &c.error {
            Some(msg) => writeln!(writer, "  {marker} {}: {msg}", c.name)?,
            None => writeln!(writer, "  {marker} {}", c.name)?,
        }
        for issue in &c.issues {
            writeln!(
                writer,
                "      [{}] {}: {}",
                issue.level, issue.code, issue.message
            )?;
        }
    }
    writeln!(writer, "{}", output.status)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const MANIFEST: &str = r#"
[package]
name    = "check-test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
"#;

    const PACKAGE_K: &str = r#"
schema Input:
    x: int = 1

input: Input = option("input") or Input {}

resources = []
"#;

    fn workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("akua.toml"), MANIFEST).unwrap();
        fs::write(tmp.path().join("package.k"), PACKAGE_K).unwrap();
        tmp
    }

    fn args<'a>(ws: &'a Path, pkg: &'a Path) -> CheckArgs<'a> {
        CheckArgs {
            workspace: ws,
            package_path: pkg,
        }
    }

    #[test]
    fn clean_workspace_passes_all_checks() {
        let ws = workspace();
        let pkg = Path::new("package.k");
        let mut stdout = Vec::new();
        let code = run(&Context::human(), &args(ws.path(), pkg), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);
        assert!(String::from_utf8(stdout).unwrap().contains("ok"));
    }

    #[test]
    fn missing_manifest_fails_check() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.k"), PACKAGE_K).unwrap();
        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &args(tmp.path(), Path::new("package.k")),
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::UserError);
    }

    #[test]
    fn lockfile_skipped_when_absent() {
        // Default workspace has no lockfile — check still passes.
        let ws = workspace();
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &args(ws.path(), Path::new("package.k")), &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        let names: Vec<&str> = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["manifest", "package"]);
    }

    #[test]
    fn lockfile_included_when_present() {
        let ws = workspace();
        fs::write(ws.path().join("akua.lock"), "version = 1\n").unwrap();
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &args(ws.path(), Path::new("package.k")), &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        let names: Vec<&str> = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["manifest", "lockfile", "package"]);
    }

    #[test]
    fn malformed_package_surfaces_via_issues() {
        let ws = workspace();
        fs::write(ws.path().join("package.k"), "schema X:\n  !!!\n").unwrap();
        let ctx = Context::json();
        let mut stdout = Vec::new();
        let code = run(&ctx, &args(ws.path(), Path::new("package.k")), &mut stdout)
            .expect("run");
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        let pkg = parsed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "package")
            .expect("package entry");
        assert_eq!(pkg["ok"], false);
        assert!(!pkg["issues"].as_array().unwrap().is_empty());
    }
}
