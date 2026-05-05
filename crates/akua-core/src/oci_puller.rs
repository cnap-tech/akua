//! Pull an akua Package from an OCI registry.
//!
//! inverse of [`crate::oci_pusher`]. Fetches the manifest at
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
    build_client, get_with_auth, parse_ref, registry_scheme, TokenCache, TransportError,
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
pub fn pull(oci_ref: &str, tag: &str, creds: &CredsStore) -> Result<PulledPackage, OciPullError> {
    let parsed = parse_ref(oci_ref)?;
    let client = build_client()?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    let scheme = registry_scheme(&parsed.registry);
    let manifest_url = format!(
        "{}://{}/v2/{}/manifests/{}",
        scheme, parsed.registry, parsed.repository, tag
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
        "{}://{}/v2/{}/blobs/{}",
        scheme, parsed.registry, parsed.repository, layer.digest
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

    let scheme = registry_scheme(&parsed.registry);
    let manifest_url = format!(
        "{}://{}/v2/{}/manifests/{}",
        scheme, parsed.registry, parsed.repository, att_tag
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
            let manifest: Manifest = serde_json::from_slice(&bytes)?;
            let layer = manifest
                .layers
                .into_iter()
                .find(|l| l.media_type == crate::cosign::DSSE_ENVELOPE_MEDIA_TYPE)
                .ok_or(OciPullError::NoAkuaLayer {
                    oci_ref: oci_ref.to_string(),
                    tag: att_tag.clone(),
                })?;
            let blob_url = format!(
                "{}://{}/v2/{}/blobs/{}",
                scheme, parsed.registry, parsed.repository, layer.digest
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

// ---------------------------------------------------------------------------
// Tests — parse errors, ref errors. Live-registry pulls go in
// integration tests (or paired with a publish against a local registry).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci_pusher::compute_publish_digests;
    use httpmock::prelude::*;

    fn mock_oci_ref(server: &MockServer, repo: &str) -> String {
        format!("oci://127.0.0.1:{}/{}", server.port(), repo)
    }

    #[test]
    fn pull_rejects_bad_ref() {
        let store = CredsStore::empty();
        let err = pull("not-a-ref", "1.0.0", &store).unwrap_err();
        assert!(matches!(
            err,
            OciPullError::Transport(TransportError::BadRef(_))
        ));
    }

    /// Anonymous happy-path pull: registry serves the manifest + the
    /// declared blob. Asserts the puller surfaces both digests
    /// matching what the publisher would have computed.
    #[test]
    fn pull_happy_path_anonymous() {
        let server = MockServer::start();
        let repo = "test/pkg";
        let tag = "1.0.0";
        let layer = b"hello world tarball" as &[u8];
        let d = compute_publish_digests(layer);
        let manifest_bytes = d.manifest_bytes.clone();

        server.mock(|when, then| {
            when.method(GET).path(format!("/v2/{repo}/manifests/{tag}"));
            then.status(200)
                .header("content-type", OCI_MANIFEST_MEDIA_TYPE)
                .body(manifest_bytes);
        });
        server.mock(|when, then| {
            when.method(GET)
                .path(format!("/v2/{repo}/blobs/{}", d.layer_digest));
            then.status(200).body(layer);
        });

        let pulled = pull(&mock_oci_ref(&server, repo), tag, &CredsStore::empty())
            .expect("pull must succeed");
        assert_eq!(pulled.tarball, layer);
        assert_eq!(pulled.layer_digest, d.layer_digest);
        assert_eq!(pulled.manifest_digest, d.manifest_digest);
    }

    /// Manifest's layer list contains no akua-package layer (e.g. a
    /// helm chart artifact at the same ref) → typed NoAkuaLayer error.
    /// Protects against agents accidentally pulling the wrong artifact
    /// class.
    #[test]
    fn pull_rejects_non_akua_manifest() {
        let server = MockServer::start();
        let repo = "test/wrong-class";
        let tag = "1.0.0";
        // Manifest with one layer of a different media type.
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST_MEDIA_TYPE,
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "size": 2,
                "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            },
            "layers": [{
                "mediaType": "application/vnd.cncf.helm.chart.content.v1.tar+gzip",
                "size": 1,
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            }],
        });
        server.mock(|when, then| {
            when.method(GET).path(format!("/v2/{repo}/manifests/{tag}"));
            then.status(200).body(manifest.to_string());
        });

        let err = pull(&mock_oci_ref(&server, repo), tag, &CredsStore::empty()).unwrap_err();
        assert!(matches!(err, OciPullError::NoAkuaLayer { .. }));
    }

    /// Registry hands us a blob whose hash doesn't match the manifest's
    /// declared `layer.digest` → LayerDigestMismatch. This is the
    /// cache-poisoning / proxy-tamper detection invariant.
    #[test]
    fn pull_detects_layer_digest_mismatch() {
        let server = MockServer::start();
        let repo = "test/tampered";
        let tag = "1.0.0";
        let advertised_layer = b"correct bytes" as &[u8];
        let d = compute_publish_digests(advertised_layer);

        server.mock(|when, then| {
            when.method(GET).path(format!("/v2/{repo}/manifests/{tag}"));
            then.status(200).body(d.manifest_bytes.clone());
        });
        // Same blob endpoint, but registry returns *different* bytes
        // than the manifest declared.
        server.mock(|when, then| {
            when.method(GET)
                .path(format!("/v2/{repo}/blobs/{}", d.layer_digest));
            then.status(200).body(b"tampered bytes" as &[u8]);
        });

        let err = pull(&mock_oci_ref(&server, repo), tag, &CredsStore::empty()).unwrap_err();
        match err {
            OciPullError::LayerDigestMismatch { actual, declared } => {
                assert_ne!(actual, declared);
                assert_eq!(declared, d.layer_digest);
            }
            other => panic!("expected LayerDigestMismatch, got {other:?}"),
        }
    }

    /// Registry returns body that isn't valid JSON → ManifestParse.
    /// (Real-world: HTML 502 page from a load-balancer.)
    #[test]
    fn pull_handles_malformed_manifest() {
        let server = MockServer::start();
        let repo = "test/garbled";
        let tag = "1.0.0";
        server.mock(|when, then| {
            when.method(GET).path(format!("/v2/{repo}/manifests/{tag}"));
            then.status(200).body("not json at all <html>");
        });
        let err = pull(&mock_oci_ref(&server, repo), tag, &CredsStore::empty()).unwrap_err();
        assert!(matches!(err, OciPullError::ManifestParse(_)));
    }

    /// `pull_attestation`: 404 from the att-tag endpoint must surface
    /// as `Ok(None)` — a publisher that didn't sign attestations is
    /// a legitimate state for the caller to apply policy against.
    #[cfg(feature = "cosign-verify")]
    #[test]
    fn pull_attestation_returns_none_on_404() {
        let server = MockServer::start();
        let repo = "test/no-att";
        let manifest_digest = "sha256:abcd";
        let att_tag = "sha256-abcd.att";
        server.mock(|when, then| {
            when.method(GET)
                .path(format!("/v2/{repo}/manifests/{att_tag}"));
            then.status(404).body("not found");
        });
        let result = pull_attestation(
            &mock_oci_ref(&server, repo),
            manifest_digest,
            &CredsStore::empty(),
        )
        .expect("404 must be Ok(None), not Err");
        assert!(result.is_none());
    }
}
