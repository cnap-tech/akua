//! SLSA v1 provenance predicate + in-toto v1 statement builder.
//!
//! Phase 7 B. When `akua publish` signs an artifact, it also produces
//! an attestation describing *how* the artifact was built: which
//! `akua` version invoked the build, what the invocation shape was,
//! which source materials (resolved chart deps) went in. The
//! attestation is wrapped in a DSSE envelope (Phase 7 B cosign
//! addition) and pushed as an `.att` sidecar.
//!
//! Scope (Phase 7 B slice 1):
//!
//! - **SLSA v1.0** provenance, `build type = https://akua.dev/slsa/publish/v1`.
//! - Materials come from `akua.lock`'s resolved digests.
//! - No builder identity proofs (Phase 6 B sigstore/fulcio).
//! - No recursive chain walk on pull-verify (slice 2).
//!
//! Spec references:
//! - <https://slsa.dev/spec/v1.0/provenance>
//! - <https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md>

use serde::{Deserialize, Serialize};

use crate::lock_file::AkuaLock;

/// in-toto statement wrapping a SLSA provenance predicate. Serializes
/// to the canonical v1.0 statement JSON; that's the payload the DSSE
/// envelope signs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InTotoStatement {
    #[serde(rename = "_type")]
    pub ty: String,

    /// One-element subject: the OCI artifact the attestation describes.
    pub subject: Vec<Subject>,

    #[serde(rename = "predicateType")]
    pub predicate_type: String,

    pub predicate: SlsaProvenance,
}

/// in-toto subject: a name + a digest map (we always emit `sha256`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    /// OCI ref without the scheme (e.g. `ghcr.io/acme/app`).
    pub name: String,
    pub digest: std::collections::BTreeMap<String, String>,
}

