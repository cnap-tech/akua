//! `akua verify` — lockfile consistency check.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua verify` section.
//!
//! Enforces, against `akua.toml` + `akua.lock`:
//!
//! - Every dep declared in `[dependencies]` appears in the lockfile.
//! - Every `[[package]]` in the lockfile corresponds to a declared dep
//!   (no orphans — someone modified the lockfile without updating the
//!   manifest).
//! - When `[package].strict_signing = true` (the default), every
//!   locked package carries a `signature`.
//!
//! Digest format (sha256: prefix) is enforced by the lockfile parser
//! itself; parse errors surface here as structured verify failures.

use std::io::Write;
use std::path::Path;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{chart_resolver, AkuaLock, AkuaManifest, LockLoadError, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

/// Verify verdict JSON shape. Stable across releases.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VerifyOutput {
    /// `"ok"` when everything checks out; `"fail"` otherwise. Derived
    /// from `violations.is_empty()`.
    pub status: &'static str,

    pub summary: Summary,

    /// Structured violations. Empty when `status == "ok"`.
    pub violations: Vec<Violation>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub declared_deps: usize,
    pub locked_packages: usize,
    pub strict_signing: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Violation {
    /// Dep declared in `akua.toml` but missing from `akua.lock`.
    UnlockedDep { name: String },
    /// Package locked in `akua.lock` but not declared in `akua.toml`.
    OrphanLocked { name: String, version: String },
    /// Package is locked without a signature while `strict_signing = true`.
    MissingSignature { name: String, version: String },
    /// Path-dep on-disk content diverges from the digest `akua.lock`
    /// pinned. Someone mutated the vendored chart without re-running
    /// `akua add` — either intentional (run add to refresh) or
    /// accidental (revert the edit).
    PathDigestDrift {
        name: String,
        expected: String,
        actual: String,
    },
    /// Path-dep's declared target no longer exists on disk. Likely a
    /// deleted `./vendor/<chart>` directory.
    PathMissing { name: String, path: String },
}

impl VerifyOutput {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Errors that prevent the verify check from running at all. Distinct
/// from [`Violation`]s which are check failures on well-formed inputs.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Lock(#[from] LockLoadError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl VerifyError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            VerifyError::Manifest(e) => {
                let base = e.to_structured();
                if matches!(e, ManifestLoadError::Missing { .. }) {
                    base.with_suggestion("run `akua init` or check the working directory")
                } else {
                    base
                }
            }
            VerifyError::Lock(e) => {
                let base = e.to_structured();
                if matches!(e, LockLoadError::Missing { .. }) {
                    base.with_suggestion(
                        "run `akua add` to resolve deps and generate the lockfile",
                    )
                } else {
                    base
                }
            }
            VerifyError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            VerifyError::Manifest(e) if e.is_system() => ExitCode::SystemError,
            VerifyError::Lock(e) if e.is_system() => ExitCode::SystemError,
            VerifyError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

/// Core check: load both files, run the cross-consistency checks, and
/// return a [`VerifyOutput`]. Pure; no writes.
pub fn check(workspace: &Path) -> Result<VerifyOutput, VerifyError> {
    let manifest = AkuaManifest::load(workspace)?;
    let lock = AkuaLock::load(workspace)?;

    let mut violations = Vec::new();

    let locked_names: std::collections::HashSet<&str> =
        lock.packages.iter().map(|p| p.name.as_str()).collect();
    for (dep_name, _dep) in &manifest.dependencies {
        if !locked_names.contains(dep_name.as_str()) {
            violations.push(Violation::UnlockedDep {
                name: dep_name.clone(),
            });
        }
    }

    let declared_names: std::collections::HashSet<&str> =
        manifest.dependencies.keys().map(|s| s.as_str()).collect();
    for pkg in &lock.packages {
        if !declared_names.contains(pkg.name.as_str()) {
            violations.push(Violation::OrphanLocked {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
            });
        }
    }

    if manifest.package.strict_signing {
        for pkg in &lock.packages {
            // Path-sourced deps are local files (not registry-fetched),
            // so there's nothing to sign against — exempt from strict
            // signing. Phase 6 may revisit (provenance for locally-
            // vendored charts via `akua publish` attestation), but
            // until then a missing sig on a path dep is correct.
            if pkg.is_path() {
                continue;
            }
            if pkg.signature.is_none() {
                violations.push(Violation::MissingSignature {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                });
            }
        }
    }

    // Path-dep drift detection: re-hash the on-disk chart and compare
    // to the lockfile digest. A mismatch means someone mutated the
    // vendored tree without re-running `akua add` — CI must fail. We
    // run the resolver in offline mode so OCI/git deps don't touch
    // the network from `akua verify`.
    let drift_resolution = chart_resolver::resolve(&manifest, workspace);
    if let Ok(resolved) = drift_resolution {
        for pkg in &lock.packages {
            if !pkg.is_path() {
                continue;
            }
            match resolved.entries.get(&pkg.name) {
                None => {
                    // Dep declared in manifest but resolver couldn't find
                    // its target. Only fires when `akua.toml` still lists
                    // a path dep — OrphanLocked covers removal cases.
                    if manifest.dependencies.contains_key(&pkg.name) {
                        violations.push(Violation::PathMissing {
                            name: pkg.name.clone(),
                            path: pkg.source.clone(),
                        });
                    }
                }
                Some(resolved_chart) if resolved_chart.sha256 != pkg.digest => {
                    violations.push(Violation::PathDigestDrift {
                        name: pkg.name.clone(),
                        expected: pkg.digest.clone(),
                        actual: resolved_chart.sha256.clone(),
                    });
                }
                Some(_) => {} // digest matches
            }
        }
    } else if let Err(e) = drift_resolution {
        // Resolver failures for path deps are surfaced as PathMissing
        // violations — anything else (e.g. UnsupportedSource on a
        // bare OCI dep) is legitimately not our problem here.
        if let chart_resolver::ChartResolveError::NotFound { name, path }
        | chart_resolver::ChartResolveError::NotADirectory { name, path } = e
        {
            violations.push(Violation::PathMissing {
                name,
                path: path.display().to_string(),
            });
        }
    }

    let status = if violations.is_empty() { "ok" } else { "fail" };

    Ok(VerifyOutput {
        status,
        summary: Summary {
            declared_deps: manifest.dependencies.len(),
            locked_packages: lock.packages.len(),
            strict_signing: manifest.package.strict_signing,
        },
        violations,
    })
}

/// Run the verb against the given workspace. Verify errors (missing /
/// malformed files) are surfaced to the caller; check failures
/// (violations) produce an exit 1 with verdict on stdout.
pub fn run<W: Write>(
    ctx: &Context,
    workspace: &Path,
    stdout: &mut W,
) -> Result<ExitCode, VerifyError> {
    let output = check(workspace)?;
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(VerifyError::StdoutWrite)?;
    Ok(if output.is_ok() {
        ExitCode::Success
    } else {
        ExitCode::UserError
    })
}

fn write_text<W: Write>(stdout: &mut W, output: &VerifyOutput) -> std::io::Result<()> {
    writeln!(
        stdout,
        "verify: {} declared, {} locked, strict_signing={}",
        output.summary.declared_deps, output.summary.locked_packages, output.summary.strict_signing,
    )?;

    if output.is_ok() {
        writeln!(stdout, "ok")?;
    } else {
        writeln!(stdout, "fail: {} violation(s)", output.violations.len())?;
        for v in &output.violations {
            match v {
                Violation::UnlockedDep { name } => writeln!(stdout, "  - unlocked-dep: {name}")?,
                Violation::OrphanLocked { name, version } => {
                    writeln!(stdout, "  - orphan-locked: {name}@{version}")?
                }
                Violation::MissingSignature { name, version } => {
                    writeln!(stdout, "  - missing-signature: {name}@{version}")?
                }
                Violation::PathDigestDrift {
                    name,
                    expected,
                    actual,
                } => writeln!(
                    stdout,
                    "  - path-digest-drift: {name}\n      expected {expected}\n      actual   {actual}\n      run `akua add --force {name} --path <current_path>` to refresh"
                )?,
                Violation::PathMissing { name, path } => writeln!(
                    stdout,
                    "  - path-missing: {name} at `{path}`"
                )?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_workspace(toml: &str, lock: &str) -> TempDir {
        let dir = TempDir::new().expect("tmp");
        fs::write(dir.path().join("akua.toml"), toml).expect("write toml");
        fs::write(dir.path().join("akua.lock"), lock).expect("write lock");
        dir
    }

    const MANIFEST_TWO_OCI: &str = r#"
[package]
name    = "ws"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
cnpg   = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }
webapp = { oci = "oci://ghcr.io/acme/charts/webapp",            version = "2.1.0" }
"#;

    const LOCK_MATCHING: &str = r#"
version = 1

[[package]]
name      = "cnpg"
version   = "0.20.0"
source    = "oci://ghcr.io/cloudnative-pg/charts/cluster"
digest    = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
signature = "cosign:sigstore:cloudnative-pg"

[[package]]
name      = "webapp"
version   = "2.1.0"
source    = "oci://ghcr.io/acme/charts/webapp"
digest    = "sha256:a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
signature = "cosign:key:acme"
"#;

    #[test]
    fn ok_verdict_when_manifest_and_lock_agree() {
        let ws = write_workspace(MANIFEST_TWO_OCI, LOCK_MATCHING);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "ok");
        assert_eq!(result.summary.declared_deps, 2);
        assert_eq!(result.summary.locked_packages, 2);
        assert!(result.summary.strict_signing);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn flags_unlocked_dep() {
        let lock_missing_webapp = r#"
version = 1

[[package]]
name      = "cnpg"
version   = "0.20.0"
source    = "oci://ghcr.io/cloudnative-pg/charts/cluster"
digest    = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
signature = "cosign:sigstore:cloudnative-pg"
"#;
        let ws = write_workspace(MANIFEST_TWO_OCI, lock_missing_webapp);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0],
            Violation::UnlockedDep {
                name: "webapp".into()
            }
        );
    }

    #[test]
    fn flags_orphan_locked() {
        let lock_with_orphan = format!(
            "{LOCK_MATCHING}
[[package]]
name      = \"zzz-extra\"
version   = \"9.9.9\"
source    = \"oci://example.com/orphan\"
digest    = \"sha256:deadbeef00000000000000000000000000000000000000000000000000000000\"
signature = \"cosign:key:x\"
"
        );
        let ws = write_workspace(MANIFEST_TWO_OCI, &lock_with_orphan);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        let has_orphan = result.violations.iter().any(|v| {
            matches!(v, Violation::OrphanLocked { name, version }
                if name == "zzz-extra" && version == "9.9.9")
        });
        assert!(
            has_orphan,
            "expected orphan-locked violation, got: {:?}",
            result.violations
        );
    }

    #[test]
    fn flags_missing_signature_under_strict_signing() {
        let lock_unsigned = r#"
version = 1

[[package]]
name    = "cnpg"
version = "0.20.0"
source  = "oci://ghcr.io/cloudnative-pg/charts/cluster"
digest  = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"

[[package]]
name      = "webapp"
version   = "2.1.0"
source    = "oci://ghcr.io/acme/charts/webapp"
digest    = "sha256:a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
signature = "cosign:key:acme"
"#;
        let ws = write_workspace(MANIFEST_TWO_OCI, lock_unsigned);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        let has_missing = result
            .violations
            .iter()
            .any(|v| matches!(v, Violation::MissingSignature { name, .. } if name == "cnpg"));
        assert!(
            has_missing,
            "expected missing-signature for cnpg: {:?}",
            result.violations
        );
    }

    #[test]
    fn permits_unsigned_when_strict_signing_disabled() {
        let permissive_manifest = r#"
[package]
name           = "ws"
version        = "0.1.0"
edition        = "akua.dev/v1alpha1"
strict_signing = false

[dependencies]
cnpg = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }
"#;
        let unsigned_lock = r#"
version = 1

[[package]]
name    = "cnpg"
version = "0.20.0"
source  = "oci://ghcr.io/cloudnative-pg/charts/cluster"
digest  = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
"#;
        let ws = write_workspace(permissive_manifest, unsigned_lock);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "ok");
        assert!(!result.summary.strict_signing);
    }

    #[test]
    fn reports_multiple_violations() {
        let manifest = r#"
[package]
name    = "ws"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
declared-but-unlocked = { oci = "oci://foo", version = "1.0.0" }
also-unlocked         = { oci = "oci://bar", version = "2.0.0" }
"#;
        let lock = r#"
version = 1

[[package]]
name    = "orphan-one"
version = "1.0"
source  = "oci://orphan-one"
digest  = "sha256:1111111111111111111111111111111111111111111111111111111111111111"

[[package]]
name    = "orphan-two"
version = "2.0"
source  = "oci://orphan-two"
digest  = "sha256:2222222222222222222222222222222222222222222222222222222222222222"
"#;
        let ws = write_workspace(manifest, lock);
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        // 2 unlocked-deps + 2 orphan-lockeds + 2 missing-sigs (strict on) = 6
        assert_eq!(
            result.violations.len(),
            6,
            "violations: {:?}",
            result.violations
        );
    }

    #[test]
    fn missing_manifest_returns_structured_error() {
        let dir = TempDir::new().expect("tmp");
        fs::write(dir.path().join("akua.lock"), "version = 1\n").expect("write lock");
        let err = check(dir.path()).expect_err("should fail");
        assert!(matches!(
            err,
            VerifyError::Manifest(ManifestLoadError::Missing { .. })
        ));
        let structured = err.to_structured();
        assert_eq!(structured.code, "E_MANIFEST_MISSING");
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn missing_lock_returns_structured_error() {
        let dir = TempDir::new().expect("tmp");
        fs::write(dir.path().join("akua.toml"), MANIFEST_TWO_OCI).expect("write toml");
        let err = check(dir.path()).expect_err("should fail");
        assert!(matches!(
            err,
            VerifyError::Lock(LockLoadError::Missing { .. })
        ));
        let structured = err.to_structured();
        assert_eq!(structured.code, "E_LOCK_MISSING");
        assert!(structured.suggestion.is_some());
    }

    #[test]
    fn malformed_manifest_surfaces_parse_error() {
        let ws = write_workspace("this is not toml {{{", LOCK_MATCHING);
        let err = check(ws.path()).expect_err("should fail");
        assert!(matches!(
            err,
            VerifyError::Manifest(ManifestLoadError::Parse { .. })
        ));
        let structured = err.to_structured();
        assert_eq!(structured.code, "E_MANIFEST_PARSE");
    }

    // ----- run() integration: stdout + exit code --------------------------

    #[test]
    fn run_emits_json_verdict_on_ok() {
        let ws = write_workspace(MANIFEST_TWO_OCI, LOCK_MATCHING);
        let ctx = Context::json();
        let mut stdout = Vec::new();
        let code = run(&ctx, ws.path(), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);

        let out = String::from_utf8(stdout).expect("utf8");
        let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["summary"]["declared_deps"], 2);
    }

    #[test]
    fn run_exits_user_error_on_violations() {
        let lock_missing_webapp = r#"
version = 1

[[package]]
name      = "cnpg"
version   = "0.20.0"
source    = "oci://ghcr.io/cloudnative-pg/charts/cluster"
digest    = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
signature = "cosign:sigstore:cloudnative-pg"
"#;
        let ws = write_workspace(MANIFEST_TWO_OCI, lock_missing_webapp);
        let ctx = Context::human();
        let mut stdout = Vec::new();
        let code = run(&ctx, ws.path(), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::UserError);

        let out = String::from_utf8(stdout).expect("utf8");
        assert!(out.contains("fail:"), "{out}");
        assert!(out.contains("unlocked-dep: webapp"), "{out}");
    }

    #[test]
    fn verifies_every_example_workspace_in_tree() {
        // Integration-ish: the four example workspaces on disk that have
        // both akua.toml + akua.lock should all verify cleanly.
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let examples = crate_dir
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .join("examples");

        for entry in fs::read_dir(&examples).expect("read examples") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("akua.toml").exists() || !path.join("akua.lock").exists() {
                continue;
            }
            let result =
                check(&path).unwrap_or_else(|e| panic!("verify failed on {}: {e}", path.display()));
            assert_eq!(
                result.status,
                "ok",
                "example {} has violations: {:?}",
                path.display(),
                result.violations
            );
        }
    }

    // ---------------------------------------------------------------
    // Path-dep drift detection (Phase 2b slice C)
    // ---------------------------------------------------------------

    /// Write a minimal chart tree under `root` for use in path-dep
    /// drift tests. Matches the digest shape the resolver produces.
    fn write_chart(root: &std::path::Path, body: &str) {
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::write(
            root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();
        fs::write(root.join("templates/cm.yaml"), body).unwrap();
    }

    /// Build a workspace: manifest pins `nginx` as a local path, lock
    /// carries a specific digest. Returns the TempDir for use.
    fn write_path_workspace(chart_body: &str, lock_digest: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        write_chart(&dir.path().join("vendor/nginx"), chart_body);
        fs::write(
            dir.path().join("akua.toml"),
            r#"
[package]
name    = "ws"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { path = "./vendor/nginx" }
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("akua.lock"),
            format!(
                r#"
version = 1

[[package]]
name    = "nginx"
version = "local"
source  = "path+file://./vendor/nginx"
digest  = "{lock_digest}"
"#
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn path_dep_digest_drift_surfaces_violation() {
        let ws = write_path_workspace(
            "apiVersion: v1\nkind: ConfigMap\nmetadata: { name: demo }\n",
            // Bogus digest — chart on disk won't hash to this.
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        );
        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        let drift = result
            .violations
            .iter()
            .find(|v| matches!(v, Violation::PathDigestDrift { .. }))
            .expect("drift violation present");
        match drift {
            Violation::PathDigestDrift { name, expected, .. } => {
                assert_eq!(name, "nginx");
                assert!(expected.starts_with("sha256:0000"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn path_dep_with_matching_digest_is_ok() {
        // Compute the real digest for the vendored chart, write it
        // into the lock, verify clean.
        let tmp = TempDir::new().unwrap();
        write_chart(
            &tmp.path().join("vendor/nginx"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata: { name: demo }\n",
        );
        fs::write(
            tmp.path().join("akua.toml"),
            r#"
[package]
name    = "ws"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { path = "./vendor/nginx" }
"#,
        )
        .unwrap();
        let manifest =
            AkuaManifest::load(tmp.path()).expect("manifest");
        let resolved =
            chart_resolver::resolve(&manifest, tmp.path()).expect("resolve");
        let real_digest = resolved.entries.get("nginx").unwrap().sha256.clone();
        fs::write(
            tmp.path().join("akua.lock"),
            format!(
                r#"
version = 1

[[package]]
name    = "nginx"
version = "local"
source  = "path+file://./vendor/nginx"
digest  = "{real_digest}"
"#
            ),
        )
        .unwrap();

        let result = check(tmp.path()).expect("check");
        assert_eq!(result.status, "ok", "violations: {:?}", result.violations);
    }

    #[test]
    fn deleted_path_dep_surfaces_path_missing_violation() {
        let ws = write_path_workspace(
            "apiVersion: v1\nkind: ConfigMap\nmetadata: { name: demo }\n",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        );
        // Rip out the vendored chart.
        fs::remove_dir_all(ws.path().join("vendor")).unwrap();

        let result = check(ws.path()).expect("check");
        assert_eq!(result.status, "fail");
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, Violation::PathMissing { name, .. } if name == "nginx")),
            "violations: {:?}",
            result.violations
        );
    }
}
