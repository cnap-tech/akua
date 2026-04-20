//! `akua check` — fast workspace check: parse akua.toml + akua.lock,
//! lint the Package.k. No execution, no writes.
//!
//! Spec: per CLAUDE.md "fast syntax / type / dep check, no execution".

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{lint_kcl, AkuaLock, AkuaManifest, LintIssue, LockLoadError, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct CheckArgs<'a> {
    /// Workspace root. `akua.toml` (required) and `akua.lock` (optional)
    /// are looked up here.
    pub workspace: &'a Path,

    /// Path to the Package.k to lint. Relative paths resolve against
    /// `workspace`.
    pub package_path: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckOutput {
    pub status: &'static str,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckResult {
    /// Short label for the check: `"manifest"`, `"lockfile"`,
    /// `"package"`.
    pub name: &'static str,

    pub ok: bool,

    /// One-line error from the failing check; absent when `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Per-file issues from linting the Package.k. Other check kinds
    /// leave this empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<LintIssue>,
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
    let mut checks = Vec::with_capacity(3);

    checks.push(check_manifest(args.workspace));

    // Lockfile is optional — a fresh package may have zero declared
    // deps and no lockfile yet. Absent → pass with an "ok" result.
    if args.workspace.join("akua.lock").exists() {
        checks.push(check_lockfile(args.workspace));
    }

    let pkg_path = resolve_package_path(args.workspace, args.package_path);
    checks.push(check_package(&pkg_path));

    let status = if checks.iter().all(|c| c.ok) { "ok" } else { "fail" };

    let output = CheckOutput { status, checks };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(CheckError::StdoutWrite)?;

    Ok(if output.status == "ok" {
        ExitCode::Success
    } else {
        ExitCode::UserError
    })
}

fn resolve_package_path(workspace: &Path, package: &Path) -> PathBuf {
    if package.is_absolute() {
        package.to_path_buf()
    } else {
        workspace.join(package)
    }
}

fn check_manifest(workspace: &Path) -> CheckResult {
    match AkuaManifest::load(workspace) {
        Ok(_) => ok("manifest"),
        Err(e) => fail("manifest", manifest_error_message(&e)),
    }
}

fn manifest_error_message(e: &ManifestLoadError) -> String {
    match e {
        ManifestLoadError::Missing { path } => {
            format!("akua.toml not found at {}", path.display())
        }
        ManifestLoadError::Io { path, source } => {
            format!("i/o at {}: {source}", path.display())
        }
        ManifestLoadError::Parse { path, source } => {
            format!("parse error at {}: {source}", path.display())
        }
    }
}

fn check_lockfile(workspace: &Path) -> CheckResult {
    match AkuaLock::load(workspace) {
        Ok(_) => ok("lockfile"),
        Err(e) => fail("lockfile", lockfile_error_message(&e)),
    }
}

fn lockfile_error_message(e: &LockLoadError) -> String {
    match e {
        LockLoadError::Missing { path } => format!("akua.lock not found at {}", path.display()),
        LockLoadError::Io { path, source } => format!("i/o at {}: {source}", path.display()),
        LockLoadError::Parse { path, source } => {
            format!("parse error at {}: {source}", path.display())
        }
    }
}

fn check_package(path: &Path) -> CheckResult {
    if !path.exists() {
        return fail("package", format!("{} not found", path.display()));
    }
    match lint_kcl(path) {
        Ok(issues) if issues.is_empty() => ok("package"),
        Ok(issues) => CheckResult {
            name: "package",
            ok: false,
            error: Some(format!("{} lint issue(s)", issues.len())),
            issues,
        },
        Err(e) => fail("package", e.to_string()),
    }
}

fn ok(name: &'static str) -> CheckResult {
    CheckResult {
        name,
        ok: true,
        error: None,
        issues: Vec::new(),
    }
}

fn fail(name: &'static str, error: String) -> CheckResult {
    CheckResult {
        name,
        ok: false,
        error: Some(error),
        issues: Vec::new(),
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
outputs = [{ kind: "RawManifests", target: "./" }]
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
