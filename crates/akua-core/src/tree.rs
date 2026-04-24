//! Pure-compute implementation of `akua tree`: walk an already-
//! parsed `AkuaManifest` + optional `AkuaLock` and produce the
//! typed output shape. No filesystem access — CLI + SDK read files
//! at their own layers.
//!
//! Spec: [`docs/cli.md akua tree`](../../../../docs/cli.md#akua-tree).

use serde::Serialize;

use crate::mod_file::{Dependency, DependencySource};
use crate::{AkuaLock, AkuaManifest};

crate::contract_type! {
/// Output shape for `akua tree --json`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TreeOutput {
    pub package: PackageInfo,
    pub dependencies: Vec<DepRow>,
}
}

crate::contract_type! {
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub edition: String,
}
}

crate::contract_type! {
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepRow {
    pub name: String,

    /// `"oci"` / `"git"` / `"path"` / `"unknown"`.
    pub source: &'static str,

    /// The actual ref recorded in `akua.toml`.
    pub source_ref: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Lockfile row, present only when `akua.lock` exists and
    /// contains a matching entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<LockedInfo>,
}
}

crate::contract_type! {
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockedInfo {
    pub digest: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Local fork override active for this dep. When set, the dep's
    /// canonical source is whatever the manifest declared, but build-
    /// time resolution reads files from `replaced_by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}
}

/// Walk a parsed manifest + optional lock and produce the tree output.
pub fn tree_from_parsed(manifest: &AkuaManifest, lock: Option<&AkuaLock>) -> TreeOutput {
    let mut deps = Vec::with_capacity(manifest.dependencies.len());
    for (name, dep) in &manifest.dependencies {
        let (source, source_ref) = source_of(dep);
        let locked = lock
            .and_then(|l| l.packages.iter().find(|p| &p.name == name))
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
    TreeOutput {
        package: PackageInfo {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
            edition: manifest.package.edition.clone(),
        },
        dependencies: deps,
    }
}

/// Convenience: parse `akua.toml` + optional `akua.lock` strings and
/// produce the tree output. The CLI reads files and calls this; the
/// WASM bundle exposes the same entry to JS.
pub fn tree_from_sources(
    manifest: &str,
    lock: Option<&str>,
) -> Result<TreeOutput, TreeSourceError> {
    let manifest = AkuaManifest::parse(manifest).map_err(TreeSourceError::Manifest)?;
    let lock = lock
        .map(AkuaLock::parse)
        .transpose()
        .map_err(TreeSourceError::Lock)?;
    Ok(tree_from_parsed(&manifest, lock.as_ref()))
}

#[derive(Debug, thiserror::Error)]
pub enum TreeSourceError {
    #[error(transparent)]
    Manifest(crate::mod_file::ManifestError),

    #[error(transparent)]
    Lock(crate::lock_file::LockError),
}

fn source_of(dep: &Dependency) -> (&'static str, String) {
    match dep.source() {
        Some(DependencySource::Oci) => ("oci", dep.oci.clone().unwrap_or_default()),
        Some(DependencySource::Git) => ("git", dep.git.clone().unwrap_or_default()),
        Some(DependencySource::Path) => ("path", dep.path.clone().unwrap_or_default()),
        None => ("unknown", String::new()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_MANIFEST: &str = r#"
[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
"#;

    const MANIFEST_WITH_DEP: &str = r#"
[package]
name = "smoke"
version = "0.0.1"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { path = "./vendor/nginx" }
"#;

    const LOCK_WITH_NGINX: &str = r#"version = 1

[[package]]
name = "nginx"
version = "0.0.1"
digest = "sha256:abc"
source = "path+file:///tmp/nginx"
"#;

    #[test]
    fn empty_manifest_yields_zero_deps() {
        let out = tree_from_sources(EMPTY_MANIFEST, None).expect("parse");
        assert_eq!(out.package.name, "smoke");
        assert_eq!(out.package.version, "0.0.1");
        assert!(out.dependencies.is_empty());
    }

    #[test]
    fn dep_appears_in_list_unlocked_when_no_lockfile() {
        let out = tree_from_sources(MANIFEST_WITH_DEP, None).expect("parse");
        assert_eq!(out.dependencies.len(), 1);
        let nginx = &out.dependencies[0];
        assert_eq!(nginx.name, "nginx");
        assert_eq!(nginx.source, "path");
        assert!(nginx.locked.is_none());
    }

    #[test]
    fn dep_merges_with_lockfile_when_present() {
        let out = tree_from_sources(MANIFEST_WITH_DEP, Some(LOCK_WITH_NGINX)).expect("parse");
        let nginx = &out.dependencies[0];
        assert!(nginx.locked.is_some());
        let locked = nginx.locked.as_ref().unwrap();
        assert_eq!(locked.digest, "sha256:abc");
    }

    #[test]
    fn bad_toml_surfaces_typed_error() {
        assert!(tree_from_sources("not toml [[[", None).is_err());
    }
}
