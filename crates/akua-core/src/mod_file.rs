//! `akua.toml` — the human-edited manifest.
//!
//! Spec: [`docs/lockfile-format.md §akua.toml`](../../../docs/lockfile-format.md).
//!
//! Two-file package manager split inherited from Go (intent + evidence),
//! with TOML format borrowed from Cargo because our dep forms are richer
//! than go.mod's directives can express cleanly. Companion file is
//! [`crate::lock_file`] (`akua.lock`).
//!
//! This module is **pure parsing and serialization**. No network, no fs
//! walks, no resolution. Digest resolution lives elsewhere (Phase A.6).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The top-level shape of an `akua.toml` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AkuaManifest {
    pub package: PackageSection,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceSection>,

    /// Dependencies keyed by local alias (the name as it appears in `import`
    /// statements). Ordering is preserved on round-trip by using `BTreeMap`.
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// `[package]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageSection {
    pub name: String,
    pub version: String,
    pub edition: String,

    /// When `false`, unsigned deps are permitted in `akua.lock`. Default `true`.
    #[serde(default = "default_strict_signing")]
    pub strict_signing: bool,
}

fn default_strict_signing() -> bool {
    true
}

/// `[workspace]` table, present in monorepos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSection {
    /// Glob patterns resolving to member package directories.
    #[serde(default)]
    pub members: Vec<String>,
}

/// A single dependency. Form is discriminated by which source-type field is
/// set (`oci` / `git` / `path`). Exactly one must be present; all three
/// present is a validation error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    /// OCI ref. Exclusive with `git`, `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci: Option<String>,

    /// Git URL. Exclusive with `oci`, `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,

    /// Local filesystem path. Exclusive with `oci`, `git`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Version constraint (semver exact or range). Required for `oci`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Git tag. Set for `git` deps to pin a specific release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// Git commit SHA. Alternative to `tag` for `git` deps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,

    /// Local-fork override. Keeps the `oci` / `git` ref recorded as the
    /// canonical source; resolves from `replace.path` at build time instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<Replace>,
}

/// Local-fork override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replace {
    pub path: String,
}

/// The source form discriminant, computed from which field is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySource {
    Oci,
    Git,
    Path,
}

/// Errors produced by this module.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("dependency `{name}`: exactly one of oci / git / path must be set, got {count}")]
    AmbiguousSource { name: String, count: usize },

    #[error("dependency `{name}`: oci dep requires a version")]
    OciMissingVersion { name: String },

    #[error("dependency `{name}`: git dep requires either tag or rev")]
    GitMissingTagOrRev { name: String },

    #[error("dependency `{name}`: path dep must not set version / tag / rev")]
    PathHasPin { name: String },

    #[error("[package].edition must start with `akua.dev/`, got `{0}`")]
    BadEdition(String),

    #[error("[package].name must be a valid KCL identifier, got `{0}`")]
    BadPackageName(String),
}

impl AkuaManifest {
    /// Parse an `akua.toml` from a string.
    pub fn parse(s: &str) -> Result<Self, ManifestError> {
        let manifest: AkuaManifest = toml::from_str(s)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Serialize back to canonical TOML. Fields in deterministic order;
    /// dependencies alphabetical (BTreeMap guarantees this).
    pub fn to_toml(&self) -> Result<String, ManifestError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Cross-field validation that serde's structural parse can't catch on
    /// its own.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !self.package.edition.starts_with("akua.dev/") {
            return Err(ManifestError::BadEdition(self.package.edition.clone()));
        }
        if !is_valid_package_name(&self.package.name) {
            return Err(ManifestError::BadPackageName(self.package.name.clone()));
        }
        for (name, dep) in &self.dependencies {
            dep.validate(name)?;
        }
        Ok(())
    }
}

