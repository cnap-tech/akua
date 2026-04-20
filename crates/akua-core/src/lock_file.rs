//! `akua.lock` — the machine-maintained lockfile.
//!
//! Spec: [`docs/lockfile-format.md §akua.lock`](../../../docs/lockfile-format.md).
//!
//! Cargo.lock-flavored TOML: one `[[package]]` array entry per resolved
//! artifact, alphabetical by `name`. The schema is versioned (`version = 1`
//! at the top) so the format can evolve without breaking old tools.
//!
//! This module is **pure parsing and serialization**. No network, no digest
//! computation, no signature verification. Those live in the digest/attest
//! paths (Phase A.6) and consume the parsed structure here.

use serde::{Deserialize, Serialize};

/// Current lockfile schema version. Bumped on breaking changes.
pub const CURRENT_VERSION: u32 = 1;

/// The top-level shape of an `akua.lock` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AkuaLock {
    /// Lockfile format version. Current is [`CURRENT_VERSION`].
    pub version: u32,

    /// Resolved packages. Alphabetical by `name`; one entry per resolved
    /// `(name, version)` tuple.
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
}

/// A single resolved dependency, pinned by digest + signature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,

    /// Full source ref: `oci://…`, `git+https://…`, or `path+file://…`.
    pub source: String,

    /// Content-addressable hash: `sha256:` for OCI; sha256 over tarball
    /// for git.
    pub digest: String,

    /// Cosign signature. Keyless: `cosign:sigstore:<issuer>`. Keyed:
    /// `cosign:key:<identity>`. May be absent only when the consuming
    /// workspace's manifest has `strict_signing = false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Transitive dependency edges as `"<name>@<version>"` strings, for
    /// graph walks during `akua verify`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,

    /// SLSA attestation digest; present when the dep's author publishes
    /// one alongside the artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<String>,

    /// Local replace override; present when the workspace's `akua.toml`
    /// applied a `replace = { path = "…" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<Replaced>,

    /// Retracted version flag. Ignored by build; surfaced by `akua verify`
    /// as a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yanked: Option<bool>,

    /// Kyverno-specific: the original Kyverno source digest kept for audit
    /// when a Kyverno bundle was converted to Rego at `akua add` time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kyverno_source_digest: Option<String>,

    /// Kyverno-specific: the Kyverno→Rego converter version used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converter_version: Option<String>,
}

/// Local fork marker written into `akua.lock` when a `replace` directive
/// from `akua.toml` is active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replaced {
    pub path: String,
}

/// Errors produced by this module.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("unsupported lockfile version {found}; this akua understands version {expected}")]
    VersionMismatch { found: u32, expected: u32 },

    #[error("package entries must be alphabetical by name; `{later}` comes before `{earlier}`")]
    UnsortedPackages { later: String, earlier: String },

    #[error("package `{0}` appears multiple times at the same version")]
    DuplicatePackage(String),

    #[error("package `{0}`: digest must start with `sha256:`")]
    BadDigest(String),
}

impl AkuaLock {
    /// Parse an `akua.lock` from a string. Performs format-version,
    /// alphabetical-order, and digest-prefix checks.
    pub fn parse(s: &str) -> Result<Self, LockError> {
        let lock: AkuaLock = toml::from_str(s)?;
        lock.validate()?;
        Ok(lock)
    }

    /// Serialize back to canonical TOML. Package order is what the caller
    /// provided; callers are expected to have sorted via [`sort`] or to
    /// have produced the lock in sorted order in the first place.
    pub fn to_toml(&self) -> Result<String, LockError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Validate structural invariants not caught by serde.
    pub fn validate(&self) -> Result<(), LockError> {
        if self.version != CURRENT_VERSION {
            return Err(LockError::VersionMismatch {
                found: self.version,
                expected: CURRENT_VERSION,
            });
        }
        // Alphabetical + uniqueness.
        for window in self.packages.windows(2) {
            let a = &window[0];
            let b = &window[1];
            if b.name < a.name {
                return Err(LockError::UnsortedPackages {
                    later: b.name.clone(),
                    earlier: a.name.clone(),
                });
            }
            if a.name == b.name && a.version == b.version {
                return Err(LockError::DuplicatePackage(a.name.clone()));
            }
        }
        for pkg in &self.packages {
            // Digest check: OCI + git sources use sha256. If we ever add a
            // non-sha256 algorithm, this check grows.
            if !pkg.digest.starts_with("sha256:") {
                return Err(LockError::BadDigest(pkg.name.clone()));
            }
        }
        Ok(())
    }

