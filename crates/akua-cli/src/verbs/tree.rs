//! `akua tree` — print the manifest's declared deps + lockfile entries.
//!
//! Flat listing for now (transitive deps land with the resolver). Joins
//! manifest aliases to lockfile rows by name; reports each dep's
//! declared source and (when locked) digest + signature.

use std::io::Write;
use std::path::Path;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::mod_file::{Dependency, DependencySource};
use akua_core::{AkuaLock, AkuaManifest, LockLoadError, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct TreeArgs<'a> {
    pub workspace: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TreeOutput {
    pub package: PackageInfo,
    pub dependencies: Vec<DepRow>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepRow {
    pub name: String,

    /// `"oci"` / `"git"` / `"path"` / `"unknown"`.
    pub source: &'static str,

    /// The actual ref recorded in `akua.toml`.
    pub source_ref: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Lockfile row, present only when `akua.lock` exists and contains
    /// a matching entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<LockedInfo>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockedInfo {
    pub digest: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Local fork override active for this dep. When set, the dep's
    /// canonical source is whatever the manifest declared, but build-
    /// time resolution reads files from `replaced_by`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Lock(#[from] LockLoadError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl TreeError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            TreeError::Manifest(e) => e.to_structured(),
            TreeError::Lock(e) => e.to_structured(),
            TreeError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            TreeError::Manifest(e) if e.is_system() => ExitCode::SystemError,
            TreeError::Lock(e) if e.is_system() => ExitCode::SystemError,
            TreeError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &TreeArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, TreeError> {
    let manifest = AkuaManifest::load(args.workspace)?;

    let lock = if args.workspace.join("akua.lock").exists() {
        Some(AkuaLock::load(args.workspace)?)
    } else {
        None
    };

    let mut deps = Vec::with_capacity(manifest.dependencies.len());
    for (name, dep) in &manifest.dependencies {
        let (source, source_ref) = source_of(dep);
        let locked = lock
            .as_ref()
            .and_then(|l| l.packages.iter().find(|p| p.name == *name))
            .map(|p| LockedInfo {
                digest: p.digest.clone(),
                signature: p.signature.clone(),
                replaced_by: p.replaced.as_ref().map(|r| r.path.clone()),
            });
        deps.push(DepRow {
            name: name.clone(),
            source,
            source_ref,
            version: dep.version.clone(),
            locked,
        });
    }

    let output = TreeOutput {
        package: PackageInfo {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
            edition: manifest.package.edition.clone(),
        },
        dependencies: deps,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(TreeError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn source_of(dep: &Dependency) -> (&'static str, String) {
    match dep.source() {
        Some(DependencySource::Oci) => ("oci", dep.oci.clone().unwrap_or_default()),
        Some(DependencySource::Git) => ("git", dep.git.clone().unwrap_or_default()),
        Some(DependencySource::Path) => ("path", dep.path.clone().unwrap_or_default()),
        None => ("unknown", String::new()),
    }
}

fn write_text<W: Write>(writer: &mut W, output: &TreeOutput) -> std::io::Result<()> {
    writeln!(
        writer,
        "{}@{} ({} deps, edition={})",
        output.package.name,
        output.package.version,
        output.dependencies.len(),
        output.package.edition,
    )?;
    if output.dependencies.is_empty() {
        writeln!(writer, "  (no dependencies)")?;
        return Ok(());
    }
    for dep in &output.dependencies {
        let version = dep
            .version
            .as_deref()
            .map(|v| format!("@{v}"))
            .unwrap_or_default();
        let lock_marker = match &dep.locked {
            Some(l) => {
                let sig_marker = if l.signature.is_some() { "signed" } else { "unsigned" };
                format!(" [locked {} ({})]", short_digest(&l.digest), sig_marker)
            }
            None => String::new(),
        };
        let replace_marker = dep
            .locked
            .as_ref()
            .and_then(|l| l.replaced_by.as_deref())
            .map(|p| format!(" [replace -> {p}]"))
            .unwrap_or_default();
        writeln!(
            writer,
            "  - {}{} ({} {}){}{}",
            dep.name, version, dep.source, dep.source_ref, lock_marker, replace_marker,
        )?;
    }
    Ok(())
}

/// `sha256:abc123…` → `abc12345`. Keeps text-mode tree output legible.
fn short_digest(d: &str) -> String {
    d.strip_prefix("sha256:")
        .map(|rest| rest.chars().take(8).collect::<String>())
        .unwrap_or_else(|| d.chars().take(8).collect())
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
name    = "demo"
version = "0.2.0"
edition = "akua.dev/v1alpha1"

[dependencies]
cnpg = { oci = "oci://ghcr.io/cnpg/charts/cluster", version = "0.20.0" }
local = { path = "../sibling" }
"#;

    const LOCK: &str = r#"
version = 1

[[package]]
name      = "cnpg"
version   = "0.20.0"
source    = "oci://ghcr.io/cnpg/charts/cluster"
digest    = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
signature = "cosign:sigstore:cnpg"
"#;

    fn workspace(toml: &str, lock: Option<&str>) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("akua.toml"), toml).unwrap();
        if let Some(lock) = lock {
            fs::write(tmp.path().join("akua.lock"), lock).unwrap();
        }
        tmp
    }

    fn json_ctx() -> Context {
        Context::json()
    }

    #[test]
    fn lists_declared_deps_without_lockfile() {
        let ws = workspace(MANIFEST, None);
        let mut stdout = Vec::new();
        run(&json_ctx(), &TreeArgs { workspace: ws.path() }, &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["package"]["name"], "demo");
        let deps = parsed["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d["name"] == "cnpg" && d["source"] == "oci"));
        assert!(deps.iter().any(|d| d["name"] == "local" && d["source"] == "path"));
        // No lockfile → no `locked` field.
        assert!(deps[0].get("locked").is_none() || deps[0]["locked"].is_null());
    }

    #[test]
    fn joins_lockfile_rows_when_present() {
        let ws = workspace(MANIFEST, Some(LOCK));
        let mut stdout = Vec::new();
        run(&json_ctx(), &TreeArgs { workspace: ws.path() }, &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        let cnpg = parsed["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "cnpg")
            .expect("cnpg present");
        assert!(cnpg["locked"]["digest"].as_str().unwrap().starts_with("sha256:"));
        assert_eq!(cnpg["locked"]["signature"], "cosign:sigstore:cnpg");

        // `local` isn't in the lockfile → no `locked` block.
        let local = parsed["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "local")
            .expect("local present");
        assert!(local.get("locked").is_none() || local["locked"].is_null());
    }

    #[test]
    fn empty_dependencies_emits_human_readable_marker() {
        let manifest = r#"
[package]
name    = "empty"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
"#;
        let ws = workspace(manifest, None);
        let mut stdout = Vec::new();
        run(&Context::human(), &TreeArgs { workspace: ws.path() }, &mut stdout)
            .expect("run");
        assert!(String::from_utf8(stdout).unwrap().contains("(no dependencies)"));
    }

    #[test]
    fn missing_manifest_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let err = run(
            &Context::human(),
            &TreeArgs { workspace: tmp.path() },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_MANIFEST_MISSING);
    }
}
