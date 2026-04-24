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

    // Pre-compute all three digests + serialized bytes locally. The
    // registry's `sha256:` is a function of these bytes, so the
    // digests we compute offline == the digests the registry will
    // advertise post-PUT. This invariant is what makes `akua sign`
    // able to produce a valid signature without a network round-trip.
    let d = compute_publish_digests(layer_bytes);

    upload_blob(
        &client,
        &parsed,
        layer_bytes,
        &d.layer_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;
    upload_blob(
        &client,
        &parsed,
        &d.config_bytes,
        &d.config_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, tag
    );
    let manifest_bytes = d.manifest_bytes.clone();
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
        manifest_digest: d.manifest_digest,
        layer_digest: d.layer_digest,
        layer_size: d.layer_size,
    })
}

/// Deterministic publish-side digests for `layer_bytes`. Pure function:
/// identical inputs → identical outputs, no network, no I/O. Used by
/// [`push`] for the actual upload, and by `akua sign` to compute the
/// manifest digest to sign without a registry round-trip.
///
/// **Version coupling.** The config blob embeds the akua binary
/// version (`env!("CARGO_PKG_VERSION")`) to let consumers reject
/// artifacts built by a version they don't understand. That means
/// `akua sign`-then-`akua push` must be run by the *same* akua
/// version — a signature produced on akua 0.1.0 is invalid for an
/// artifact pushed by akua 0.2.0 if the config shape drifted. Air-gap
/// flows should pin the binary.
#[derive(Debug, Clone)]
pub struct PublishDigests {
    pub layer_digest: String,
    pub layer_size: u64,
    pub config_digest: String,
    pub config_bytes: Vec<u8>,
    pub manifest_digest: String,
    pub manifest_bytes: Vec<u8>,
}

pub fn compute_publish_digests(layer_bytes: &[u8]) -> PublishDigests {
    let layer_digest = format!("sha256:{}", hex_encode(&Sha256::digest(layer_bytes)));
    let layer_size = layer_bytes.len() as u64;

    let config_bytes = serde_json::to_vec(&MinimalConfig {
        akua_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .expect("MinimalConfig serialization must not fail");
    let config_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&config_bytes)));

    let manifest = OciManifest {
        schema_version: 2,
        media_type: OCI_MANIFEST_MEDIA_TYPE.to_string(),
        config: Descriptor {
            media_type: AKUA_PACKAGE_CONFIG_MEDIA_TYPE.to_string(),
            size: config_bytes.len() as u64,
            digest: config_digest.clone(),
        },
        layers: vec![Descriptor {
            media_type: AKUA_PACKAGE_LAYER_MEDIA_TYPE.to_string(),
            size: layer_size,
            digest: layer_digest.clone(),
        }],
    };
    let manifest_bytes =
        serde_json::to_vec(&manifest).expect("OciManifest serialization must not fail");
    let manifest_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&manifest_bytes)));

    PublishDigests {
        layer_digest,
        layer_size,
        config_digest,
        config_bytes,
        manifest_digest,
        manifest_bytes,
    }
}

