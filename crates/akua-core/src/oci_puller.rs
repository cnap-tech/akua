//! Pull an akua Package from an OCI registry.
//!
//! Phase 7 inverse of [`crate::oci_pusher`]. Fetches the manifest at
//! `oci://<registry>/<repo>:<tag>`, selects the single akua-typed
//! layer, pulls its blob bytes, and hands them back. Caller (the
//! `akua pull` verb) unpacks to the target workspace via
//! [`crate::package_tar::unpack_to`].
//!
//! This module deliberately doesn't cache-extract like
//! [`crate::oci_fetcher`]: `akua pull` writes into a user-named
//! directory, not a content-addressed cache. Callers can re-run
//! `pull` if they lost the extracted tree.

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::oci_auth::{self, CredsStore};
use crate::oci_pusher::{AKUA_PACKAGE_LAYER_MEDIA_TYPE, OCI_MANIFEST_MEDIA_TYPE};
use crate::oci_transport::{
    build_client, get_with_auth, parse_ref, TokenCache, TransportError,
};

/// Result of a successful pull. `tarball` is the raw `.tar.gz` bytes
/// — same shape `akua publish` uploaded. Callers that want the
/// unpacked tree use [`crate::package_tar::unpack_to`].
#[derive(Debug, Clone)]
pub struct PulledPackage {
    pub tarball: Vec<u8>,
    pub manifest_digest: String,
    pub layer_digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum OciPullError {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error("manifest at `{oci_ref}:{tag}` has no akua package layer (media type {AKUA_PACKAGE_LAYER_MEDIA_TYPE})")]
    NoAkuaLayer { oci_ref: String, tag: String },

    #[error("pulled blob digest `{actual}` doesn't match manifest-declared `{declared}`")]
    LayerDigestMismatch { actual: String, declared: String },

    #[error("manifest parse: {0}")]
    ManifestParse(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
struct Manifest {
    layers: Vec<Layer>,
}

#[derive(Debug, Deserialize)]
struct Layer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    // `size` is advertised in the manifest but we verify via the
    // sha256 recompute below, so we don't read it — dropping the
    // field avoids the false signal that it's checked.
}

/// Fetch the akua Package published at `oci_ref:tag`. Returns the
/// tarball bytes + resolved digests. Layer digest verification
/// against the manifest's declaration is done inline — a mismatch
/// means the registry handed us different bytes than the manifest
/// advertised (cache poisoning / proxy bug).
pub fn pull(
    oci_ref: &str,
    tag: &str,
    creds: &CredsStore,
) -> Result<PulledPackage, OciPullError> {
    let parsed = parse_ref(oci_ref)?;
    let client = build_client()?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, tag
    );
    let manifest_bytes = get_with_auth(
        &client,
        &manifest_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
        |req| req.header("Accept", OCI_MANIFEST_MEDIA_TYPE),
    )?;
    let manifest_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&manifest_bytes)));
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;

    let layer = manifest
        .layers
        .into_iter()
        .find(|l| l.media_type == AKUA_PACKAGE_LAYER_MEDIA_TYPE)
        .ok_or_else(|| OciPullError::NoAkuaLayer {
            oci_ref: oci_ref.to_string(),
            tag: tag.to_string(),
        })?;

    let blob_url = format!(
        "https://{}/v2/{}/blobs/{}",
        parsed.registry, parsed.repository, layer.digest
    );
    let blob = get_with_auth(
        &client,
        &blob_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
        |req| req,
    )?;

    let actual = format!("sha256:{}", hex_encode(&Sha256::digest(&blob)));
    if actual != layer.digest {
        return Err(OciPullError::LayerDigestMismatch {
            actual,
            declared: layer.digest,
        });
    }

    Ok(PulledPackage {
        tarball: blob,
        manifest_digest,
        layer_digest: layer.digest,
    })
}

/// Pull the cosign `.att` attestation sidecar for an artifact if
/// one exists. Returns the raw DSSE envelope bytes (the layer
/// contents). `None` on a 404 — the publisher didn't attest this
/// artifact, which is a legitimate state the caller can choose
/// policy for. Other registry errors bubble through `OciPullError`.
///
/// Verification + predicate parsing live in
/// [`crate::cosign::verify_dsse`] and [`crate::slsa`] respectively;
/// this fn is purely the transport step.
#[cfg(feature = "cosign-verify")]
pub fn pull_attestation(
    oci_ref: &str,
    manifest_digest: &str,
    creds: &CredsStore,
) -> Result<Option<Vec<u8>>, OciPullError> {
    let parsed = parse_ref(oci_ref)?;
    let client = build_client()?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    // Tag convention matches what `akua publish` + cosign write:
    // `sha256:<hex>` → `sha256-<hex>.att`.
    let hex = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(manifest_digest);
    let att_tag = format!("sha256-{hex}.att");

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, att_tag
    );
    match get_with_auth(
        &client,
        &manifest_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
        |req| req.header("Accept", crate::oci_pusher::OCI_MANIFEST_MEDIA_TYPE),
    ) {
        Ok(bytes) => {
            let manifest: AttestationSidecarManifest = serde_json::from_slice(&bytes)?;
            let layer = manifest
                .layers
                .into_iter()
                .find(|l| l.media_type == crate::cosign::DSSE_ENVELOPE_MEDIA_TYPE)
                .ok_or(OciPullError::NoAkuaLayer {
                    oci_ref: oci_ref.to_string(),
                    tag: att_tag.clone(),
                })?;
            let blob_url = format!(
                "https://{}/v2/{}/blobs/{}",
                parsed.registry, parsed.repository, layer.digest
            );
            let envelope_bytes = get_with_auth(
                &client,
                &blob_url,
                &parsed.registry,
                registry_creds.as_ref(),
                &mut token,
                |req| req,
            )?;
            Ok(Some(envelope_bytes))
        }
        Err(TransportError::Status { status: 404, .. }) => Ok(None),
        Err(e) => Err(OciPullError::Transport(e)),
    }
}

#[cfg(feature = "cosign-verify")]
#[derive(Debug, Deserialize)]
struct AttestationSidecarManifest {
    layers: Vec<Layer>,
}

// ---------------------------------------------------------------------------
// Tests — parse errors, ref errors. Live-registry pulls go in
// integration tests (or paired with a publish against a local registry).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_rejects_bad_ref() {
        let store = CredsStore::empty();
        let err = pull("not-a-ref", "1.0.0", &store).unwrap_err();
        assert!(matches!(
            err,
            OciPullError::Transport(TransportError::BadRef(_))
        ));
    }
}
