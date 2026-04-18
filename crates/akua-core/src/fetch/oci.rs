//! OCI Distribution chart pull.
//!
//! All the OCI-specific glue in one place:
//! - [`OciAuth`] / [`RegistryCredentials`] — user-supplied credentials
//!   (redacted `Debug` impls so `?auth` tracing can't leak them).
//! - [`OciRef`] — parsed `oci://host/path:tag`.
//! - manifest fetch + layer selection (strict media-type enforcement).
//! - blob stream to tempfile with on-the-fly sha256 (bypasses
//!   `oci-client::pull` which buffers the layer in memory).
//! - anonymous bearer-token exchange (the ghcr.io public-pull dance).

use std::path::Path;

use super::options::{limit_exceeded, max_download_bytes, LimitKind};
use super::ssrf_client::{redact_userinfo, ssrf_safe_client};
use super::streaming::stream_response_to_file;
use super::FetchError;
use crate::umbrella::Dependency;

/// Canonical media type for a Helm chart layer inside an OCI artifact.
/// We reject manifests that don't advertise this exact type — a
/// fallback to `layers[0]` would let a malicious registry substitute
/// arbitrary bytes at the layer slot.
pub const HELM_LAYER_MEDIA_TYPE: &str =
    "application/vnd.cncf.helm.chart.content.v1.tar+gzip";

/// Registry credentials applied to OCI pulls. Each entry is keyed by
/// registry host (e.g. `"ghcr.io"`, `"registry.cnap.internal"`).
/// Anonymous is the fallback when a repository's host isn't in the map.
#[derive(Clone, Default)]
pub struct OciAuth {
    pub creds: std::collections::HashMap<String, RegistryCredentials>,
}

/// Redacted on purpose — we never want `{:?}` to reveal which hosts
/// have creds (could leak namespace structure) or the shape of the
/// secrets. Shows only the host count.
impl std::fmt::Debug for OciAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OciAuth {{ creds: <{} hosts redacted> }}", self.creds.len())
    }
}

#[derive(Clone)]
pub enum RegistryCredentials {
    Basic { username: String, password: String },
    Bearer(String),
}

/// Redacted `Debug` so `tracing::debug!(?creds, …)` doesn't leak
/// passwords or tokens into logs. Usernames are kept (they're rarely
/// sensitive) so operators can still tell which account is active.
impl std::fmt::Debug for RegistryCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { username, .. } => {
                write!(f, "Basic {{ username: {username:?}, password: <redacted> }}")
            }
            Self::Bearer(_) => write!(f, "Bearer(<redacted>)"),
        }
    }
}

/// Parsed `oci://host/repo:tag` for direct URL construction.
pub struct OciRef {
    pub host: String,
    pub repository: String,
    pub tag: String,
}

impl OciRef {
    pub fn parse(repo: &str, name: &str, version: &str) -> Result<Self, FetchError> {
        let without_scheme = repo
            .strip_prefix("oci://")
            .ok_or_else(|| FetchError::UnsupportedRepo(redact_userinfo(repo)))?;
        let trimmed = without_scheme.trim_end_matches('/');
        let (host, path) = trimmed.split_once('/').ok_or_else(|| {
            FetchError::InvalidOciRef(format!(
                "no repository path in {}",
                redact_userinfo(repo)
            ))
        })?;
        validate_oci_host(host)?;
        let repository = if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}/{name}")
        };
        Ok(Self {
            host: host.to_string(),
            repository,
            tag: version.to_string(),
        })
    }
}