/// SLSA v1.0 provenance predicate. Lean shape — only fields we
/// actually populate. `buildDefinition` tells a consumer "who built
/// this + how", `runDetails` tells them "in what environment."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlsaProvenance {
    #[serde(rename = "buildDefinition")]
    pub build_definition: BuildDefinition,
    #[serde(rename = "runDetails")]
    pub run_details: RunDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDefinition {
    /// URI identifying how to interpret the remaining fields. akua's
    /// own build type — consumers who recognize it get structured
    /// parsing, consumers who don't still get the hash chain.
    #[serde(rename = "buildType")]
    pub build_type: String,

    /// The invocation that produced the artifact. Only includes
    /// declarative inputs (oci_ref, tag) — no env vars, no host info.
    #[serde(rename = "externalParameters")]
    pub external_parameters: ExternalParameters,

    /// Resolved dependencies that materially affected the artifact.
    /// Populated from `akua.lock` at publish time. `default` on
    /// deserialize so an attestation that came from a pre-lockfile
    /// workspace still parses.
    #[serde(
        rename = "resolvedDependencies",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub resolved_dependencies: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalParameters {
    /// `oci://<registry>/<repo>` the artifact was published under.
    #[serde(rename = "ociRef")]
    pub oci_ref: String,
    pub tag: String,
}

/// in-toto v1 ResourceDescriptor — used for both materials + products.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDescriptor {
    pub name: String,
    pub digest: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDetails {
    pub builder: Builder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Builder {
    /// Canonical identifier for this akua version. Mirrors
    /// `{"id": "https://akua.dev/cli/v<X.Y.Z>"}`.
    pub id: String,
}

pub const IN_TOTO_STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
pub const SLSA_PROVENANCE_PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
pub const AKUA_BUILD_TYPE: &str = "https://akua.dev/slsa/publish/v1";

/// Build an attestation for an artifact we just published.
/// `manifest_digest` is the `sha256:<hex>` of the OCI manifest —
/// what cosign already signs + what the in-toto subject pins.
/// `subject_name` is the docker-reference (OCI ref without scheme).
pub fn build_publish_attestation(
    subject_name: &str,
    manifest_digest: &str,
    oci_ref: &str,
    tag: &str,
    lock: Option<&AkuaLock>,
) -> InTotoStatement {
    let mut subject_digest = std::collections::BTreeMap::new();
    // in-toto digest maps use the raw algorithm name ("sha256"), not
    // the `sha256:` prefix some OCI fields carry.
    let (algo, hex) = split_digest(manifest_digest);
    subject_digest.insert(algo.to_string(), hex.to_string());

    let resolved_dependencies = lock
        .map(|l| l.packages.iter().map(lock_entry_to_resource).collect())
        .unwrap_or_default();

    InTotoStatement {
        ty: IN_TOTO_STATEMENT_TYPE.to_string(),
        subject: vec![Subject {
            name: subject_name.to_string(),
            digest: subject_digest,
        }],
        predicate_type: SLSA_PROVENANCE_PREDICATE_TYPE.to_string(),
        predicate: SlsaProvenance {
            build_definition: BuildDefinition {
                build_type: AKUA_BUILD_TYPE.to_string(),
                external_parameters: ExternalParameters {
                    oci_ref: oci_ref.to_string(),
                    tag: tag.to_string(),
                },
                resolved_dependencies,
            },
            run_details: RunDetails {
                builder: Builder {
                    id: format!("https://akua.dev/cli/v{}", env!("CARGO_PKG_VERSION")),
                },
            },
        },
    }
}

/// Serialize the statement to the bytes the DSSE envelope signs.
/// Plain `serde_json::to_vec` — canonical-json isn't required by the
/// in-toto spec and the signature covers these exact bytes, not a
/// re-canonicalized form.
pub fn statement_bytes(stmt: &InTotoStatement) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(stmt)
}

fn lock_entry_to_resource(pkg: &crate::lock_file::LockedPackage) -> ResourceDescriptor {
    let mut digest = std::collections::BTreeMap::new();
    let (algo, hex) = split_digest(&pkg.digest);
    digest.insert(algo.to_string(), hex.to_string());
    ResourceDescriptor {
        name: pkg.name.clone(),
        digest,
        uri: Some(pkg.source.clone()),
    }
}

/// `"sha256:abc"` → `("sha256", "abc")`. `"git:abc"` → `("git", "abc")`.
/// Unprefixed → `("sha256", …)` as a safe default — SLSA subjects
/// default to sha256 anywhere we can't read the prefix.
fn split_digest(d: &str) -> (&str, &str) {
    d.split_once(':').unwrap_or(("sha256", d))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_statement_with_expected_shape() {
        let stmt = build_publish_attestation(
            "ghcr.io/acme/app",
            "sha256:deadbeef",
            "oci://ghcr.io/acme/app",
            "1.2.3",
            None,
        );
        assert_eq!(stmt.ty, "https://in-toto.io/Statement/v1");
        assert_eq!(stmt.predicate_type, "https://slsa.dev/provenance/v1");
        assert_eq!(stmt.subject.len(), 1);
        assert_eq!(stmt.subject[0].name, "ghcr.io/acme/app");
        assert_eq!(
            stmt.subject[0].digest.get("sha256").map(String::as_str),
            Some("deadbeef")
        );
        assert_eq!(stmt.predicate.build_definition.build_type, AKUA_BUILD_TYPE);
        assert_eq!(stmt.predicate.build_definition.external_parameters.tag, "1.2.3");
        assert!(stmt.predicate.run_details.builder.id.starts_with("https://akua.dev/cli/v"));
    }

    #[test]
    fn materials_pulled_from_lock() {
        use crate::lock_file::LockedPackage;

        let mut lock = AkuaLock::empty();
        lock.packages.push(LockedPackage {
            name: "nginx".to_string(),
            version: "local".to_string(),
            source: "path+file://./vendor/nginx".to_string(),
            digest: "sha256:abc123".to_string(),
            signature: None,
            dependencies: vec![],
            attestation: None,
            replaced: None,
            yanked: None,
            kyverno_source_digest: None,
            converter_version: None,
        });

        let stmt = build_publish_attestation(
            "ghcr.io/acme/app",
            "sha256:deadbeef",
            "oci://ghcr.io/acme/app",
            "1.0.0",
            Some(&lock),
        );
        let deps = &stmt.predicate.build_definition.resolved_dependencies;
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "nginx");
        assert_eq!(deps[0].digest.get("sha256").unwrap(), "abc123");
        assert_eq!(
            deps[0].uri.as_deref(),
            Some("path+file://./vendor/nginx")
        );
    }

    #[test]
    fn split_digest_handles_both_schemes() {
        assert_eq!(split_digest("sha256:abc"), ("sha256", "abc"));
        assert_eq!(split_digest("git:abc"), ("git", "abc"));
        assert_eq!(split_digest("bare"), ("sha256", "bare"));
    }

    #[test]
    fn statement_serializes_to_json() {
        let stmt = build_publish_attestation(
            "ghcr.io/acme/app",
            "sha256:deadbeef",
            "oci://ghcr.io/acme/app",
            "1.0.0",
            None,
        );
        let bytes = statement_bytes(&stmt).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        // Load-bearing keys + values consumers + cosign tooling
        // expect — pin them so a reshuffle breaks here, not on a
        // customer's verify.
        assert!(s.contains("\"_type\":\"https://in-toto.io/Statement/v1\""));
        assert!(s.contains("\"predicateType\":\"https://slsa.dev/provenance/v1\""));
        assert!(s.contains("\"buildType\":\"https://akua.dev/slsa/publish/v1\""));
        assert!(s.contains("\"subject\":["));
    }
}
