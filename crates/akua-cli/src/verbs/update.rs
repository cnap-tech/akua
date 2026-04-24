//! `akua update` — intentionally bump `akua.lock` against whatever
//! upstream now serves.
//!
//! Distinct from [`super::lock`]: where `akua lock` fails hard on
//! OCI digest drift (security — registry served different bytes than
//! the last pinned digest), `akua update` accepts the drift and
//! records the new digest. Operators invoke `update` when they
//! *want* the refresh.
//!
//! Cargo analogue: `cargo update` (without `--locked`).
//!
//! `--dep <name>` scopes the write to a single entry — mirrors
//! `cargo update -p foo`. The resolver still evaluates the whole
//! dep graph (there's no partial resolve), but the lockfile only
//! replaces the named entry.

use std::io::Write;
use std::path::Path;

use akua_core::chart_resolver::{self, ChartResolveError, ResolvedSource, ResolverOptions};
use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{AkuaLock, AkuaManifest, LockLoadError, ManifestLoadError};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct UpdateArgs<'a> {
    pub workspace: &'a Path,
    /// When `Some`, only the named dep is written to the lock;
    /// others retain their prior digests. `None` refreshes all.
    pub dep: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateOutput {
    pub updated: Vec<UpdatedEntry>,
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdatedEntry {
    pub name: String,
    pub version: String,
    pub from_digest: Option<String>,
    pub to_digest: String,
    pub source_kind: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Lock(#[from] LockLoadError),

    #[error(transparent)]
    Resolve(#[from] ChartResolveError),

    #[error("no dep named `{name}` declared in akua.toml")]
    UnknownDep { name: String },

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl UpdateError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            UpdateError::Manifest(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            UpdateError::Lock(e) => {
                StructuredError::new(codes::E_LOCK_PARSE, e.to_string()).with_default_docs()
            }
            UpdateError::Resolve(e) => {
                StructuredError::new(codes::E_CHART_RESOLVE, e.to_string()).with_default_docs()
            }
            UpdateError::UnknownDep { .. } => {
                StructuredError::new(codes::E_ADD_INVALID_DEP, self.to_string())
                    .with_default_docs()
            }
            UpdateError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            UpdateError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &UpdateArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, UpdateError> {
    let manifest = AkuaManifest::load(args.workspace)?;

    if let Some(name) = args.dep {
        if !manifest.dependencies.contains_key(name) {
            return Err(UpdateError::UnknownDep {
                name: name.to_string(),
            });
        }
    }

    let prior_lock = AkuaLock::load(args.workspace).unwrap_or_else(|_| AkuaLock::empty());

    // Deliberately empty — that's the whole point of update vs lock.
    // A fresh resolve with no expected_digests accepts whatever the
    // registry / git remote / path dep serves right now.
    let opts = ResolverOptions {
        offline: false,
        cache_root: None,
        expected_digests: BTreeMap::new(),
        cosign_public_key_pem: None,
    };

    let resolved = chart_resolver::resolve_with_options(&manifest, args.workspace, &opts)?;

    let prior_by_name: BTreeMap<String, String> = prior_lock
        .packages
        .iter()
        .map(|p| (p.name.clone(), p.digest.clone()))
        .collect();

    let mut new_lock = prior_lock.clone();
    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut skipped = Vec::new();

    for chart in resolved.entries.values() {
        // Under --dep, everything else is retained verbatim from the
        // prior lock. The resolver still had to run (it fills fresh
        // digests for the named dep), but merge_into_lock is gated.
        if let Some(target) = args.dep {
            if chart.name != target {
                skipped.push(chart.name.clone());
                continue;
            }
        }

        let prior_digest = prior_by_name.get(&chart.name).cloned();
        let to_digest = chart.sha256.clone();
        if prior_digest.as_deref() == Some(&to_digest) {
            unchanged.push(chart.name.clone());
        } else {
            let (_source_str, version, _replace) = chart.source.to_locked_fields();
            updated.push(UpdatedEntry {
                name: chart.name.clone(),
                version,
                from_digest: prior_digest,
                to_digest,
                source_kind: source_kind_for(&chart.source),
            });
        }
    }

    // Merge the resolved charts that pass the dep filter. `--dep`
    // mode: only the named entry is merged; the rest stay untouched.
    // No --dep: everything merges (the standard update flow).
    let scoped = match args.dep {
        Some(target) => scoped_resolved(&resolved, target),
        None => resolved,
    };
    chart_resolver::merge_into_lock(&mut new_lock, &scoped);
    new_lock.save(args.workspace)?;

    updated.sort_by(|a, b| a.name.cmp(&b.name));
    unchanged.sort();
    skipped.sort();

    let output = UpdateOutput {
        updated,
        unchanged,
        skipped,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(UpdateError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn scoped_resolved(
    resolved: &chart_resolver::ResolvedCharts,
    target: &str,
) -> chart_resolver::ResolvedCharts {
    let mut entries = BTreeMap::new();
    if let Some(chart) = resolved.entries.get(target) {
        entries.insert(target.to_string(), chart.clone());
    }
    chart_resolver::ResolvedCharts { entries }
}

fn source_kind_for(src: &ResolvedSource) -> &'static str {
    match src {
        ResolvedSource::Path { .. } => "path",
        ResolvedSource::Oci { .. } | ResolvedSource::OciReplaced { .. } => "oci",
        ResolvedSource::Git { .. } | ResolvedSource::GitReplaced { .. } => "git",
    }
}

fn write_text<W: Write>(w: &mut W, out: &UpdateOutput) -> std::io::Result<()> {
    if out.updated.is_empty() && out.unchanged.is_empty() && out.skipped.is_empty() {
        writeln!(w, "no dependencies declared")?;
        return Ok(());
    }
    if !out.updated.is_empty() {
        writeln!(w, "updated {} package(s):", out.updated.len())?;
        for u in &out.updated {
            match &u.from_digest {
                Some(from) => writeln!(
                    w,
                    "  [{}] {} {}  {} → {}",
                    u.source_kind, u.name, u.version, from, u.to_digest
                )?,
                None => writeln!(
                    w,
                    "  [{}] {} {}  (new) → {}",
                    u.source_kind, u.name, u.version, u.to_digest
                )?,
            }
        }
    }
    if !out.unchanged.is_empty() {
        writeln!(w, "unchanged: {}", out.unchanged.join(", "))?;
    }
    if !out.skipped.is_empty() {
        writeln!(w, "skipped (--dep filter): {}", out.skipped.join(", "))?;
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

    fn workspace_with_two_path_deps(tmp: &Path) {
        std::fs::create_dir_all(tmp.join("vendor/nginx")).unwrap();
        std::fs::create_dir_all(tmp.join("vendor/redis")).unwrap();
        std::fs::write(
            tmp.join("vendor/nginx/Chart.yaml"),
            b"apiVersion: v2\nname: nginx\nversion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("vendor/redis/Chart.yaml"),
            b"apiVersion: v2\nname: redis\nversion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("akua.toml"),
            br#"
[package]
name = "update-test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { path = "./vendor/nginx" }
redis = { path = "./vendor/redis" }
"#,
        )
        .unwrap();
    }

    #[test]
    fn update_writes_fresh_lock_and_marks_all_new_on_first_run() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_two_path_deps(tmp.path());

        let mut stdout = Vec::new();
        let args = UpdateArgs {
            workspace: tmp.path(),
            dep: None,
        };
        run(&ctx_json(), &args, &mut stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let updated = parsed["updated"].as_array().unwrap();
        assert_eq!(updated.len(), 2);
        for entry in updated {
            assert!(entry["from_digest"].is_null());
            assert!(entry["to_digest"].as_str().unwrap().starts_with("sha256:"));
        }
    }

    #[test]
    fn update_reports_unchanged_when_path_content_didnt_move() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_two_path_deps(tmp.path());

        // Seed the lockfile.
        let args = UpdateArgs {
            workspace: tmp.path(),
            dep: None,
        };
        run(&ctx_json(), &args, &mut Vec::new()).unwrap();

        // Second run — no chart content changed, so nothing bumps.
        let mut stdout = Vec::new();
        run(&ctx_json(), &args, &mut stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(parsed["updated"].as_array().unwrap().is_empty());
        assert_eq!(parsed["unchanged"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn update_detects_path_dep_content_change_as_bump() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_two_path_deps(tmp.path());

        run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: None,
            },
            &mut Vec::new(),
        )
        .unwrap();

        // Mutate one chart's content — bumps its digest.
        std::fs::write(
            tmp.path().join("vendor/nginx/values.yaml"),
            b"image: new\n",
        )
        .unwrap();

        let mut stdout = Vec::new();
        run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: None,
            },
            &mut stdout,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let updated = parsed["updated"].as_array().unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0]["name"], "nginx");
        assert!(updated[0]["from_digest"].as_str().unwrap().starts_with("sha256:"));
        assert!(updated[0]["to_digest"].as_str().unwrap().starts_with("sha256:"));
        assert_ne!(updated[0]["from_digest"], updated[0]["to_digest"]);
    }

    #[test]
    fn dep_filter_touches_only_named_entry() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_two_path_deps(tmp.path());

        // Seed.
        run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: None,
            },
            &mut Vec::new(),
        )
        .unwrap();
        let seeded = std::fs::read_to_string(tmp.path().join("akua.lock")).unwrap();

        // Mutate BOTH charts; then `update --dep nginx` should only
        // refresh nginx's entry. redis's prior (stale) digest stays.
        std::fs::write(
            tmp.path().join("vendor/nginx/values.yaml"),
            b"image: n\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("vendor/redis/values.yaml"),
            b"image: r\n",
        )
        .unwrap();

        let mut stdout = Vec::new();
        run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: Some("nginx"),
            },
            &mut stdout,
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let updated = parsed["updated"].as_array().unwrap();
        let skipped = parsed["skipped"].as_array().unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0]["name"], "nginx");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0], "redis");

        // Post-state: redis's on-disk digest matches the seeded one
        // (stale), not the freshly-mutated one.
        let after = std::fs::read_to_string(tmp.path().join("akua.lock")).unwrap();
        assert_ne!(after, seeded, "lock should have been rewritten");
        // redis's digest in `after` == redis's digest in `seeded`.
        // Cheap check: the seeded redis block is still a substring.
        let seeded_redis_line = seeded
            .lines()
            .find(|l| l.contains("redis"))
            .expect("redis in seeded");
        assert!(
            after.contains(seeded_redis_line),
            "redis line should be unchanged\nseeded: {seeded}\nafter: {after}"
        );
    }

    #[test]
    fn unknown_dep_surfaces_user_error() {
        let tmp = tempfile::tempdir().unwrap();
        workspace_with_two_path_deps(tmp.path());
        let err = run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: Some("does-not-exist"),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, UpdateError::UnknownDep { .. }));
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn update_with_no_deps_is_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("akua.toml"),
            b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();

        let mut stdout = Vec::new();
        run(
            &ctx_json(),
            &UpdateArgs {
                workspace: tmp.path(),
                dep: None,
            },
            &mut stdout,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(parsed["updated"].as_array().unwrap().is_empty());
        assert!(parsed["unchanged"].as_array().unwrap().is_empty());
    }
}