impl Dependency {
    /// Which source form is this? Errors if the triple of `oci` / `git` /
    /// `path` doesn't have exactly one set.
    pub fn source(&self) -> Result<DependencySource, ManifestError> {
        let count = self.oci.is_some() as usize
            + self.git.is_some() as usize
            + self.path.is_some() as usize;
        if count != 1 {
            return Err(ManifestError::AmbiguousSource {
                // This error doesn't have the dep name; callers that know the
                // name wrap it via validate(name) below.
                name: String::new(),
                count,
            });
        }
        if self.oci.is_some() {
            Ok(DependencySource::Oci)
        } else if self.git.is_some() {
            Ok(DependencySource::Git)
        } else {
            Ok(DependencySource::Path)
        }
    }

    /// Full validation. Callers pass the local alias so errors name the dep.
    pub fn validate(&self, name: &str) -> Result<(), ManifestError> {
        let source = self.source().map_err(|e| match e {
            ManifestError::AmbiguousSource { count, .. } => ManifestError::AmbiguousSource {
                name: name.to_string(),
                count,
            },
            other => other,
        })?;
        match source {
            DependencySource::Oci => {
                if self.version.is_none() {
                    return Err(ManifestError::OciMissingVersion {
                        name: name.to_string(),
                    });
                }
            }
            DependencySource::Git => {
                if self.tag.is_none() && self.rev.is_none() {
                    return Err(ManifestError::GitMissingTagOrRev {
                        name: name.to_string(),
                    });
                }
            }
            DependencySource::Path => {
                if self.version.is_some() || self.tag.is_some() || self.rev.is_some() {
                    return Err(ManifestError::PathHasPin {
                        name: name.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Package name rules (aligned with Cargo / npm / poetry conventions):
/// - non-empty
/// - ASCII alphanumeric, `-`, `_` only
/// - must not start with `-` (registry ergonomics)
///
/// Digit-prefixed names are permitted (e.g. `01-hello-webapp`).
fn is_valid_package_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first == '-' {
        return false;
    }
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Lifted from `examples/01-hello-webapp/akua.toml`.
    const EXAMPLE_01: &str = r#"
[package]
name    = "01-hello-webapp"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { oci = "oci://registry-1.docker.io/bitnamicharts/nginx", version = "18.2.0" }
"#;

    /// Exercises workspace + multiple deps + mixed sources.
    const EXAMPLE_WORKSPACE: &str = r#"
[package]
name    = "acme-platform"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[workspace]
members = ["./apps/*", "./policies/*"]

[dependencies]
k8s       = { oci = "oci://ghcr.io/kcl-lang/k8s",              version = "1.31.2" }
cnpg      = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }
our-glue  = { oci = "oci://pkg.acme.internal/glue", version = "0.3.0", replace = { path = "../glue-fork" } }
local-dev = { path = "../shared" }
from-git  = { git = "https://github.com/foo/bar", tag = "v1.2.3" }
"#;

    #[test]
    fn parses_minimal_example() {
        let m = AkuaManifest::parse(EXAMPLE_01).expect("parse");
        assert_eq!(m.package.name, "01-hello-webapp");
        assert_eq!(m.package.version, "0.1.0");
        assert_eq!(m.package.edition, "akua.dev/v1alpha1");
        assert!(m.package.strict_signing);
        assert!(m.workspace.is_none());
        assert_eq!(m.dependencies.len(), 1);
        let nginx = m.dependencies.get("nginx").expect("nginx dep");
        assert_eq!(
            nginx.oci.as_deref(),
            Some("oci://registry-1.docker.io/bitnamicharts/nginx")
        );
        assert_eq!(nginx.version.as_deref(), Some("18.2.0"));
        assert!(nginx.git.is_none());
        assert!(nginx.path.is_none());
    }

    #[test]
    fn parses_workspace_with_mixed_sources() {
        let m = AkuaManifest::parse(EXAMPLE_WORKSPACE).expect("parse");
        assert_eq!(
            m.workspace.as_ref().unwrap().members,
            vec!["./apps/*".to_string(), "./policies/*".to_string()]
        );
        assert_eq!(m.dependencies.len(), 5);

        assert_eq!(
            m.dependencies["our-glue"].replace.as_ref().unwrap().path,
            "../glue-fork"
        );

        assert_eq!(
            m.dependencies["local-dev"].source().unwrap(),
            DependencySource::Path
        );
        assert_eq!(
            m.dependencies["from-git"].source().unwrap(),
            DependencySource::Git
        );
        assert_eq!(
            m.dependencies["k8s"].source().unwrap(),
            DependencySource::Oci
        );
    }

    #[test]
    fn round_trips_canonical_form() {
        let original = AkuaManifest::parse(EXAMPLE_WORKSPACE).expect("parse");
        let serialized = original.to_toml().expect("serialize");
        let reparsed = AkuaManifest::parse(&serialized).expect("reparse");
        assert_eq!(original, reparsed, "round-trip should preserve structure");
    }

    #[test]
    fn rejects_ambiguous_source() {
        let bad = r#"
[package]
name    = "bad"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
twosrc = { oci = "oci://foo", git = "https://example.com/bar", version = "1.0.0", tag = "v1" }
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("twosrc"), "error should name the dep: {msg}");
        assert!(msg.contains("exactly one"), "err: {msg}");
    }

    #[test]
    fn rejects_oci_without_version() {
        let bad = r#"
[package]
name    = "bad"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
bare = { oci = "oci://foo" }
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::OciMissingVersion { ref name } if name == "bare"),
            "expected OciMissingVersion, got {err:?}"
        );
    }

    #[test]
    fn rejects_git_without_tag_or_rev() {
        let bad = r#"
[package]
name    = "bad"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
bare-git = { git = "https://github.com/foo/bar" }
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::GitMissingTagOrRev { ref name } if name == "bare-git"),
            "expected GitMissingTagOrRev, got {err:?}"
        );
    }