/// Push a DSSE-wrapped attestation (SLSA v1 provenance) as an `.att`
/// sidecar for an already-published artifact. Tag shape matches
/// cosign's convention: `sha256-<hex>.att`. The layer carries the
/// DSSE envelope JSON with media type
/// `application/vnd.dsse.envelope.v1+json`.
///
/// Parallel to [`push_cosign_signature`] — same two-blob + manifest
/// shape, different media type + tag suffix, no per-layer
/// annotations (the signature lives inside the DSSE payload).
#[cfg(feature = "cosign-verify")]
pub fn push_attestation(
    oci_ref: &str,
    manifest_digest: &str,
    dsse_envelope_bytes: &[u8],
    creds: &CredsStore,
) -> Result<String, OciPushError> {
    let parsed = parse_ref(oci_ref).map_err(OciPushError::from)?;
    let client = build_client().map_err(OciPushError::from)?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    // Payload (the DSSE envelope) + empty config — same two-blob
    // layout cosign uses for `.att`.
    let payload_digest = format!(
        "sha256:{}",
        hex_encode(&Sha256::digest(dsse_envelope_bytes))
    );
    upload_blob(
        &client,
        &parsed,
        dsse_envelope_bytes,
        &payload_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;
    let config_bytes: &[u8] = b"{}";
    let config_digest = format!("sha256:{}", hex_encode(&Sha256::digest(config_bytes)));
    upload_blob(
        &client,
        &parsed,
        config_bytes,
        &config_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    let manifest = OciManifest {
        schema_version: 2,
        media_type: OCI_MANIFEST_MEDIA_TYPE.to_string(),
        config: Descriptor {
            media_type: "application/vnd.oci.image.config.v1+json".to_string(),
            size: config_bytes.len() as u64,
            digest: config_digest,
        },
        layers: vec![Descriptor {
            media_type: crate::cosign::DSSE_ENVELOPE_MEDIA_TYPE.to_string(),
            size: dsse_envelope_bytes.len() as u64,
            digest: payload_digest,
        }],
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;

    let hex = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(manifest_digest);
    let att_tag = format!("sha256-{hex}.att");

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, att_tag
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

    Ok(att_tag)
}

/// Push a cosign `.sig` sidecar for an already-published artifact.
/// `payload_bytes` + `signature_b64` come from
/// [`crate::cosign::build_simple_signing_payload`] and
/// [`crate::cosign::sign_keyed`]. Signature tag is `sha256-<hex>.sig`.
pub fn push_cosign_signature(
    oci_ref: &str,
    manifest_digest: &str,
    payload_bytes: &[u8],
    signature_b64: &str,
    creds: &CredsStore,
) -> Result<String, OciPushError> {
    let parsed = parse_ref(oci_ref).map_err(OciPushError::from)?;
    let client = build_client().map_err(OciPushError::from)?;
    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();

    // Payload blob.
    let payload_digest = format!("sha256:{}", hex_encode(&Sha256::digest(payload_bytes)));
    upload_blob(
        &client,
        &parsed,
        payload_bytes,
        &payload_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    // Empty config — cosign sidecar uses the generic OCI image config
    // media type with `{}` content.
    let config_bytes: &[u8] = b"{}";
    let config_digest = format!("sha256:{}", hex_encode(&Sha256::digest(config_bytes)));
    upload_blob(
        &client,
        &parsed,
        config_bytes,
        &config_digest,
        registry_creds.as_ref(),
        &mut token,
    )?;

    // Signature manifest. The layer carries the signature in an
    // `annotations` map, distinct from the config.
    #[derive(Serialize)]
    struct SigLayer<'a> {
        #[serde(rename = "mediaType")]
        media_type: &'a str,
        size: u64,
        digest: &'a str,
        annotations: std::collections::BTreeMap<&'a str, &'a str>,
    }
    #[derive(Serialize)]
    struct SigManifest<'a> {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        #[serde(rename = "mediaType")]
        media_type: &'a str,
        config: Descriptor,
        layers: Vec<SigLayer<'a>>,
    }
    let mut annotations = std::collections::BTreeMap::new();
    annotations.insert("dev.cosignproject.cosign/signature", signature_b64);
    let manifest = SigManifest {
        schema_version: 2,
        media_type: OCI_MANIFEST_MEDIA_TYPE,
        config: Descriptor {
            media_type: "application/vnd.oci.image.config.v1+json".to_string(),
            size: config_bytes.len() as u64,
            digest: config_digest,
        },
        layers: vec![SigLayer {
            media_type: "application/vnd.dev.cosign.simplesigning.v1+json",
            size: payload_bytes.len() as u64,
            digest: &payload_digest,
            annotations,
        }],
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;

    // Tag transform: `sha256:<hex>` → `sha256-<hex>.sig`.
    let hex = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(manifest_digest);
    let sig_tag = format!("sha256-{hex}.sig");

    let sig_manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, sig_tag
    );
    send_with_auth(
        &client,
        &sig_manifest_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
        |req| {
            req.header("Content-Type", OCI_MANIFEST_MEDIA_TYPE)
                .body(manifest_bytes.clone())
        },
        HttpMethod::Put,
    )?;

    Ok(sig_tag)
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
