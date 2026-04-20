//! `akua lint` — KCL parse-only validation of a Package.k.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua lint` section.
//!
//! Parse-only: catches syntax errors and import resolution failures
//! without executing the program. Execution errors (schema validation,
//! unresolved options) surface through `akua render --dry-run`.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{lint_kcl, LintIssue, PackageKError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct LintArgs<'a> {
    pub package_path: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LintOutput {
    /// `"ok"` when no issues reported; `"fail"` otherwise.
    pub status: &'static str,

    pub issues: Vec<LintIssue>,
}

impl LintOutput {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Kcl(#[from] PackageKError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl LintError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            LintError::Io { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            LintError::Kcl(e) => {
                StructuredError::new(codes::E_LINT_FAIL, e.to_string()).with_default_docs()
            }
            LintError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            LintError::Io { source, .. } if source.kind() != std::io::ErrorKind::NotFound => {
                ExitCode::SystemError
            }
            LintError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &LintArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, LintError> {
    // NotFound surfaces via the Io branch even though `lint_kcl` doesn't
    // open the file itself — probe up-front so the error maps cleanly.
    if !args.package_path.exists() {
        return Err(LintError::Io {
            path: args.package_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found", args.package_path.display()),
            ),
        });
    }

    let issues = lint_kcl(args.package_path)?;
    let output = LintOutput {
        status: if issues.is_empty() { "ok" } else { "fail" },
        issues,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(LintError::StdoutWrite)?;

    Ok(if output.is_ok() {
        ExitCode::Success
    } else {
        ExitCode::UserError
    })
}

fn write_text<W: Write>(writer: &mut W, output: &LintOutput) -> std::io::Result<()> {
    if output.is_ok() {
        writeln!(writer, "ok: no lint issues")?;
        return Ok(());
    }
    writeln!(writer, "fail: {} issue(s)", output.issues.len())?;
    for issue in &output.issues {
        let location = match (&issue.file, issue.line, issue.column) {
            (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}"),
            (Some(f), Some(l), None) => format!("{f}:{l}"),
            (Some(f), None, None) => f.clone(),
            _ => String::new(),
        };
        let prefix = if location.is_empty() {
            String::new()
        } else {
            format!("  at {location}")
        };
        writeln!(writer, "  [{}] {}: {}{}", issue.level, issue.code, issue.message, prefix)?;
    }
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

    const VALID: &str = r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = []
outputs = [{ kind: "RawManifests", target: "./" }]
"#;

    const BROKEN: &str = "schema Input:\n  !!!\n";

    fn write(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("package.k");
        fs::write(&p, body).unwrap();
        (tmp, p)
    }

    fn args(path: &Path) -> LintArgs<'_> {
        LintArgs { package_path: path }
    }

    #[test]
    fn valid_package_returns_status_ok_and_exit_success() {
        let (_tmp, path) = write(VALID);
        let mut stdout = Vec::new();
        let code = run(&Context::human(), &args(&path), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);
        assert!(String::from_utf8(stdout).unwrap().contains("ok: no lint issues"));
    }

    #[test]
    fn broken_package_returns_status_fail_and_exit_user_error() {
        let (_tmp, path) = write(BROKEN);
        let mut stdout = Vec::new();
        let code = run(&Context::human(), &args(&path), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::UserError);
        assert!(String::from_utf8(stdout).unwrap().contains("fail:"));
    }

    #[test]
    fn json_output_lists_issues_with_position_info() {
        let (_tmp, path) = write(BROKEN);
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &args(&path), &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["status"], "fail");
        let issues = parsed["issues"].as_array().expect("issues array");
        assert!(!issues.is_empty());
        // At least one issue carries a `message`.
        assert!(!issues[0]["message"].as_str().unwrap().is_empty());
    }

    #[test]
    fn missing_file_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.k");
        let err = run(&Context::human(), &args(&missing), &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }
}
