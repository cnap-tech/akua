//! Push an akua Package to an OCI registry.
//!
//! — the inverse of [`crate::oci_fetcher`]. Where the fetcher
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
    apply_bearer, build_client, fetch_token, parse_ref, registry_scheme, BearerChallenge, OciRef,
    TokenCache, TransportError,
};

/// Media types akua uses for its own published artifacts. Distinct
/// from helm + docker media types so consumers can reject unrelated
/// artifact classes on pull.
pub const AKUA_PACKAGE_LAYER_MEDIA_TYPE: &str = "application/vnd.akua.package.content.v1.tar+gzip";
pub const AKUA_PACKAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.akua.package.config.v1+json";

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
        "{}://{}/v2/{}/manifests/{}",
        registry_scheme(&parsed.registry),
        parsed.registry,
        parsed.repository,
        tag
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
        "{}://{}/v2/{}/manifests/{}",
        registry_scheme(&parsed.registry),
        parsed.registry,
        parsed.repository,
        att_tag
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
        "{}://{}/v2/{}/manifests/{}",
        registry_scheme(&parsed.registry),
        parsed.registry,
        parsed.repository,
        sig_tag
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
    let scheme = registry_scheme(&parsed.registry);
    let start_url = format!(
        "{}://{}/v2/{}/blobs/uploads/",
        scheme, parsed.registry, parsed.repository
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
        format!("{}://{}{}", scheme, parsed.registry, location)
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
    use httpmock::prelude::*;

    /// Construct an `oci://127.0.0.1:<port>/<repo>` ref against the
    /// running mock. `registry_scheme` recognizes loopback hosts and
    /// switches to plain http, so push/pull will hit the mock.
    fn mock_oci_ref(server: &MockServer, repo: &str) -> String {
        format!("oci://127.0.0.1:{}/{}", server.port(), repo)
    }

    /// Stand up the three endpoints a happy-path push hits:
    ///   1. POST /v2/<repo>/blobs/uploads/  → 202 + Location
    ///   2. PUT  <Location>?digest=<sha256> → 201
    ///   3. PUT  /v2/<repo>/manifests/<tag> → 201
    ///
    /// Returns `(POST, PUT-blob, PUT-manifest)` so callers can assert
    /// hit counts after `push()`.
    fn mock_happy_path<'a>(
        server: &'a MockServer,
        repo: &str,
        tag: &str,
    ) -> (httpmock::Mock<'a>, httpmock::Mock<'a>, httpmock::Mock<'a>) {
        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let manifest_path = format!("/v2/{repo}/manifests/{tag}");
        let location = "/upload-session/abc";

        let post_mock = server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202).header("Location", location);
        });
        let put_blob_mock = server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(201);
        });
        let put_manifest_mock = server.mock(|when, then| {
            when.method(PUT)
                .path(manifest_path)
                .header("content-type", OCI_MANIFEST_MEDIA_TYPE);
            then.status(201);
        });
        (post_mock, put_blob_mock, put_manifest_mock)
    }

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
        assert!(matches!(
            err,
            OciPushError::Transport(TransportError::BadRef(_))
        ));
    }

    /// Anonymous happy-path push against a mock registry. Asserts the
    /// returned digests match what `compute_publish_digests` produces
    /// offline, and that the manifest we PUT parses cleanly through
    /// oci-client (independent OCI-spec validation: not just our own
    /// serializer round-tripping).
    #[test]
    fn push_happy_path_anonymous() {
        let server = MockServer::start();
        let repo = "test/pkg";
        let tag = "1.0.0";
        let oci_ref = mock_oci_ref(&server, repo);
        let layer = b"fake tarball bytes for test only";

        // Capture every request body so we can introspect what the
        // pusher sent. httpmock's `hits()` counts; `body()` from the
        // captured request requires reading via `received_requests`.
        let (post_mock, put_blob_mock, put_manifest_mock) = mock_happy_path(&server, repo, tag);

        let store = CredsStore::empty();
        let pushed = push(&oci_ref, tag, layer, &store).expect("push must succeed");

        // Two blobs uploaded (layer + config) → POST + PUT each fire 2x.
        post_mock.assert_hits(2);
        put_blob_mock.assert_hits(2);
        put_manifest_mock.assert_hits(1);

        // Returned digests match the offline-computed ones — the
        // invariant `akua sign` relies on.
        let d = compute_publish_digests(layer);
        assert_eq!(pushed.layer_digest, d.layer_digest);
        assert_eq!(pushed.manifest_digest, d.manifest_digest);
        assert_eq!(pushed.layer_size, layer.len() as u64);
        assert_eq!(pushed.tag, tag);
        assert_eq!(pushed.oci_ref, oci_ref);

        // Independent spec validation: our manifest bytes parse as a
        // valid OciImageManifest per oci-client's parser.
        let parsed: oci_client::manifest::OciImageManifest =
            serde_json::from_slice(&d.manifest_bytes).expect("manifest bytes must be OCI-valid");
        assert_eq!(parsed.schema_version, 2);
        assert_eq!(parsed.media_type.as_deref(), Some(OCI_MANIFEST_MEDIA_TYPE));
        assert_eq!(parsed.config.media_type, AKUA_PACKAGE_CONFIG_MEDIA_TYPE);
        assert_eq!(parsed.layers.len(), 1);
        assert_eq!(parsed.layers[0].media_type, AKUA_PACKAGE_LAYER_MEDIA_TYPE);
        assert_eq!(parsed.layers[0].digest, d.layer_digest);
        assert_eq!(parsed.layers[0].size as u64, layer.len() as u64);
    }

    /// Registry returns 401 with a Bearer challenge → pusher fetches a
    /// token from the realm and retries the upload. Same end-state as
    /// happy-path; this exercises the auth branch in `send_with_auth_raw`.
    #[test]
    fn push_walks_bearer_challenge() {
        let server = MockServer::start();
        let repo = "private/pkg";
        let tag = "2.0.0";
        let oci_ref = mock_oci_ref(&server, repo);
        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let manifest_path = format!("/v2/{repo}/manifests/{tag}");
        let realm = format!("http://127.0.0.1:{}/token", server.port());
        let location = "/upload-session/auth";

        // First POST → 401 challenge. We use a header-match guard
        // (no Authorization) so the same path can serve both attempts:
        // unauthenticated → 401, authenticated → 202.
        let challenge = server.mock(|when, then| {
            when.method(POST).path(upload_path.clone()).matches(|req| {
                !req.headers
                    .as_ref()
                    .map(|h| {
                        h.iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                    })
                    .unwrap_or(false)
            });
            then.status(401).header(
                "WWW-Authenticate",
                format!(r#"Bearer realm="{realm}",service="acme",scope="repository:{repo}:push""#),
            );
        });
        // Token endpoint: returns the bearer the pusher will reuse.
        let token_mock = server.mock(|when, then| {
            when.method(GET).path("/token");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"token":"deadbeef"}"#);
        });
        // Authenticated POST → 202 (only matches when Authorization present).
        let post_authed = server.mock(|when, then| {
            when.method(POST)
                .path(upload_path)
                .header("authorization", "Bearer deadbeef");
            then.status(202).header("Location", location);
        });
        let put_blob = server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(201);
        });
        let put_manifest = server.mock(|when, then| {
            when.method(PUT).path(manifest_path);
            then.status(201);
        });

        let store = CredsStore::empty();
        push(&oci_ref, tag, b"x", &store).expect("auth-retry push must succeed");

        // Challenge fires once (the *first* POST), token exchanged once,
        // remaining traffic uses the cached bearer so subsequent POST/PUT
        // never re-challenge.
        challenge.assert_hits(1);
        token_mock.assert_hits(1);
        post_authed.assert_hits(2);
        put_blob.assert_hits(2);
        put_manifest.assert_hits(1);
    }

    /// Registry's POST /blobs/uploads/ returns 202 but with no
    /// Location header → MissingUploadLocation. This is a registry
    /// bug we surface as a typed error rather than hanging.
    #[test]
    fn push_missing_upload_location() {
        let server = MockServer::start();
        let repo = "broken/pkg";
        let oci_ref = mock_oci_ref(&server, repo);
        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202); // no Location
        });
        let err = push(&oci_ref, "1.0.0", b"x", &CredsStore::empty()).unwrap_err();
        assert!(matches!(err, OciPushError::MissingUploadLocation));
    }

    /// Registry rejects the blob PUT with a 5xx → UploadFailed
    /// surfaces the status code + body. Captures the most common
    /// real-world failure: registry returns 503 under load.
    #[test]
    fn push_blob_put_failed() {
        let server = MockServer::start();
        let repo = "flaky/pkg";
        let oci_ref = mock_oci_ref(&server, repo);
        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let location = "/upload-session/x";
        server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202).header("Location", location);
        });
        server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(503).body("backend unavailable");
        });
        let err = push(&oci_ref, "1.0.0", b"x", &CredsStore::empty()).unwrap_err();
        match err {
            OciPushError::UploadFailed { status, body } => {
                assert_eq!(status, 503);
                assert!(body.contains("backend unavailable"));
            }
            other => panic!("expected UploadFailed, got {other:?}"),
        }
    }

    /// `push_cosign_signature` writes a sidecar manifest with the
    /// signature in a layer annotation. Verify it round-trips through
    /// the mock + the returned tag uses cosign's `sha256-<hex>.sig`
    /// shape.
    #[test]
    fn push_cosign_signature_happy_path() {
        let server = MockServer::start();
        let repo = "signed/pkg";
        let oci_ref = mock_oci_ref(&server, repo);
        let manifest_digest =
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd";
        let expected_sig_tag =
            "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd.sig";

        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let sig_manifest_path = format!("/v2/{repo}/manifests/{expected_sig_tag}");
        let location = "/upload-session/sig";

        server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202).header("Location", location);
        });
        server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(201);
        });
        let manifest_put = server.mock(|when, then| {
            when.method(PUT).path(sig_manifest_path);
            then.status(201);
        });

        let returned_tag = push_cosign_signature(
            &oci_ref,
            manifest_digest,
            b"payload bytes",
            "fakesigb64==",
            &CredsStore::empty(),
        )
        .expect("cosign sig push must succeed");

        assert_eq!(returned_tag, expected_sig_tag);
        manifest_put.assert();
    }

    /// `push_attestation` pushes a DSSE envelope sidecar tagged
    /// `sha256-<hex>.att` (cosign's attestation tag convention).
    #[cfg(feature = "cosign-verify")]
    #[test]
    fn push_attestation_happy_path() {
        let server = MockServer::start();
        let repo = "attested/pkg";
        let oci_ref = mock_oci_ref(&server, repo);
        let manifest_digest =
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567";
        let expected_att_tag =
            "sha256-abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567.att";

        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let manifest_path = format!("/v2/{repo}/manifests/{expected_att_tag}");
        let location = "/upload-session/att";

        server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202).header("Location", location);
        });
        server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(201);
        });
        let manifest_put = server.mock(|when, then| {
            when.method(PUT).path(manifest_path);
            then.status(201);
        });

        let returned_tag = push_attestation(
            &oci_ref,
            manifest_digest,
            b"dsse envelope",
            &CredsStore::empty(),
        )
        .expect("attestation push must succeed");

        assert_eq!(returned_tag, expected_att_tag);
        manifest_put.assert();
    }
}
