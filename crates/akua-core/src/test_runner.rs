//! Discover + execute KCL test files.
//!
//! A "test file" is any `.k` file whose name matches `test_*.k` or
//! `*_test.k` — both conventions are in the wild, akua accepts
//! either. Each file is loaded through the standard `PackageK`
//! evaluator; KCL's built-in `check:` blocks + `assert` statements
//! fire at eval-time, so:
//!
//! - clean eval  → pass
//! - `err_message` non-empty → fail, message preserved verbatim
//! - I/O error reading the file → hard error (not a test failure)
//!
//! No test-harness abstraction on top of KCL's existing assertion
//! machinery: fewer concepts to learn, exact same behavior `kcl
//! test` / `kcl run` already produce. That's the point.
//!
//! Package-level `charts.*` deps aren't resolved here — tests should
//! target the Package's declarative schemas, not its render output.
//! If you need to exercise a render, that's `akua render` + a golden
//! diff.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::package_k::{PackageK, PackageKError};

/// Aggregate report from a test run. Fields stable per cli-contract
/// §1 — agents branch on `status` + per-file outcomes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestReport {
    /// `"ok"` when every test passed, `"fail"` when any failed,
    /// `"empty"` when no test files were found at all.
    pub status: &'static str,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub tests: Vec<TestOutcome>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TestOutcome {
    /// Relative path from the search root. Stable across machines
    /// when the workspace is in git.
    pub file: PathBuf,
    /// `"pass"` / `"fail"`.
    pub status: &'static str,
    /// KCL error message for failures, empty on pass.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TestRunError {
    #[error("walking workspace `{}`: {source}", root.display())]
    Walk {
        root: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Find + run every test file under `root`. Returns an aggregate
/// `TestReport` even when every test fails — reporting errors
/// per-test is the contract; a walk-level I/O failure is the only
/// hard error.
pub fn run(root: &Path) -> Result<TestReport, TestRunError> {
    let files = discover_test_files(root)?;
    let mut outcomes = Vec::with_capacity(files.len());
    let mut passed = 0;
    let mut failed = 0;
    let inputs = serde_yaml::Value::Mapping(Default::default());

    for file in &files {
        let rel = file.strip_prefix(root).unwrap_or(file).to_path_buf();
        match run_one(file, &inputs) {
            Ok(()) => {
                passed += 1;
                outcomes.push(TestOutcome {
                    file: rel,
                    status: "pass",
                    message: String::new(),
                });
            }
            Err(msg) => {
                failed += 1;
                outcomes.push(TestOutcome {
                    file: rel,
                    status: "fail",
                    message: msg,
                });
            }
        }
    }

    let status = match (files.len(), failed) {
        (0, _) => "empty",
        (_, 0) => "ok",
        _ => "fail",
    };
    Ok(TestReport {
        status,
        total: files.len(),
        passed,
        failed,
        tests: outcomes,
    })
}

/// Run a single test file via the PackageK loader. A KCL eval error
/// surfaces as a test failure with the error message verbatim; any
/// other load error (I/O, serde) collapses to the message — the
/// caller doesn't need to distinguish.
fn run_one(file: &Path, inputs: &serde_yaml::Value) -> Result<(), String> {
    let pkg = PackageK::load(file).map_err(|e| e.to_string())?;
    // Tests may not emit `resources = [...]`; we only care about
    // whether `exec_program` returned clean. Fake out the
    // missing-resources check by using the evaluator directly.
    let outcome = pkg.render(inputs);
    match outcome {
        Ok(_) => Ok(()),
        // A failing `assert` raises inside `exec_program` → surfaces
        // as `KclEval` before `render` reaches the resources check,
        // so this arm can't silently mask a test failure. It only
        // fires on a clean-eval test file that doesn't emit
        // `resources = [...]` — the "bare asserts" shape, which is
        // exactly the pass case we want.
        Err(PackageKError::MissingResources) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Walk `root` for files matching `test_*.k` or `*_test.k`. Sorts
/// the result so test reports are byte-deterministic across
/// filesystems. Hidden dirs + render outputs + language-ecosystem
/// siblings are skipped via the shared [`crate::walk`] helper.
pub(crate) fn discover_test_files(root: &Path) -> Result<Vec<PathBuf>, TestRunError> {
    let pairs = crate::walk::collect_files(root, is_test_file).map_err(|source| {
        TestRunError::Walk {
            root: root.to_path_buf(),
            source,
        }
    })?;
    Ok(pairs.into_iter().map(|(_rel, abs)| abs).collect())
}

/// `test_*.k` or `*_test.k`. Case-sensitive — KCL itself is.
pub(crate) fn is_test_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".k") else {
        return false;
    };
    stem.starts_with("test_") || stem.ends_with("_test")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn is_test_file_matches_both_conventions() {
        assert!(is_test_file("test_schema.k"));
        assert!(is_test_file("schema_test.k"));
        assert!(!is_test_file("regular.k"));
        assert!(!is_test_file("test_file.txt"));
        // Exactly `test_.k` is edge-case-valid.
        assert!(is_test_file("test_.k"));
        assert!(is_test_file("_test.k"));
    }

    #[test]
    fn discover_sorts_and_filters_by_convention() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "test_a.k", b"resources = []\n");
        write(tmp.path(), "b_test.k", b"resources = []\n");
        // `regular.k` isn't a test — no prefix/suffix match.
        write(tmp.path(), "regular.k", b"resources = []\n");
        write(tmp.path(), "deploy/test_nope.k", b"resources = []\n");
        write(tmp.path(), ".hidden/test_nope.k", b"resources = []\n");

        let files = discover_test_files(tmp.path()).unwrap();
        let rels: Vec<String> = files
            .iter()
            .map(|p| p.strip_prefix(tmp.path()).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(rels, vec!["b_test.k".to_string(), "test_a.k".to_string()]);
    }

    #[test]
    fn empty_workspace_yields_empty_report() {
        let tmp = tempfile::tempdir().unwrap();
        let report = run(tmp.path()).unwrap();
        assert_eq!(report.status, "empty");
        assert_eq!(report.total, 0);
    }

    #[test]
    fn passing_test_reports_pass() {
        let tmp = tempfile::tempdir().unwrap();
        // `assert` that holds → clean eval → pass. No `resources` =
        // hits the `MissingResources` arm, which we treat as pass.
        write(
            tmp.path(),
            "test_arith.k",
            b"assert 1 + 1 == 2, \"math is broken\"\n",
        );
        let report = run(tmp.path()).unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn failing_test_reports_fail_with_message() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "test_arith.k",
            b"assert 1 + 1 == 3, \"math is broken\"\n",
        );
        let report = run(tmp.path()).unwrap();
        assert_eq!(report.status, "fail");
        assert_eq!(report.failed, 1);
        assert!(
            report.tests[0].message.contains("math is broken"),
            "expected assert message, got: {}",
            report.tests[0].message
        );
    }

    #[test]
    fn syntax_error_reports_fail() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "test_broken.k", b"this is not kcl {{{\n");
        let report = run(tmp.path()).unwrap();
        assert_eq!(report.status, "fail");
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn mixed_pass_and_fail_accurate_counts() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "test_a.k", b"assert True\n");
        write(tmp.path(), "test_b.k", b"assert False, \"boom\"\n");
        write(tmp.path(), "c_test.k", b"assert 2 > 1\n");
        let report = run(tmp.path()).unwrap();
        assert_eq!(report.status, "fail");
        assert_eq!(report.total, 3);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn discover_skips_hidden_and_excluded_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "test_real.k", b"assert True\n");
        write(tmp.path(), ".git/test_nope.k", b"assert False\n");
        write(tmp.path(), "target/test_nope.k", b"assert False\n");
        write(tmp.path(), "node_modules/test_nope.k", b"assert False\n");

        let files = discover_test_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test_real.k"));
    }
}
