//! Push an akua Package to an OCI registry.
//!
//! Phase 7 — the inverse of [`crate::oci_fetcher`]. Where the fetcher
//! pulls a manifest + blob and unpacks, the pusher takes a tarball,
//! uploads it as a blob, and publishes a manifest at a tag.
//!
//! Monolithic upload path (the simplest legal flow per the
//! distribution spec):
//!
//! 1. `POST /v2/<repo>/blobs/uploads/` → 202 Accepted with
//!    `Location: /v2/<repo>/blobs/uploads/<uuid>` indicating where
//!    to PUT the bytes.
//! 2. `PUT <location>?digest=sha256:<hex>` with the blob body. 201
//!    Created on success.
//! 3. Repeat for the config blob (a tiny JSON stub today).
//! 4. `PUT /v2/<repo>/manifests/<tag>` with the manifest JSON + the
//!    correct `Content-Type`. 201 Created.
//!
//! All four operations go through the shared [`crate::oci_transport`]
//! auth flow — a single `TokenCache` gets reused across steps so the
//! bearer-challenge dance runs at most once.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::oci_auth::{self, CredsStore};
use crate::oci_transport::{
    apply_bearer, build_client, fetch_token, parse_ref, BearerChallenge, OciRef, TokenCache,
    TransportError,
};

/// Media types akua uses for its own published artifacts. Distinct
/// from helm + docker media types so consumers can reject unrelated
/// artifact classes on pull.
pub const AKUA_PACKAGE_LAYER_MEDIA_TYPE: &str =
    "application/vnd.akua.package.content.v1.tar+gzip";
pub const AKUA_PACKAGE_CONFIG_MEDIA_TYPE: &str =
    "application/vnd.akua.package.config.v1+json";

/// OCI image manifest media type — what `PUT /v2/.../manifests/...`
/// sets as its Content-Type + what consumers `Accept:` on pull.
pub const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Result of a successful push. The manifest digest is what cosign
/// signs and what consumers pin in their `akua.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedArtifact {
    /// `oci://<registry>/<repo>:<tag>` that the artifact was published
    /// under. Identical to the input — surfaced for the CLI output.
    pub oci_ref: String,
    pub tag: String,
    /// Resolved manifest digest (`sha256:<hex>` of the manifest JSON
    /// bytes as we PUT them).
    pub manifest_digest: String,
    /// Content-address of the single data layer.
    pub layer_digest: String,
    pub layer_size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OciPushError {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error("registry didn't return a Location header on blob upload start")]
    MissingUploadLocation,

    #[error("registry returned {status} on blob upload: {body}")]
    UploadFailed { status: u16, body: String },

    #[error("manifest serialization: {0}")]
    ManifestEncode(#[from] serde_json::Error),
}

/// Push the tarball `layer_bytes` as the single data layer of an
/// akua Package artifact tagged `tag` under `oci_ref`. Returns the
/// resolved digests the caller records in `akua.lock` / surfaces to
/// the user.
pub fn push(
    oci_ref: &str,
    tag: &str,
    layer_bytes: &[u8],
    creds: &CredsStore,
) -> Result<PushedArtifact, OciPushError> {
    let parsed = parse_ref(oci_ref).map_err(OciPushError::from)?;
    let client = build_client().map_err(OciPushError::from)?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    // 1. Layer upload. sha256 the bytes up front so the URL-query
    //    digest matches what we PUT.
    let layer_digest = format!("sha256:{}", hex_encode(&Sha256::digest(layer_bytes)));
    let layer_size = layer_bytes.len() as u64;
    upload_blob(
        &client,
        &parsed,
        layer_bytes,
        &layer_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    // 2. Config upload. The config blob is a minimal JSON stub today;
    //    future slices may embed akua.toml + akua.lock metadata.
    let config_bytes =
        serde_json::to_vec(&MinimalConfig { akua_version: env!("CARGO_PKG_VERSION").to_string() })?;
    let config_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&config_bytes)));
    upload_blob(
        &client,
        &parsed,
        &config_bytes,
        &config_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    // 3. Manifest put. Compute its digest from our own bytes — that's
    //    what we serialize, so it's what `sha256:` the registry will
    //    advertise regardless of header ordering on the wire.
    let manifest = OciManifest {
        schema_version: 2,
        media_type: OCI_MANIFEST_MEDIA_TYPE.to_string(),
        config: Descriptor {
            media_type: AKUA_PACKAGE_CONFIG_MEDIA_TYPE.to_string(),
            size: config_bytes.len() as u64,
            digest: config_digest,
        },
        layers: vec![Descriptor {
            media_type: AKUA_PACKAGE_LAYER_MEDIA_TYPE.to_string(),
            size: layer_size,
            digest: layer_digest.clone(),
        }],
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&manifest_bytes)));

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, tag
    );
    send_with_auth(
        &client,
        &manifest_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
        |req| {
            req.header("Content-Type", OCI_MANIFEST_MEDIA_TYPE)
                .body(manifest_bytes.clone())
        },
        HttpMethod::Put,
    )?;

    Ok(PushedArtifact {
        oci_ref: oci_ref.to_string(),
        tag: tag.to_string(),
        manifest_digest,
        layer_digest: manifest.layers[0].digest.clone(),
        layer_size,
    })
}

// --- Blob upload ----------------------------------------------------------

