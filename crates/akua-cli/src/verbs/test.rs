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
use akua_core::test_runner::{self, GoldenReport, TestReport};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct TestArgs<'a> {
    /// Root under which test files are discovered.
    pub workspace: &'a Path,

    /// `--golden`: also run golden snapshot tests. Every `package.k`
    /// in the workspace gets rendered against each sibling
    /// `inputs*.yaml` and dir-diffed against `snapshots/<pkg>/<stem>/`.
    pub golden: bool,

    /// `--update-snapshots`: regenerate the snapshot tree rather
    /// than diff. Implies `--golden`.
    pub update_snapshots: bool,
}

/// Combined report — runs assertion tests + (optionally) golden
/// tests in a single invocation so CI gets one verdict emit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CombinedReport {
    /// `"ok"` only when *both* sub-reports were `ok` (or `empty`).
    /// Any fail → `"fail"`. `"updated"` when a golden-only run did
    /// regen and asserts passed (or were empty).
    pub status: &'static str,
    pub assertions: TestReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub golden: Option<GoldenReport>,
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
        // Every variant here is system-level (disk walk failed, or
        // writing to stdout failed). Test-failure user-errors go
        // through the `Ok(ExitCode::UserError)` path in `run` —
        // they're a valid verdict, not an error.
        ExitCode::SystemError
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &TestArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, TestError> {
    let assertions = test_runner::run(args.workspace)?;

    // `--update-snapshots` implies `--golden`. We intentionally
    // still run assertion tests so `akua test --update-snapshots`
    // doesn't silently skip authoring-quality checks while ops
    // assume they passed.
    let want_golden = args.golden || args.update_snapshots;
    let golden = if want_golden {
        Some(test_runner::run_golden(args.workspace, args.update_snapshots)?)
    } else {
        None
    };

    let combined_status = combined_status(&assertions, golden.as_ref());
    let exit = match combined_status {
        "fail" => ExitCode::UserError,
        _ => ExitCode::Success,
    };

    let combined = CombinedReport {
        status: combined_status,
        assertions,
        golden,
    };
    emit_output(stdout, ctx, &combined, |w| write_text(w, &combined))
        .map_err(TestError::StdoutWrite)?;
    Ok(exit)
}

/// Combine the two sub-reports' statuses. Any `fail` in either →
/// overall `fail`. Otherwise prefer `updated` when golden regen
/// happened, then `ok`, with `empty` only when both sub-reports
/// are empty.
fn combined_status(
    assertions: &TestReport,
    golden: Option<&GoldenReport>,
) -> &'static str {
    let assertion_fail = assertions.status == "fail";
    let golden_fail = golden.map(|g| g.status == "fail").unwrap_or(false);
    if assertion_fail || golden_fail {
        return "fail";
    }
    if golden.map(|g| g.status == "updated").unwrap_or(false) {
        return "updated";
    }
    let both_empty = assertions.status == "empty"
        && golden.map(|g| g.status == "empty").unwrap_or(true);
    if both_empty {
        return "empty";
    }
    "ok"
}

fn write_text<W: Write>(w: &mut W, report: &CombinedReport) -> std::io::Result<()> {
    write_assertions(w, &report.assertions)?;
    if let Some(g) = &report.golden {
        writeln!(w)?;
        write_golden(w, g)?;
    }
    Ok(())
}

fn write_assertions<W: Write>(w: &mut W, report: &TestReport) -> std::io::Result<()> {
    match report.status {
        "empty" => {
            writeln!(w, "assert: no tests found (*test*.k)")?;
            return Ok(());
        }
        "ok" => {
            writeln!(w, "assert: {} passed", report.total)?;
        }
        "fail" => {
            writeln!(
                w,
                "assert: {} passed, {} failed of {}",
                report.passed, report.failed, report.total,
            )?;
        }
        _ => {
            writeln!(w, "assert: status={}", report.status)?;
        }
    }
    for t in &report.tests {
        let marker = if t.status == "pass" { "ok" } else { "FAIL" };
        writeln!(w, "  {marker} {}", t.file.display())?;
        if !t.message.is_empty() {
            for line in t.message.lines() {
                writeln!(w, "      {line}")?;
            }
        }
    }
    Ok(())
}