    #[test]
    fn rejects_path_with_version() {
        let bad = r#"
[package]
name    = "bad"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
pinned-path = { path = "../foo", version = "1.0.0" }
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::PathHasPin { ref name } if name == "pinned-path"),
            "expected PathHasPin, got {err:?}"
        );
    }

    #[test]
    fn rejects_bad_edition() {
        let bad = r#"
[package]
name    = "fine"
version = "0.1.0"
edition = "cargo/v1"

[dependencies]
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::BadEdition(ref e) if e == "cargo/v1"),
            "expected BadEdition, got {err:?}"
        );
    }

    #[test]
    fn rejects_bad_package_name_has_space() {
        let bad = r#"
[package]
name    = "has space"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::BadPackageName(ref n) if n == "has space"),
            "expected BadPackageName, got {err:?}"
        );
    }

    #[test]
    fn package_name_validation() {
        // Allowed
        assert!(is_valid_package_name("webapp"));
        assert!(is_valid_package_name("web-app"));
        assert!(is_valid_package_name("web_app_123"));
        assert!(is_valid_package_name("_leading_underscore"));
        assert!(is_valid_package_name("01-hello-webapp")); // digit-prefix OK
        assert!(is_valid_package_name("9starts-with-digit"));

        // Disallowed
        assert!(!is_valid_package_name(""));
        assert!(!is_valid_package_name("-leading-hyphen"));
        assert!(!is_valid_package_name("has space"));
        assert!(!is_valid_package_name("has.dot"));
    }

    #[test]
    fn rejects_bad_package_name_leading_hyphen() {
        let bad = r#"
[package]
name    = "-bad-name"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
"#;
        let err = AkuaManifest::parse(bad).unwrap_err();
        assert!(
            matches!(err, ManifestError::BadPackageName(ref n) if n == "-bad-name"),
            "expected BadPackageName, got {err:?}"
        );
    }

    #[test]
    fn strict_signing_defaults_to_true() {
        let m = AkuaManifest::parse(EXAMPLE_01).expect("parse");
        assert!(m.package.strict_signing);
    }

    #[test]
    fn strict_signing_can_be_disabled() {
        let s = r#"
[package]
name           = "unsigned-ok"
version        = "0.1.0"
edition        = "akua.dev/v1alpha1"
strict_signing = false

[dependencies]
"#;
        let m = AkuaManifest::parse(s).expect("parse");
        assert!(!m.package.strict_signing);
    }
}