/// Host portion must be a bare `hostname[:port]` — no userinfo (`@`),
/// no fragment (`#`), no query (`?`). An attacker-authored
/// `oci://user:pass@evil.com/foo` otherwise leaks credentials in the
/// request's URL or confuses auth header selection.
fn validate_oci_host(host: &str) -> Result<(), FetchError> {
    if host.is_empty() {
        return Err(FetchError::InvalidOciRef("empty host".to_string()));
    }
    for ch in host.chars() {
        let ok = ch.is_ascii_alphanumeric()
            || ch == '.'
            || ch == '-'
            || ch == ':'
            || ch == '['
            || ch == ']';
        if !ok {
            // Don't echo the full host — it may contain `user:pass@`
            // userinfo that an attacker stuffed in to leak via logs.
            return Err(FetchError::InvalidOciRef(format!(
                "disallowed character in OCI host (character `{ch}` not allowed; hosts must match hostname[:port])"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct OciManifestJson {
    #[serde(default)]
    layers: Vec<OciManifestLayer>,
}

#[derive(Debug, serde::Deserialize)]
struct OciManifestLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: i64,
}

/// Pick the Helm chart layer from the manifest. Strict media-type
/// enforcement — see [`HELM_LAYER_MEDIA_TYPE`].
fn pick_helm_layer(manifest: &OciManifestJson) -> Result<&OciManifestLayer, FetchError> {
    if manifest.layers.is_empty() {
        return Err(FetchError::MissingHelmLayer(
            "manifest has no layers".to_string(),
        ));
    }
    manifest
        .layers
        .iter()
        .find(|l| l.media_type == HELM_LAYER_MEDIA_TYPE)
        .ok_or_else(|| {
            FetchError::MissingHelmLayer(format!(
                "no layer with media type {HELM_LAYER_MEDIA_TYPE}; registry may have served a non-Helm artifact"
            ))
        })
}

/// Streaming OCI pull — bypasses `oci-client::pull` (which buffers the
/// full layer in a `Vec<u8>`) and talks to the registry directly via
/// reqwest. GET /v2/<repo>/manifests/<tag> for the precheck, then
/// stream /v2/<repo>/blobs/<digest> chunks to a tempfile. Peak memory
/// is one reqwest chunk (~16 KB), independent of chart size.
///
/// Returns the tempfile + the hex sha256 digest of the streamed bytes.
/// We verify at the end that the digest matches the manifest's
/// advertised layer digest — if it doesn't, the registry served
/// corrupt / wrong bytes and we surface that as
/// [`FetchError::DigestMismatch`](super::FetchError::DigestMismatch).
pub(super) async fn fetch_oci_to_file(
    dep: &Dependency,
    user_auth: &OciAuth,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    let parts = OciRef::parse(&dep.repository, &dep.name, &dep.version)?;
    let auth_header = resolve_oci_auth(&parts, user_auth).await;
    let manifest = fetch_manifest_json(&parts, auth_header.as_deref()).await?;
    let layer = pick_helm_layer(&manifest)?;
    let limit = max_download_bytes();
    let advertised = layer.size.max(0) as u64;
    if advertised > limit {
        return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
    }

    let (temp, digest) = stream_oci_blob_to_file(
        &parts,
        auth_header.as_deref(),
        &layer.digest,
        limit,
        scratch_dir,
    )
    .await?;

    // Content integrity: the registry advertised this digest; what we
    // just streamed had better match it. If it doesn't, the bytes are
    // either corrupt or a swapped layer — don't hand them to unpack.
    let layer_ref = format!("oci://{}/{}:{}", parts.host, parts.repository, parts.tag);
    super::digest::verify(&layer_ref, &layer.digest, &digest)?;
    Ok((temp, digest))
}

/// Sync wrapper over [`fetch_oci_manifest_digest`]. When a Tokio
/// runtime is already active (typical inside Temporal workers / async
/// bin crates), calls `block_on` on the current handle via a
/// `spawn_blocking` detour — avoids building a second runtime. Falls
/// back to a fresh current-thread runtime for non-async callers (the
/// CLI `akua inspect` path).
pub fn fetch_oci_manifest_digest_blocking(
    parts: &OciRef,
    user_auth: &OciAuth,
) -> Result<String, FetchError> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return tokio::task::block_in_place(|| {
            handle.block_on(fetch_oci_manifest_digest(parts, user_auth))
        });
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(FetchError::Unpack)?
        .block_on(fetch_oci_manifest_digest(parts, user_auth))
}

const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.v2+json";

/// Build a `reqwest::Request` for the manifest URL. Shared by the
/// HEAD-for-digest path ([`fetch_oci_manifest_digest`]) and the
/// GET-for-layers path ([`fetch_manifest_json`]).
fn manifest_request(
    parts: &OciRef,
    auth_header: Option<&str>,
    method: reqwest::Method,
) -> Result<reqwest::RequestBuilder, FetchError> {
    let url = format!(
        "https://{}/v2/{}/manifests/{}",
        parts.host, parts.repository, parts.tag
    );
    let http = ssrf_safe_client()?;
    let mut req = http
        .request(method, &url)
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT);
    if let Some(h) = auth_header {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    Ok(req)
}

/// Fetch the manifest digest for an OCI reference via a single HEAD
/// request. Returns the `Docker-Content-Digest` header value (e.g.
/// `"sha256:abc…"`). Consumers use this for upstream-change detection
/// without pulling the chart.
pub async fn fetch_oci_manifest_digest(
    parts: &OciRef,
    user_auth: &OciAuth,
) -> Result<String, FetchError> {
    crate::ssrf::validate_host(&parts.host)?;
    let auth_header = resolve_oci_auth(parts, user_auth).await;
    let resp = manifest_request(parts, auth_header.as_deref(), reqwest::Method::HEAD)?
        .send()
        .await?
        .error_for_status()?;
    resp.headers()
        .get("docker-content-digest")
        .ok_or_else(|| {
            FetchError::MalformedManifest(
                "registry did not return Docker-Content-Digest".into(),
            )
        })?
        .to_str()
        .map(ToString::to_string)
        .map_err(|e| FetchError::MalformedManifest(format!("non-ASCII digest header: {e}")))
}

async fn fetch_manifest_json(
    parts: &OciRef,
    auth_header: Option<&str>,
) -> Result<OciManifestJson, FetchError> {
    let resp = manifest_request(parts, auth_header, reqwest::Method::GET)?
        .send()
        .await?
        .error_for_status()?;
    resp.json::<OciManifestJson>()
        .await
        .map_err(|e| FetchError::MalformedManifest(format!("JSON parse: {e}")))
}

/// Stream the layer blob to a tempfile, computing sha256 on the fly.
/// Enforces `AKUA_MAX_DOWNLOAD_BYTES` against both the stream's
/// `Content-Length` (pre-flight) and the bytes actually received.
async fn stream_oci_blob_to_file(
    parts: &OciRef,
    auth_header: Option<&str>,
    digest: &str,
    limit: u64,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    let url = format!(
        "https://{}/v2/{}/blobs/{}",
        parts.host, parts.repository, digest
    );
    let http = ssrf_safe_client()?;
    let mut req = http.get(&url);
    if let Some(h) = auth_header {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await?.error_for_status()?;
    stream_response_to_file(resp, limit, scratch_dir).await
}

/// Resolve the Authorization header for an OCI operation. Caller creds
/// win; otherwise try the anonymous bearer-token dance that unlocks
/// public ghcr.io / bitnami / podinfo-style registries that don't
/// advertise on `/v2/`.
async fn resolve_oci_auth(parts: &OciRef, user_auth: &OciAuth) -> Option<String> {
    if let Some(creds) = user_auth.creds.get(&parts.host) {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        return Some(match creds {
            RegistryCredentials::Basic { username, password } => {
                let joined = format!("{username}:{password}");
                format!("Basic {}", STANDARD.encode(joined.as_bytes()))
            }
            RegistryCredentials::Bearer(token) => format!("Bearer {token}"),
        });
    }
    anonymous_bearer_token(parts)
        .await
        .map(|t| format!("Bearer {t}"))
}

/// Probe the manifest URL anonymously. If the registry responds with
/// `401 Bearer realm=…,service=…,scope=…`, exchange for a read-only
/// token. Returns `None` for any other outcome — caller falls back to
/// the unauthenticated request (servers allowing anonymous will accept).
async fn anonymous_bearer_token(parts: &OciRef) -> Option<String> {
    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parts.host, parts.repository, parts.tag
    );
    let http = ssrf_safe_client().ok()?;
    let probe = http.head(&manifest_url).send().await.ok()?;
    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        return None;
    }
    let header = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    let challenge = parse_bearer_challenge(header)?;
    let mut url = reqwest::Url::parse(&challenge.realm).ok()?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(service) = challenge.service.as_deref() {
            q.append_pair("service", service);
        }
        if let Some(scope) = challenge.scope.as_deref() {
            q.append_pair("scope", scope);
        } else {
            q.append_pair("scope", &format!("repository:{}:pull", parts.repository));
        }
    }
    let token_resp: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
    token_resp
        .get("token")
        .or_else(|| token_resp.get("access_token"))?
        .as_str()
        .map(ToString::to_string)
}

pub(super) struct BearerChallenge {
    pub(super) realm: String,
    pub(super) service: Option<String>,
    pub(super) scope: Option<String>,
}

/// Parse the `WWW-Authenticate: Bearer …` header into realm/service/scope.
///
/// Grammar per RFC 6750 §3: `Bearer k1="v1", k2="v2"`. Values are
/// double-quoted, keys are case-insensitive. We ignore any keys beyond
/// the three we care about (e.g. `error`, `error_description`).
pub(super) fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let body = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    let mut realm = None;
    let mut service = None;
    let mut scope = None;
    for part in body.split(',') {
        let mut kv = part.trim().splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim().trim_matches('"').to_string();
        match k.to_ascii_lowercase().as_str() {
            "realm" => realm = Some(v),
            "service" => service = Some(v),
            "scope" => scope = Some(v),
            _ => {}
        }
    }
    Some(BearerChallenge {
        realm: realm?,
        service,
        scope,
    })
}