/// Monolithic blob upload: start with `POST` to get a location, then
/// `PUT <location>?digest=<sha256:hex>` with the body.
fn upload_blob(
    client: &reqwest::blocking::Client,
    parsed: &OciRef,
    bytes: &[u8],
    digest: &str,
    creds: Option<&oci_auth::Credentials>,
    token: &mut TokenCache,
) -> Result<(), OciPushError> {
    let start_url = format!(
        "https://{}/v2/{}/blobs/uploads/",
        parsed.registry, parsed.repository
    );
    let resp = send_with_auth_raw(
        client,
        &start_url,
        &parsed.registry,
        creds,
        token,
        |req| req.header("Content-Length", "0"),
        HttpMethod::Post,
    )?;
    let location = resp
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .ok_or(OciPushError::MissingUploadLocation)?
        .to_string();

    // Location may be absolute or path-relative.
    let put_url = if location.starts_with("http://") || location.starts_with("https://") {
        location
    } else {
        format!("https://{}{}", parsed.registry, location)
    };
    // Append the digest query param. Some registries expect `?digest=`;
    // if Location already has query params, switch to `&`.
    let sep = if put_url.contains('?') { '&' } else { '?' };
    let put_url = format!("{put_url}{sep}digest={digest}");

    let put_resp = send_with_auth_raw(
        client,
        &put_url,
        &parsed.registry,
        creds,
        token,
        |req| {
            req.header("Content-Type", "application/octet-stream")
                .body(bytes.to_vec())
        },
        HttpMethod::Put,
    )?;
    let status = put_resp.status().as_u16();
    if !(status == 201 || status == 202) {
        let body = put_resp.text().unwrap_or_default();
        return Err(OciPushError::UploadFailed { status, body });
    }
    Ok(())
}

// --- Auth flow for non-GET methods ----------------------------------------

#[derive(Copy, Clone)]
enum HttpMethod {
    Post,
    Put,
}

/// Like the fetcher's `get_with_auth`, but dispatches POST or PUT and
/// returns the full `Response` (push paths care about `Location:` +
/// status code, not just body bytes).
fn send_with_auth_raw(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&oci_auth::Credentials>,
    token_cache: &mut TokenCache,
    decorate: impl Fn(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
    method: HttpMethod,
) -> Result<reqwest::blocking::Response, OciPushError> {
    // Attempt #1 — reuse any cached token / raw PAT.
    let initial = decorate(new_request(client, method, url));
    let initial = apply_bearer(initial, token_cache, creds);
    let resp = initial.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;
    if resp.status().as_u16() != 401 {
        return Ok(resp);
    }

    // 401 → bearer-challenge round-trip + retry.
    let challenge = BearerChallenge::from_resp(&resp).ok_or(TransportError::AuthRequired {
        registry: registry.to_string(),
    })?;
    let token = fetch_token(client, &challenge, creds)?;
    token_cache.token = Some(token.clone());

    let retry_req = decorate(new_request(client, method, url)).bearer_auth(&token);
    let retry = retry_req.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;
    if retry.status().as_u16() == 401 {
        return Err(TransportError::AuthRequired {
            registry: registry.to_string(),
        }
        .into());
    }
    Ok(retry)
}

fn new_request(
    client: &reqwest::blocking::Client,
    method: HttpMethod,
    url: &str,
) -> reqwest::blocking::RequestBuilder {
    match method {
        HttpMethod::Post => client.post(url),
        HttpMethod::Put => client.put(url),
    }
}

/// Body-discarding wrapper for calls where the caller just needs
/// 2xx-or-error semantics.
fn send_with_auth(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&oci_auth::Credentials>,
    token_cache: &mut TokenCache,
    decorate: impl Fn(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
    method: HttpMethod,
) -> Result<(), OciPushError> {
    let resp = send_with_auth_raw(client, url, registry, creds, token_cache, decorate, method)?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let body = resp.text().unwrap_or_default();
        return Err(OciPushError::UploadFailed { status, body });
    }
    Ok(())
}

// --- Manifest shape -------------------------------------------------------

#[derive(Debug, Serialize)]
struct OciManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: String,
    config: Descriptor,
    layers: Vec<Descriptor>,
}

#[derive(Debug, Serialize)]
struct Descriptor {
    #[serde(rename = "mediaType")]
    media_type: String,
    size: u64,
    digest: String,
}

#[derive(Debug, Serialize)]
struct MinimalConfig {
    #[serde(rename = "akuaVersion")]
    akua_version: String,
}

// ---------------------------------------------------------------------------
// Tests — manifest shape + local error paths. End-to-end push tests
// need a registry and live in the integration-test crates when added.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_serialization_shape() {
        let m = OciManifest {
            schema_version: 2,
            media_type: OCI_MANIFEST_MEDIA_TYPE.to_string(),
            config: Descriptor {
                media_type: AKUA_PACKAGE_CONFIG_MEDIA_TYPE.to_string(),
                size: 42,
                digest: "sha256:abc".to_string(),
            },
            layers: vec![Descriptor {
                media_type: AKUA_PACKAGE_LAYER_MEDIA_TYPE.to_string(),
                size: 1024,
                digest: "sha256:def".to_string(),
            }],
        };
        let s = serde_json::to_string(&m).unwrap();
        // Load-bearing keys agents + external tooling expect — lock
        // in the on-the-wire spelling.
        assert!(s.contains("\"schemaVersion\":2"));
        assert!(s.contains("\"mediaType\":\"application/vnd.oci.image.manifest.v1+json\""));
        assert!(s.contains("\"mediaType\":\"application/vnd.akua.package.content.v1.tar+gzip\""));
        assert!(s.contains("\"mediaType\":\"application/vnd.akua.package.config.v1+json\""));
        assert!(s.contains("\"digest\":\"sha256:abc\""));
        assert!(s.contains("\"digest\":\"sha256:def\""));
    }

    #[test]
    fn push_rejects_bad_ref() {
        let store = CredsStore::empty();
        let err = push("not-a-ref", "1.0.0", b"", &store).unwrap_err();
        assert!(matches!(err, OciPushError::Transport(TransportError::BadRef(_))));
    }
}
