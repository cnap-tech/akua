//! On-disk sidecar format for offline cosign signatures.
//!
//! Produced by `akua sign --tarball ...`: carries everything needed
//! for `akua push --sig <sidecar>` to upload a `.sig` alongside the
//! tarball without re-signing. Enables the air-gap flow:
//!
//! 1. `akua pack`   → `pkg.tar.gz`
//! 2. `akua sign`   → `pkg.tar.gz.akuasig` (this format)
//! 3. transfer across the trust boundary
//! 4. `akua push --sig pkg.tar.gz.akuasig`
//!
//! The shape is JSON (stable across versions — additive fields land
//! via `#[serde(default)]`, never rename). `.akuasig` is akua-specific
//! to avoid confusion with cosign's on-disk `.sig` convention.
//!
//! `manifest_digest` pins the registry-side sha256 the signature is
//! bound to. The push verb re-computes the manifest digest from the
//! tarball at upload time and rejects the sidecar if they diverge —
//! catches "wrong sidecar for this tarball" and version drift between
//! sign and push hosts (see [`crate::oci_pusher::compute_publish_digests`]
//! note on version coupling).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignSidecar {
    /// Target repository — `oci://<registry>/<repo>`. Frozen into
    /// the signed payload, so a sidecar is not portable across refs.
    pub oci_ref: String,
    pub tag: String,
    /// The manifest digest the signature is bound to
    /// (`sha256:<hex>`). Matched against `compute_publish_digests`
    /// output at push time.
    pub manifest_digest: String,
    /// The raw simple-signing payload JSON (as signed). Kept
    /// verbatim so verifiers reproduce the signed bytes byte-exact.
    pub simple_signing_payload: String,
    /// Base64 (STANDARD) encoding of the ECDSA signature bytes.
    pub signature_b64: String,
    /// Pinning info so operators can triage version mismatches.
    pub akua_version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("i/o at `{}`: {source}", path.display())]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed sidecar at `{}`: {source}", path.display())]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("serializing sidecar: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl SignSidecar {
    /// Write the sidecar as pretty JSON so operators can `cat` it.
    /// Over-the-wire bytes don't matter (unlike the signed payload
    /// itself, which is preserved verbatim inside the sidecar).
    pub fn write_to(&self, path: &Path) -> Result<(), SidecarError> {
        let body = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, body).map_err(|source| SidecarError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn read_from(path: &Path) -> Result<Self, SidecarError> {
        let body = std::fs::read(path).map_err(|source| SidecarError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_slice(&body).map_err(|source| SidecarError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SignSidecar {
        SignSidecar {
            oci_ref: "oci://ghcr.io/acme/pkg".into(),
            tag: "0.1.0".into(),
            manifest_digest: "sha256:abc123".into(),
            simple_signing_payload: r#"{"critical":{"identity":{"docker-reference":"ghcr.io/acme/pkg"},"image":{"docker-manifest-digest":"sha256:abc123"},"type":"cosign container image signature"}}"#
                .into(),
            signature_b64: "MEUCIQD…=".into(),
            akua_version: "0.1.0".into(),
        }
    }

    #[test]
    fn write_then_read_round_trips_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.tar.gz.akuasig");
        let s = sample();
        s.write_to(&path).unwrap();

        let got = SignSidecar::read_from(&path).unwrap();
        assert_eq!(got, s);
    }

    #[test]
    fn read_from_missing_file_errors_with_io_not_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.akuasig");
        let err = SignSidecar::read_from(&missing).unwrap_err();
        assert!(matches!(err, SidecarError::Io { .. }));
    }

    #[test]
    fn read_from_malformed_json_errors_with_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.akuasig");
        std::fs::write(&path, b"not json").unwrap();
        let err = SignSidecar::read_from(&path).unwrap_err();
        assert!(matches!(err, SidecarError::Parse { .. }));
    }

    #[test]
    fn serialized_form_is_pretty_printed_multiline_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.akuasig");
        sample().write_to(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains('\n'),
            "expected pretty JSON (newlines): {body}"
        );
        assert!(body.contains("\"manifest_digest\""), "{body}");
    }
}
