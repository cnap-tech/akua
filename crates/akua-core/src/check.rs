//! Pure-compute implementation of `akua check`: run the three
//! fast structural gates (manifest parse, lockfile parse,
//! Package.k parse) against source strings. No filesystem access
//! here — the CLI + SDK read files at their own layers and hand
//! buffers in.
//!
//! Spec: [`docs/cli-contract.md §2`](../../../../docs/cli-contract.md#2-exit-codes)
//! + [`docs/cli.md akua check`](../../../../docs/cli.md#akua-check).

use serde::Serialize;

use crate::package_k::{lint_kcl_source, LintIssue};
use crate::{AkuaLock, AkuaManifest};

crate::contract_type! {
/// Output shape for `akua check --json`. The `status` field is
/// `"ok"` iff every entry in `checks` has `ok == true`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckOutput {
    pub status: &'static str,
    pub checks: Vec<CheckResult>,
}
}

crate::contract_type! {
/// One structural gate's verdict. `name` is the gate label:
/// `"manifest"`, `"lockfile"`, `"package"`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,

    pub ok: bool,

    /// One-line error from the failing check. `#[serde(default)]` is
    /// load-bearing — schemars uses it to mark the field optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Per-file KCL lint issues — only the `"package"` check
    /// populates this; the other gates leave it empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<LintIssue>,
}
}

/// Run the three gates. Every source is `Option<&str>` because the
/// CLI tolerates a workspace missing one or more: a fresh package
/// may have no `akua.lock` yet; `akua check --package-only` won't
/// pass a manifest. The akua-cli verb decides when to read each
/// file and when to skip.
///
/// `package_source` pairs a filename with a buffer; the filename
/// is only used for diagnostic rendering (KCL reports positions
/// tagged with it).
pub fn check_from_sources(
    manifest: Option<&str>,
    lock: Option<&str>,
    package: Option<(&str, &str)>,
) -> CheckOutput {
    let mut checks = Vec::with_capacity(3);
    if let Some(toml) = manifest {
        checks.push(check_manifest(toml));
    }
    if let Some(toml) = lock {
        checks.push(check_lockfile(toml));
    }
    if let Some((filename, source)) = package {
        checks.push(check_package(filename, source));
    }
    let status = if checks.iter().all(|c| c.ok) { "ok" } else { "fail" };
    CheckOutput { status, checks }
}

fn check_manifest(toml: &str) -> CheckResult {
    match AkuaManifest::parse(toml) {
        Ok(_) => ok("manifest"),
        Err(e) => fail("manifest", e.to_string()),
    }
}

fn check_lockfile(toml: &str) -> CheckResult {
    match AkuaLock::parse(toml) {
        Ok(_) => ok("lockfile"),
        Err(e) => fail("lockfile", e.to_string()),
    }
}

fn check_package(filename: &str, source: &str) -> CheckResult {
    match lint_kcl_source(filename, source) {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = r#"
[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
"#;

    const MINIMAL_LOCK: &str = r#"version = 1
packages = []
"#;

    const MINIMAL_PACKAGE: &str = "resources = []\n";

    #[test]
    fn all_three_gates_pass() {
        let out = check_from_sources(
            Some(MINIMAL_MANIFEST),
            Some(MINIMAL_LOCK),
            Some(("package.k", MINIMAL_PACKAGE)),
        );
        assert_eq!(out.status, "ok");
        assert_eq!(out.checks.len(), 3);
        assert!(out.checks.iter().all(|c| c.ok));
    }

    #[test]
    fn missing_lockfile_is_skipped_not_failed() {
        let out = check_from_sources(
            Some(MINIMAL_MANIFEST),
            None,
            Some(("package.k", MINIMAL_PACKAGE)),
        );
        assert_eq!(out.status, "ok");
        assert_eq!(out.checks.len(), 2);
        assert_eq!(out.checks[0].name, "manifest");
        assert_eq!(out.checks[1].name, "package");
    }

    #[test]
    fn manifest_parse_failure_surfaces_via_error() {
        let out = check_from_sources(Some("not valid toml [[["), None, None);
        assert_eq!(out.status, "fail");
        assert_eq!(out.checks.len(), 1);
        assert!(!out.checks[0].ok);
        assert!(out.checks[0].error.is_some());
    }

    #[test]
    fn package_lint_issues_populate_the_issues_vec() {
        // Unclosed list literal — definite KCL parse error.
        let out = check_from_sources(None, None, Some(("bad.k", "resources = [\n")));
        assert_eq!(out.status, "fail");
        let pkg = out.checks.iter().find(|c| c.name == "package").unwrap();
        assert!(!pkg.ok);
        assert!(!pkg.issues.is_empty());
    }
}
