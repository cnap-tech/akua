//! `akua lock` — regenerate `akua.lock` from `akua.toml`.
//!
//! Cargo analogue: `cargo generate-lockfile`. Resolves every declared
//! dep (online — path, OCI, git), merges the result into any existing
//! lock (preserving signatures on entries that didn't change), writes
//! back deterministically.
//!
//! `--check` mode: regenerate in-memory, diff against the on-disk
//! lock, exit 1 on drift. Pipelines / pre-commit hooks use this to
//! catch "author edited `akua.toml` but forgot to re-lock."

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::chart_resolver::{self, ChartResolveError, ResolvedSource, ResolverOptions};
use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{AkuaLock, AkuaManifest, LockLoadError, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct LockArgs<'a> {
    pub workspace: &'a Path,
    pub check: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockOutput {
    /// `true` when the verb wrote to disk. Always `false` under
    /// `--check`. In non-check mode, a write happens even when the
    /// lockfile didn't change — same as `cargo generate-lockfile`'s
    /// unconditional refresh.
    pub wrote: bool,
    /// `true` when the regenerated lock would differ from the
    /// on-disk lock. Meaningful under `--check`; in non-check mode
    /// it records whether the write was substantive or a no-op.
    pub drift: bool,
    pub locked_packages: Vec<LockedSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockedSummary {
    pub name: String,
    pub version: String,
    pub digest: String,
    /// `"path"`, `"oci"`, or `"git"`.
    pub source_kind: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Lock(#[from] LockLoadError),

    #[error(transparent)]
    Resolve(#[from] ChartResolveError),

    #[error("lockfile drift detected — on-disk akua.lock doesn't match what `akua lock` would write")]
    Drift,

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl LockError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            LockError::Manifest(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            LockError::Lock(e) => {
                StructuredError::new(codes::E_LOCK_PARSE, e.to_string()).with_default_docs()
            }
            LockError::Resolve(e) => {
                StructuredError::new(codes::E_CHART_RESOLVE, e.to_string()).with_default_docs()
            }
            LockError::Drift => {
                StructuredError::new(codes::E_LOCK_PARSE, self.to_string()).with_default_docs()
            }
            LockError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            LockError::StdoutWrite(_) => ExitCode::SystemError,
            LockError::Drift => ExitCode::UserError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &LockArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, LockError> {
    let manifest = AkuaManifest::load(args.workspace)?;
    let prior_lock = AkuaLock::load(args.workspace).unwrap_or_else(|_| AkuaLock::empty());

    // Feed prior OCI digests so the resolver reuses the cache + trips
    // LockDigestMismatch on supply-chain drift (registry serving bytes
    // that differ from what was recorded last time).
    let expected_digests = prior_lock
        .packages
        .iter()
        .filter(|p| p.is_oci())
        .map(|p| (p.name.clone(), p.digest.clone()))
        .collect();
    let opts = ResolverOptions {
        offline: false,
        cache_root: None,
        expected_digests,
        cosign_public_key_pem: None,
    };

    let resolved = chart_resolver::resolve_with_options(&manifest, args.workspace, &opts)?;

    let mut new_lock = prior_lock.clone();
    chart_resolver::merge_into_lock(&mut new_lock, &resolved);

    // Canonical diff: serialize both to TOML via save()'s formatter
    // and compare byte-wise. save() sorts packages, so the strings
    // are deterministic and semantic equality reduces to string eq.
    let prior_bytes = lock_to_canonical_toml(&prior_lock)?;
    let new_bytes = lock_to_canonical_toml(&new_lock)?;
    let drift = prior_bytes != new_bytes;

    if args.check {
        if drift {
            // Still emit the intended output so the caller can see
            // what the regenerated lock would carry — useful for CI
            // failure messages — before returning the structured
            // Drift error.
            let out = build_output(false, true, &resolved);
            emit_output(stdout, ctx, &out, |w| write_text(w, &out))
                .map_err(LockError::StdoutWrite)?;
            return Err(LockError::Drift);
        }
    } else {
        new_lock.save(args.workspace)?;
    }

    let out = build_output(!args.check, drift, &resolved);
    emit_output(stdout, ctx, &out, |w| write_text(w, &out)).map_err(LockError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn build_output(wrote: bool, drift: bool, resolved: &chart_resolver::ResolvedCharts) -> LockOutput {
    let mut locked_packages: Vec<LockedSummary> = resolved
        .entries
        .values()
        .map(|c| {
            let (_source_str, version, _replace) = c.source.to_locked_fields();
            LockedSummary {
                name: c.name.clone(),
                version,
                digest: c.sha256.clone(),
                source_kind: source_kind_for(&c.source),
            }
        })
        .collect();
    locked_packages.sort_by(|a, b| a.name.cmp(&b.name));
    LockOutput {
        wrote,
        drift,
        locked_packages,
    }
}

fn source_kind_for(src: &ResolvedSource) -> &'static str {
    match src {
        ResolvedSource::Path { .. } => "path",
        ResolvedSource::Oci { .. } | ResolvedSource::OciReplaced { .. } => "oci",
        ResolvedSource::Git { .. } | ResolvedSource::GitReplaced { .. } => "git",
    }
}

/// Serialize a lock to its canonical TOML form — same bytes `save()`
/// would write to disk. Used for equality comparison.
fn lock_to_canonical_toml(lock: &AkuaLock) -> Result<String, LockError> {
    let mut copy = lock.clone();
    copy.sort();
    copy.to_toml()
        .map_err(|source| LockError::Lock(LockLoadError::Parse {
            path: PathBuf::from("<memory>"),
            source,
        }))
}

fn write_text<W: Write>(w: &mut W, out: &LockOutput) -> std::io::Result<()> {
    if out.locked_packages.is_empty() {
        writeln!(w, "no dependencies to lock")?;
    } else {
        writeln!(w, "locked {} package(s):", out.locked_packages.len())?;
        for p in &out.locked_packages {
            writeln!(
                w,
                "  [{}] {} {}  {}",
                p.source_kind, p.name, p.version, p.digest
            )?;
        }
    }
    if out.drift && !out.wrote {
        writeln!(w, "DRIFT — akua.lock is out of date with akua.toml")?;
    } else if out.wrote && out.drift {
        writeln!(w, "updated akua.lock")?;
    } else if out.wrote {
        writeln!(w, "akua.lock up to date")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    fn workspace_with_path_dep(tmp: &Path) {
        // Chart dir with Chart.yaml for path-dep digest.
        let chart = tmp.join("vendor/nginx");
        std::fs::create_dir_all(&chart).unwrap();
        std::fs::write(chart.join("Chart.yaml"), b"apiVersion: v2\nname: nginx\nversion: 1.0.0\n")
            .unwrap();
        std::fs::write(
            tmp.join("akua.toml"),
            br#"
[package]
name = "lock-test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { path = "./vendor/nginx" }
"#,
        )
        .unwrap();
    }

    #[test]
    fn lock_writes_akua_lock_for_path_dep() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_path_dep(tmp.path());

        let mut stdout = Vec::new();
        let args = LockArgs {
            workspace: tmp.path(),
            check: false,
        };
        let code = run(&ctx_json(), &args, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        assert!(tmp.path().join("akua.lock").is_file(), "lock not written");
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["wrote"], true);
        let pkgs = parsed["locked_packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0]["name"], "nginx");
        assert_eq!(pkgs[0]["source_kind"], "path");
        assert!(pkgs[0]["digest"].as_str().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn lock_is_idempotent_second_run_reports_no_drift() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_path_dep(tmp.path());

        let args = LockArgs {
            workspace: tmp.path(),
            check: false,
        };
        run(&ctx_json(), &args, &mut Vec::new()).unwrap();
        let first = std::fs::read_to_string(tmp.path().join("akua.lock")).unwrap();

        let mut stdout = Vec::new();
        run(&ctx_json(), &args, &mut stdout).unwrap();
        let second = std::fs::read_to_string(tmp.path().join("akua.lock")).unwrap();

        assert_eq!(first, second, "second run diverged: {first} vs {second}");
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["drift"], false);
    }

    #[test]
    fn check_exits_drift_when_lock_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_path_dep(tmp.path());

        // Write a stale lockfile (no deps recorded).
        std::fs::write(tmp.path().join("akua.lock"), "version = 1\n").unwrap();

        let args = LockArgs {
            workspace: tmp.path(),
            check: true,
        };
        let err = run(&ctx_json(), &args, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, LockError::Drift));
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn check_passes_when_lock_matches() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_path_dep(tmp.path());

        // Generate the lock first.
        run(
            &ctx_json(),
            &LockArgs {
                workspace: tmp.path(),
                check: false,
            },
            &mut Vec::new(),
        )
        .unwrap();

        // Check mode should now pass.
        let mut stdout = Vec::new();
        let code = run(
            &ctx_json(),
            &LockArgs {
                workspace: tmp.path(),
                check: true,
            },
            &mut stdout,
        )
        .unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["wrote"], false);
        assert_eq!(parsed["drift"], false);
    }

    #[test]
    fn lock_with_no_deps_is_successful_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("akua.toml"),
            b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();

        let mut stdout = Vec::new();
        run(
            &ctx_json(),
            &LockArgs {
                workspace: tmp.path(),
                check: false,
            },
            &mut stdout,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(parsed["locked_packages"].as_array().unwrap().is_empty());
    }
}
