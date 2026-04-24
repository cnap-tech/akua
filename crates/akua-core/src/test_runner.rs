//! Discover + execute KCL test files + run golden snapshot diffs.
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

    #[error("i/o during golden snapshot at `{}`: {source}", path.display())]
    SnapshotIo {
        path: PathBuf,
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
    let pairs =
        crate::walk::collect_files(root, is_test_file).map_err(|source| TestRunError::Walk {
            root: root.to_path_buf(),
            source,
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
// Golden snapshot tests
// ---------------------------------------------------------------------------

/// Aggregate report from a golden-snapshot run. Parallel shape to
/// [`TestReport`] so CLI output can present both in one emit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoldenReport {
    /// `"ok"` / `"fail"` / `"empty"` — `"updated"` when `--update`
    /// wrote snapshots (all cases counted as pass since we can't
    /// regress what we just rewrote).
    pub status: &'static str,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    /// True when `run_golden` was invoked with `update = true`.
    /// Surfaces to callers so JSON output distinguishes a clean
    /// verify from a regen.
    pub updated: bool,
    pub cases: Vec<GoldenOutcome>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoldenOutcome {
    /// Relative path of the Package.k tested.
    pub package: PathBuf,
    /// Relative path of the inputs file used, or `""` when none.
    pub inputs: PathBuf,
    /// `"pass"` / `"fail"` / `"updated"`.
    pub status: &'static str,
    /// Short human-readable summary of the diff on `fail`, empty
    /// on pass/updated.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub diff_summary: String,
}

/// Find + render every `package.k` in `root` against each sibling
/// inputs file, and dir-diff the output against `./snapshots/<rel>/<inputs-stem>/`.
/// `update == true` rewrites the snapshot tree instead of diffing.
///
/// Package discovery walks the workspace via [`crate::walk::collect_files`];
/// directories in [`crate::walk::should_skip_dir`] (`target/`,
/// `node_modules/`, `deploy/`, `rendered/`, dotdirs…) are silently
/// skipped. A `package.k` nested under any of those won't be
/// tested — relocate it or rename the parent if the skip is wrong.
///
/// Snapshots root is `<workspace>/snapshots/<pkg-dir>/<inputs-stem>/`,
/// matching the "snapshots/" convention most ecosystems standardize
/// on (Jest, insta, pytest-snapshot). An `inputs.yaml` at the
/// Package root maps to `snapshots/<pkg-dir>/inputs/`; `inputs.example.yaml`
/// → `snapshots/<pkg-dir>/inputs.example/`; and so on. A Package
/// with no inputs files still gets a single case with an empty
/// inputs map (→ `snapshots/<pkg-dir>/default/`).
pub fn run_golden(root: &Path, update: bool) -> Result<GoldenReport, TestRunError> {
    let packages = discover_packages(root)?;

    let mut cases = Vec::new();
    let mut passed = 0;
    let mut failed = 0;

    for pkg_path in &packages {
        let pkg_dir = pkg_path.parent().unwrap_or(root);
        let rel_pkg = pkg_path.strip_prefix(root).unwrap_or(pkg_path);

        let inputs_list = discover_inputs(pkg_dir);
        let iter: Vec<Option<&Path>> = if inputs_list.is_empty() {
            vec![None]
        } else {
            inputs_list.iter().map(|p| Some(p.as_path())).collect()
        };

        for inputs in iter {
            let inputs_rel: PathBuf = inputs
                .map(|p| p.strip_prefix(root).unwrap_or(p).to_path_buf())
                .unwrap_or_else(|| PathBuf::from(""));
            let stem = inputs_stem(inputs);
            let snapshot_dir = root
                .join("snapshots")
                .join(rel_pkg.parent().unwrap_or(Path::new("")))
                .join(&stem);

            match run_one_golden(pkg_path, inputs, &snapshot_dir, update) {
                Ok(GoldenVerdict::Pass) => {
                    passed += 1;
                    cases.push(GoldenOutcome {
                        package: rel_pkg.to_path_buf(),
                        inputs: inputs_rel,
                        status: "pass",
                        diff_summary: String::new(),
                    });
                }
                Ok(GoldenVerdict::Updated) => {
                    passed += 1;
                    cases.push(GoldenOutcome {
                        package: rel_pkg.to_path_buf(),
                        inputs: inputs_rel,
                        status: "updated",
                        diff_summary: String::new(),
                    });
                }
                Ok(GoldenVerdict::Fail(summary)) => {
                    failed += 1;
                    cases.push(GoldenOutcome {
                        package: rel_pkg.to_path_buf(),
                        inputs: inputs_rel,
                        status: "fail",
                        diff_summary: summary,
                    });
                }
                Err(msg) => {
                    failed += 1;
                    cases.push(GoldenOutcome {
                        package: rel_pkg.to_path_buf(),
                        inputs: inputs_rel,
                        status: "fail",
                        diff_summary: msg,
                    });
                }
            }
        }
    }

    let status = match (cases.len(), failed, update) {
        (0, _, _) => "empty",
        (_, 0, true) => "updated",
        (_, 0, false) => "ok",
        _ => "fail",
    };
    Ok(GoldenReport {
        status,
        total: cases.len(),
        passed,
        failed,
        updated: update,
        cases,
    })
}

enum GoldenVerdict {
    Pass,
    Updated,
    Fail(String),
}

fn run_one_golden(
    pkg_path: &Path,
    inputs_path: Option<&Path>,
    snapshot_dir: &Path,
    update: bool,
) -> Result<GoldenVerdict, String> {
    // Render into a fresh temp dir so we never touch the live
    // `deploy/` or `snapshot_dir` until we're ready.
    let tmp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let out_dir = tmp.path().join("render");

    let pkg = crate::PackageK::load(pkg_path).map_err(|e| e.to_string())?;
    let inputs = load_inputs_or_empty(inputs_path)?;
    let rendered = pkg.render(&inputs).map_err(|e| e.to_string())?;
    crate::package_render::render(&rendered, &out_dir, false).map_err(|e| e.to_string())?;

    if update {
        // Wipe the existing snapshot then move the fresh render
        // into place. Atomic-enough for a local dev workflow; a
        // half-updated snapshot tree is a user-visible state
        // they'd notice and re-run.
        if snapshot_dir.exists() {
            std::fs::remove_dir_all(snapshot_dir)
                .map_err(|e| format!("removing old snapshot at {}: {e}", snapshot_dir.display()))?;
        }
        if let Some(parent) = snapshot_dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        crate::walk::copy_tree(&out_dir, snapshot_dir)
            .map_err(|e| format!("write snapshot at {}: {e}", snapshot_dir.display()))?;
        return Ok(GoldenVerdict::Updated);
    }

    // Compare against the snapshot. Absent snapshot → treat as a
    // fail with a clear "no baseline" summary — steers users to
    // run `--update-snapshots` to establish one.
    if !snapshot_dir.is_dir() {
        return Ok(GoldenVerdict::Fail(format!(
            "no baseline at {} — run with --update-snapshots to establish",
            snapshot_dir.display()
        )));
    }

    let diff = crate::dir_diff::diff(snapshot_dir, &out_dir).map_err(|e| e.to_string())?;
    if diff.is_clean() {
        Ok(GoldenVerdict::Pass)
    } else {
        Ok(GoldenVerdict::Fail(summarize_diff(&diff)))
    }
}

fn summarize_diff(diff: &crate::dir_diff::DirDiff) -> String {
    let mut parts = Vec::new();
    if !diff.added.is_empty() {
        parts.push(format!("{} added", diff.added.len()));
    }
    if !diff.removed.is_empty() {
        parts.push(format!("{} removed", diff.removed.len()));
    }
    if !diff.changed.is_empty() {
        parts.push(format!("{} changed", diff.changed.len()));
    }
    parts.join(", ")
}

fn load_inputs_or_empty(path: Option<&Path>) -> Result<serde_yaml::Value, String> {
    match path {
        None => Ok(serde_yaml::Value::Mapping(Default::default())),
        Some(p) => {
            let bytes = std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()))?;
            serde_yaml::from_slice(&bytes).map_err(|e| format!("parsing {}: {e}", p.display()))
        }
    }
}

/// Discover `package.k` files throughout the workspace. Same walk
/// + skip rules as the assertion-test runner.
fn discover_packages(root: &Path) -> Result<Vec<PathBuf>, TestRunError> {
    let pairs = crate::walk::collect_files(root, |name| name == "package.k").map_err(|source| {
        TestRunError::Walk {
            root: root.to_path_buf(),
            source,
        }
    })?;
    Ok(pairs.into_iter().map(|(_rel, abs)| abs).collect())
}

/// Inputs file candidates next to a Package. We consult both
/// `inputs*.yaml` and `inputs*.json` since `akua render` accepts
/// either. Sorted so snapshot ordering is deterministic.
fn discover_inputs(pkg_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(pkg_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let is_inputs = (name.starts_with("inputs"))
            && (name.ends_with(".yaml") || name.ends_with(".yml") || name.ends_with(".json"));
        if is_inputs && entry.path().is_file() {
            out.push(entry.path());
        }
    }
    out.sort();
    out
}

/// `inputs.yaml` → `"inputs"`, `inputs.example.yaml` → `"inputs.example"`,
/// `None` → `"default"`. Drives the per-case snapshot subdir name.
fn inputs_stem(inputs: Option<&Path>) -> String {
    match inputs {
        None => "default".to_string(),
        Some(p) => {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("inputs");
            // Strip the trailing extension only (first `.` walk
            // would give us just `inputs`; we want
            // `inputs.example`).
            name.rsplit_once('.')
                .map(|(stem, _)| stem.to_string())
                .unwrap_or_else(|| name.to_string())
        }
    }
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
            .map(|p| {
                p.strip_prefix(tmp.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
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

    // --- Golden snapshot tests --------------------------------------

    /// Minimal Package that emits one ConfigMap whose `data.count`
    /// reflects the input. Stable output → valid snapshot fixture.
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

    #[test]
    fn golden_update_then_verify_passes() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.k", MINIMAL_PACKAGE.as_bytes());
        write(tmp.path(), "inputs.yaml", b"replicas: 3\n");

        // First run in update mode establishes the baseline.
        let r1 = run_golden(tmp.path(), true).unwrap();
        assert_eq!(r1.status, "updated", "cases: {:?}", r1.cases);
        assert_eq!(r1.total, 1);

        // Snapshot dir should exist + be populated.
        let snap = tmp.path().join("snapshots").join("inputs");
        assert!(snap.is_dir(), "snapshot dir not created");

        // Second run in verify mode passes against the snapshot we
        // just wrote.
        let r2 = run_golden(tmp.path(), false).unwrap();
        assert_eq!(r2.status, "ok");
        assert_eq!(r2.passed, 1);
    }

    #[test]
    fn golden_detects_render_drift() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.k", MINIMAL_PACKAGE.as_bytes());
        write(tmp.path(), "inputs.yaml", b"replicas: 3\n");
        run_golden(tmp.path(), true).unwrap();

        // Mutate inputs so the render changes — the baseline still
        // claims replicas=3.
        write(tmp.path(), "inputs.yaml", b"replicas: 7\n");
        let r = run_golden(tmp.path(), false).unwrap();
        assert_eq!(r.status, "fail");
        assert_eq!(r.failed, 1);
        assert!(
            r.cases[0].diff_summary.contains("changed")
                || r.cases[0].diff_summary.contains("added")
                || r.cases[0].diff_summary.contains("removed"),
            "summary: {}",
            r.cases[0].diff_summary
        );
    }

    #[test]
    fn golden_verify_without_baseline_surfaces_fail_with_hint() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.k", MINIMAL_PACKAGE.as_bytes());
        write(tmp.path(), "inputs.yaml", b"replicas: 3\n");

        let r = run_golden(tmp.path(), false).unwrap();
        assert_eq!(r.status, "fail");
        assert!(
            r.cases[0].diff_summary.contains("--update-snapshots"),
            "hint missing: {}",
            r.cases[0].diff_summary
        );
    }

    #[test]
    fn golden_empty_workspace_reports_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run_golden(tmp.path(), false).unwrap();
        assert_eq!(r.status, "empty");
        assert_eq!(r.total, 0);
    }

    #[test]
    fn golden_no_inputs_file_uses_default_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.k", MINIMAL_PACKAGE.as_bytes());
        let r = run_golden(tmp.path(), true).unwrap();
        assert_eq!(r.status, "updated");
        assert!(tmp.path().join("snapshots/default").is_dir());
    }

    #[test]
    fn inputs_stem_strips_only_trailing_extension() {
        assert_eq!(inputs_stem(Some(Path::new("inputs.yaml"))), "inputs");
        assert_eq!(
            inputs_stem(Some(Path::new("inputs.example.yaml"))),
            "inputs.example"
        );
        assert_eq!(inputs_stem(None), "default");
    }
}
