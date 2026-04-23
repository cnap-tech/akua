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
    apply_bearer, build_client, ensure_ok, fetch_token, parse_ref, BearerChallenge, OciRef,
    TokenCache, TransportError,
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
    #[serde(default)]
    size: u64,
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

    let _ = layer.size; // available on the Layer struct; unused.
    Ok(PulledPackage {
        tarball: blob,
        manifest_digest,
        layer_digest: layer.digest,
    })
}

/// GET with bearer-challenge auth retry. Local copy of the fetcher's
/// helper because threading `decorate` + the shared code back into
/// `oci_fetcher` would mean exporting another pub(crate) symbol; a
/// future simplify pass can unify.
fn get_with_auth(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&oci_auth::Credentials>,
    token_cache: &mut TokenCache,
    decorate: impl Fn(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
) -> Result<Vec<u8>, OciPullError> {
    let req = apply_bearer(decorate(client.get(url)), token_cache, creds);
    let resp = req.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;

    if resp.status().as_u16() != 401 {
        return Ok(ensure_ok(resp, url)?);
    }

    let challenge = BearerChallenge::from_resp(&resp).ok_or_else(|| TransportError::AuthRequired {
        registry: registry.to_string(),
    })?;
    let token = fetch_token(client, &challenge, creds)?;
    token_cache.token = Some(token.clone());

    let retry_req = decorate(client.get(url)).bearer_auth(&token);
    let retry = retry_req.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;
    Ok(ensure_ok(retry, url)?)
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