fn write_golden<W: Write>(w: &mut W, report: &GoldenReport) -> std::io::Result<()> {
    match report.status {
        "empty" => {
            writeln!(w, "golden: no package.k found")?;
            return Ok(());
        }
        "updated" => {
            writeln!(w, "golden: {} snapshot(s) updated", report.total)?;
        }
        "ok" => {
            writeln!(w, "golden: {} passed", report.total)?;
        }
        "fail" => {
            writeln!(
                w,
                "golden: {} passed, {} failed of {}",
                report.passed, report.failed, report.total,
            )?;
        }
        _ => {
            writeln!(w, "golden: status={}", report.status)?;
        }
    }
    for c in &report.cases {
        let marker = match c.status {
            "pass" => "ok",
            "updated" => "~~",
            _ => "FAIL",
        };
        let inputs_label = if c.inputs.as_os_str().is_empty() {
            "(default)".to_string()
        } else {
            c.inputs.display().to_string()
        };
        writeln!(
            w,
            "  {marker} {} [{}]",
            c.package.display(),
            inputs_label
        )?;
        if !c.diff_summary.is_empty() {
            writeln!(w, "      {}", c.diff_summary)?;
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

    fn base_args(ws: &std::path::Path) -> TestArgs<'_> {
        TestArgs {
            workspace: ws,
            golden: false,
            update_snapshots: false,
        }
    }

    #[test]
    fn empty_workspace_returns_success_with_empty_status() {
        let tmp = workspace();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &base_args(tmp.path()), &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "empty");
        assert_eq!(parsed["assertions"]["status"], "empty");
    }

    #[test]
    fn passing_tests_exit_success() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_ok.k"), b"assert True\n").unwrap();
        fs::write(tmp.path().join("ok_test.k"), b"assert 1 + 1 == 2\n").unwrap();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &base_args(tmp.path()), &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["assertions"]["passed"], 2);
    }

    #[test]
    fn failing_test_exits_user_error() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_broken.k"), b"assert False, \"nope\"\n").unwrap();
        let mut stdout = Vec::new();
        let code = run(&ctx(), &base_args(tmp.path()), &mut stdout).unwrap();
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "fail");
        assert_eq!(parsed["assertions"]["failed"], 1);
    }

    #[test]
    fn text_output_shape_matches_human_expectations() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_ok.k"), b"assert True\n").unwrap();
        let mut stdout = Vec::new();
        let ctx_human = Context::human();
        run(&ctx_human, &base_args(tmp.path()), &mut stdout).unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("assert: 1 passed"), "got: {text}");
        assert!(text.contains("ok test_ok.k"), "got: {text}");
    }

    #[test]
    fn empty_workspace_human_text() {
        let tmp = workspace();
        let mut stdout = Vec::new();
        run(&Context::human(), &base_args(tmp.path()), &mut stdout).unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("no tests found"), "got: {text}");
    }

    #[test]
    fn golden_mode_round_trip_passes() {
        let tmp = workspace();
        fs::write(
            tmp.path().join("package.k"),
            r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "demo"
    data.count: str(input.replicas)
}]
"#,
        )
        .unwrap();
        fs::write(tmp.path().join("inputs.yaml"), b"replicas: 3\n").unwrap();

        // Establish baseline.
        let args = TestArgs {
            update_snapshots: true,
            ..base_args(tmp.path())
        };
        let mut stdout = Vec::new();
        let code = run(&ctx(), &args, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "updated");
        assert_eq!(parsed["golden"]["status"], "updated");

        // Verify against the baseline.
        let args = TestArgs {
            golden: true,
            ..base_args(tmp.path())
        };
        let mut stdout = Vec::new();
        let code = run(&ctx(), &args, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["golden"]["passed"], 1);
    }

    #[test]
    fn assert_pass_plus_golden_fail_overall_fails() {
        let tmp = workspace();
        fs::write(tmp.path().join("test_ok.k"), b"assert True\n").unwrap();
        fs::write(
            tmp.path().join("package.k"),
            b"resources = [{apiVersion: \"v1\", kind: \"ConfigMap\", metadata.name: \"x\"}]\n",
        )
        .unwrap();

        // Golden mode without baseline → fail on the golden side.
        let args = TestArgs {
            golden: true,
            ..base_args(tmp.path())
        };
        let mut stdout = Vec::new();
        let code = run(&ctx(), &args, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "fail");
        assert_eq!(parsed["assertions"]["status"], "ok");
        assert_eq!(parsed["golden"]["status"], "fail");
    }
}
