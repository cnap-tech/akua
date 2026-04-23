//! `akua test` — discover + run KCL test files in a workspace.
//!
//! Convention: any file matching `test_*.k` or `*_test.k` is a test.
//! Each is evaluated via the standard PackageK loader; KCL's
//! `check:` blocks + `assert` statements fire at eval-time, so no
//! harness sits in the middle.
//!
//! Exit 0 = all passed (or no tests found); exit 1 with
//! `E_TEST_FAIL` on any failure.

use std::io::Write;
use std::path::Path;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::test_runner::{self, TestReport};

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct TestArgs<'a> {
    /// Root under which test files are discovered.
    pub workspace: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error(transparent)]
    Run(#[from] test_runner::TestRunError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl TestError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            TestError::Run(inner) => {
                StructuredError::new(codes::E_IO, inner.to_string()).with_default_docs()
            }
            TestError::StdoutWrite(e) => {
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
    args: &TestArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, TestError> {
    let report = test_runner::run(args.workspace)?;

    // Exit code is determined by the report; emit output first so
    // callers see the structured verdict regardless of the code.
    let exit = match report.status {
        "fail" => ExitCode::UserError,
        _ => ExitCode::Success,
    };

    emit_output(stdout, ctx, &report, |w| write_text(w, &report))
        .map_err(TestError::StdoutWrite)?;
    Ok(exit)
}

fn write_text<W: Write>(w: &mut W, report: &TestReport) -> std::io::Result<()> {
    match report.status {
        "empty" => {
            writeln!(w, "no tests found (*test*.k)")?;
            return Ok(());
        }
        "ok" => {
            writeln!(w, "test: {} passed", report.total)?;
        }
        "fail" => {
            writeln!(
                w,
                "test: {} passed, {} failed of {}",
                report.passed, report.failed, report.total,
            )?;
        }
        _ => {
            writeln!(w, "test: status={}", report.status)?;
        }
    }
    for t in &report.tests {
        let marker = if t.status == "pass" { "ok" } else { "FAIL" };
        writeln!(w, "  {marker} {}", t.file.display())?;
        if !t.message.is_empty() {
            // Indent multi-line error messages so the per-test
            // block is visually grouped under its filename.
            for line in t.message.lines() {
                writeln!(w, "      {line}")?;
            }
        }
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

    fn workspace() -> TempDir {
        TempDir::new().unwrap()
    }

    fn ctx() -> Context {
        Context::json()
    }

    #[test]
    fn empty_workspace_returns_success_with_empty_status() {
        let tmp = workspace();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &TestArgs { workspace: tmp.path() }, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "empty");
        assert_eq!(parsed["total"], 0);
    }

    #[test]
    fn passing_tests_exit_success() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_ok.k"), b"assert True\n").unwrap();
        fs::write(tmp.path().join("ok_test.k"), b"assert 1 + 1 == 2\n").unwrap();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &TestArgs { workspace: tmp.path() }, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["passed"], 2);
    }

    #[test]
    fn failing_test_exits_user_error() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_broken.k"), b"assert False, \"nope\"\n").unwrap();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &TestArgs { workspace: tmp.path() }, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "fail");
        assert_eq!(parsed["failed"], 1);
    }

    #[test]
    fn text_output_shape_matches_human_expectations() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_ok.k"), b"assert True\n").unwrap();
        let mut stdout = Vec::new();
        let ctx_human = Context::human();
        run(&ctx_human, &TestArgs { workspace: tmp.path() }, &mut stdout).unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("test: 1 passed"), "got: {text}");
        assert!(text.contains("ok test_ok.k"), "got: {text}");
    }

    #[test]
    fn empty_workspace_human_text() {
        let tmp = workspace();
        let mut stdout = Vec::new();
        run(&Context::human(), &TestArgs { workspace: tmp.path() }, &mut stdout).unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("no tests found"), "got: {text}");
    }
}