    /// Sort packages alphabetically by (name, version). Call this after
    /// mutating the lock; `validate` will reject unsorted locks.
    pub fn sort(&mut self) {
        self.packages
            .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
    }

    /// Construct an empty lock at the current version.
    pub fn empty() -> Self {
        AkuaLock {
            version: CURRENT_VERSION,
            packages: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_02: &str = r#"
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

    const EXAMPLE_WITH_TRANSITIVE: &str = r#"
version = 1

[[package]]
name      = "common"
version   = "2.20.0"
source    = "oci://ghcr.io/bitnamicharts/common"
digest    = "sha256:f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0"
signature = "cosign:sigstore:bitnamicharts"

[[package]]
name      = "webapp"
version   = "2.1.0"
source    = "oci://ghcr.io/acme/charts/webapp"
digest    = "sha256:a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
signature = "cosign:key:acme"
dependencies = ["common@2.20.0"]
"#;

    const EXAMPLE_ATTESTATION_AND_KYVERNO: &str = r#"
version = 1

[[package]]
name      = "kyv-sec"
version   = "2.0.0"
source    = "oci://policies.akua.dev/kyverno/security"
digest    = "sha256:1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b"
signature = "cosign:sigstore:akua-release"
kyverno_source_digest = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
converter_version = "akua-kyverno-rego@1.4.2"

[[package]]
name        = "platform-base"
version     = "1.0.0"
source      = "oci://pkg.acme.corp/platform-base"
digest      = "sha256:c7e4b8a1f3d5e6a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2"
signature   = "cosign:key:platform-team-acme"
attestation = "sha256:e9d2c7f1a3b5c7d9e1f3a5b7c9d1e3f5a7b9c1d3e5f7a9b1c3d5e7f9a1b3c5d7"
"#;

    #[test]
    fn parses_basic_lock() {
        let lock = AkuaLock::parse(EXAMPLE_02).expect("parse");
        assert_eq!(lock.version, 1);
        assert_eq!(lock.packages.len(), 2);
        assert_eq!(lock.packages[0].name, "cnpg");
        assert_eq!(lock.packages[1].name, "webapp");
        assert!(lock.packages[0].dependencies.is_empty());
    }

    #[test]
    fn parses_transitive_deps() {
        let lock = AkuaLock::parse(EXAMPLE_WITH_TRANSITIVE).expect("parse");
        let webapp = lock.packages.iter().find(|p| p.name == "webapp").unwrap();
        assert_eq!(webapp.dependencies, vec!["common@2.20.0".to_string()]);
    }

    #[test]
    fn parses_attestation_and_kyverno_fields() {
        let lock = AkuaLock::parse(EXAMPLE_ATTESTATION_AND_KYVERNO).expect("parse");
        let base = lock
            .packages
            .iter()
            .find(|p| p.name == "platform-base")
            .unwrap();
        assert!(base.attestation.is_some());
        let kyv = lock.packages.iter().find(|p| p.name == "kyv-sec").unwrap();
        assert!(kyv.kyverno_source_digest.is_some());
        assert_eq!(kyv.converter_version.as_deref(), Some("akua-kyverno-rego@1.4.2"));
    }

    #[test]
    fn round_trips() {
        let original = AkuaLock::parse(EXAMPLE_WITH_TRANSITIVE).expect("parse");
        let serialized = original.to_toml().expect("serialize");
        let reparsed = AkuaLock::parse(&serialized).expect("reparse");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn rejects_version_mismatch() {
        let bad = r#"
version = 99

[[package]]
name    = "x"
version = "1.0"
source  = "oci://x"
digest  = "sha256:00"
"#;
        let err = AkuaLock::parse(bad).unwrap_err();
        assert!(matches!(err, LockError::VersionMismatch { found: 99, .. }));
    }

    #[test]
    fn rejects_unsorted_packages() {
        let bad = r#"
version = 1

[[package]]
name    = "webapp"
version = "2.1.0"
source  = "oci://a"
digest  = "sha256:00"

[[package]]
name    = "cnpg"
version = "0.20.0"
source  = "oci://b"
digest  = "sha256:11"
"#;
        let err = AkuaLock::parse(bad).unwrap_err();
        assert!(matches!(err, LockError::UnsortedPackages { .. }));
    }

    #[test]
    fn rejects_duplicate_same_version() {
        let bad = r#"
version = 1

[[package]]
name    = "foo"
version = "1.0"
source  = "oci://a"
digest  = "sha256:00"

[[package]]
name    = "foo"
version = "1.0"
source  = "oci://b"
digest  = "sha256:11"
"#;
        let err = AkuaLock::parse(bad).unwrap_err();
        assert!(matches!(err, LockError::DuplicatePackage(ref n) if n == "foo"));
    }

    #[test]
    fn allows_same_name_different_versions() {
        let ok = r#"
version = 1

[[package]]
name    = "foo"
version = "1.0"
source  = "oci://a"
digest  = "sha256:00"

[[package]]
name    = "foo"
version = "2.0"
source  = "oci://b"
digest  = "sha256:11"
"#;
        let lock = AkuaLock::parse(ok).expect("parse");
        assert_eq!(lock.packages.len(), 2);
    }

    #[test]
    fn rejects_non_sha256_digest() {
        let bad = r#"
version = 1

[[package]]
name    = "x"
version = "1.0"
source  = "oci://x"
digest  = "md5:deadbeef"
"#;
        let err = AkuaLock::parse(bad).unwrap_err();
        assert!(matches!(err, LockError::BadDigest(ref n) if n == "x"));
    }

    #[test]
    fn sort_orders_by_name_then_version() {
        let mut lock = AkuaLock::empty();
        lock.packages.push(LockedPackage {
            name: "b".to_string(),
            version: "1.0".to_string(),
            source: "oci://b".to_string(),
            digest: "sha256:00".to_string(),
            signature: None,
            dependencies: vec![],
            attestation: None,
            replaced: None,
            yanked: None,
            kyverno_source_digest: None,
            converter_version: None,
        });
        lock.packages.push(LockedPackage {
            name: "a".to_string(),
            version: "2.0".to_string(),
            source: "oci://a".to_string(),
            digest: "sha256:11".to_string(),
            signature: None,
            dependencies: vec![],
            attestation: None,
            replaced: None,
            yanked: None,
            kyverno_source_digest: None,
            converter_version: None,
        });
        lock.packages.push(LockedPackage {
            name: "a".to_string(),
            version: "1.0".to_string(),
            source: "oci://a".to_string(),
            digest: "sha256:22".to_string(),
            signature: None,
            dependencies: vec![],
            attestation: None,
            replaced: None,
            yanked: None,
            kyverno_source_digest: None,
            converter_version: None,
        });
        lock.sort();
        assert_eq!(lock.packages[0].name, "a");
        assert_eq!(lock.packages[0].version, "1.0");
        assert_eq!(lock.packages[1].name, "a");
        assert_eq!(lock.packages[1].version, "2.0");
        assert_eq!(lock.packages[2].name, "b");
    }

    #[test]
    fn empty_lock_validates() {
        let lock = AkuaLock::empty();
        assert!(lock.validate().is_ok());
    }

    #[test]
    fn missing_signature_is_accepted_at_parse_time() {
        // strict_signing enforcement happens at the cross-file validation
        // layer (akua.toml ↔ akua.lock), not at lockfile parse.
        let s = r#"
version = 1

[[package]]
name    = "unsigned"
version = "1.0"
source  = "oci://foo"
digest  = "sha256:00"
"#;
        let lock = AkuaLock::parse(s).expect("parse");
        assert!(lock.packages[0].signature.is_none());
    }
}
