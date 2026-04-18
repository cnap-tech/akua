//! Native chart dependency fetching — replaces `helm dependency update`.
//!
//! Walks an umbrella chart's `dependencies:` list and populates `charts/`
//! with extracted subchart directories. Supports:
//!
//! - **OCI repositories** (`oci://registry/ns`) via `oci-client` — pulls
//!   the Helm chart OCI artifact, extracts the `tar+gzip` layer.
//! - **HTTP Helm repositories** (`https://...`) via `reqwest` — fetches
//!   the repo's `index.yaml`, resolves the chart URL for the requested
//!   version, downloads the `.tgz`.
//!
//! Output layout: `<charts_dir>/<name-or-alias>/` containing the unpacked
//! chart. This matches what the embedded engine's chart loader expects.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::umbrella::Dependency;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("unsupported repository scheme in `{0}` (expected `oci://` or `https://`/`http://`)")]
    UnsupportedRepo(String),
    #[error("HTTP request: {0}")]
    Http(#[from] reqwest::Error),
    #[error("OCI pull: {0}")]
    Oci(String),
    #[error("parsing repo index.yaml: {0}")]
    Index(#[from] serde_yaml::Error),
    #[error("dependency `{name}` version `{version}` not found in repo index")]
    VersionNotFound { name: String, version: String },
    #[error("dependency `{name}` index entry has no usable url")]
    NoChartUrl { name: String },
    #[error("unpacking chart: {0}")]
    Unpack(#[from] std::io::Error),
    #[error("{kind} exceeded (limit {limit}, override with {env_var})")]
    LimitExceeded {
        kind: LimitKind,
        limit: u64,
        env_var: &'static str,
    },
    #[error("chart tarball entry `{path}` attempts path traversal (absolute or `..`)")]
    UnsafeEntryPath { path: String },
}

/// Which download-safety limit tripped. Backs [`FetchError::LimitExceeded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Raw bytes from the wire (HTTP body, OCI layer).
    DownloadBytes,
    /// Uncompressed bytes during tar extraction.
    ExtractedBytes,
    /// Number of entries inside a chart tarball.
    TarEntries,
}

impl LimitKind {
    /// Env var that overrides this limit at runtime.
    fn env_var(self) -> &'static str {
        match self {
            LimitKind::DownloadBytes => "AKUA_MAX_DOWNLOAD_BYTES",
            LimitKind::ExtractedBytes => "AKUA_MAX_EXTRACTED_BYTES",
            LimitKind::TarEntries => "AKUA_MAX_TAR_ENTRIES",
        }
    }
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitKind::DownloadBytes => f.write_str("download size"),
            LimitKind::ExtractedBytes => f.write_str("extracted size"),
            LimitKind::TarEntries => f.write_str("tarball entry count"),
        }
    }
}

// Download-safety limits. Defaults protect against malicious sources
// that serve huge bodies or gzip bombs; env vars override per caller.
// Public charts we've surveyed stay well under every default here.

fn max_download_bytes() -> u64 {
    env_bytes("AKUA_MAX_DOWNLOAD_BYTES", 100 * 1024 * 1024)
}

fn max_extracted_bytes() -> u64 {
    env_bytes("AKUA_MAX_EXTRACTED_BYTES", 500 * 1024 * 1024)
}

fn max_tar_entries() -> u64 {
    env_bytes("AKUA_MAX_TAR_ENTRIES", 20_000)
}

fn env_bytes(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

/// Registry credentials applied to OCI pulls. Each entry is keyed by
/// registry host (e.g. `"ghcr.io"`, `"registry.cnap.internal"`).
/// Anonymous is the fallback when a repository's host isn't in the map.
#[derive(Debug, Clone, Default)]
pub struct OciAuth {
    pub creds: std::collections::HashMap<String, RegistryCredentials>,
}

#[derive(Debug, Clone)]
pub enum RegistryCredentials {
    Basic { username: String, password: String },
    Bearer(String),
}

/// Fetch every dep in `deps` into `charts_dir/<name-or-alias>/`.
/// Dep keying matches Helm: alias-if-set, else name.
///
/// Remote fetches run concurrently; file:// copies, cache hits, and
/// tar unpacks stay on the caller's thread because they're disk-bound
/// or zero-cost. For a multi-dep umbrella with warm cache we don't even
/// spin up a runtime.
pub fn fetch_dependencies(deps: &[Dependency], charts_dir: &Path) -> Result<(), FetchError> {
    fetch_dependencies_with_auth(deps, charts_dir, &OciAuth::default())
}

/// Like [`fetch_dependencies`] but with explicit per-host credentials
/// for private OCI registries. See [`OciAuth`].
pub fn fetch_dependencies_with_auth(
    deps: &[Dependency],
    charts_dir: &Path,
    auth: &OciAuth,
) -> Result<(), FetchError> {
    if deps.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(charts_dir)?;

    // Partition deps up-front: file:// and cache hits need no network;
    // only the remainder needs a runtime.
    let no_cache = std::env::var_os("AKUA_NO_CACHE").is_some();
    let mut needs_network: Vec<(String, Dependency)> = Vec::new();
    let mut sync_paths: Vec<(String, PathBuf)> = Vec::new();

    for dep in deps {
        let target_name = dep.alias.as_deref().unwrap_or(&dep.name).to_string();
        let target_dir = charts_dir.join(&target_name);
        if target_dir.exists() {
            std::fs::remove_dir_all(&target_dir)?;
        }
        if let Some(local) = dep.repository.strip_prefix("file://") {
            copy_chart_dir(Path::new(local), &target_dir)?;
            continue;
        }
        if !no_cache {
            if let Some(path) = cache::get_path(&cache::key_for_dep(dep)) {
                sync_paths.push((target_name, path));
                continue;
            }
        }
        needs_network.push((target_name, dep.clone()));
    }

    // Unpack the synchronous wins first, streaming from the cache blob
    // on disk (no Vec<u8> in memory).
    for (target_name, path) in sync_paths {
        let file = std::fs::File::open(&path)?;
        unpack_chart_tgz(file, charts_dir, &target_name)?;
    }

    if needs_network.is_empty() {
        return Ok(());
    }

    // Multi-thread runtime so tokio::spawn gets real parallelism on
    // concurrent remote fetches. Cap worker threads at 8 — public
    // registry servers don't reward higher fan-out.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(needs_network.len().clamp(1, 8))
        .build()
        .map_err(FetchError::Unpack)?;

    // Scratch dir for download tempfiles. Lives alongside charts/ on
    // the same filesystem so atomic rename into the cache works.
    let scratch = tempfile::tempdir_in(charts_dir)?;
    let scratch_path = scratch.path().to_path_buf();

    type FetchHandle =
        tokio::task::JoinHandle<Result<(tempfile::NamedTempFile, String), FetchError>>;
    let auth = std::sync::Arc::new(auth.clone());
    rt.block_on(async move {
        let mut handles: Vec<(String, Dependency, FetchHandle)> = needs_network
            .into_iter()
            .map(|(target_name, dep)| {
                let auth = auth.clone();
                let dep_task = dep.clone();
                let scratch = scratch_path.clone();
                let handle =
                    tokio::spawn(
                        async move { fetch_one_to_file(&dep_task, &auth, &scratch).await },
                    );
                (target_name, dep, handle)
            })
            .collect();

        // Unpack as each fetch completes — tar unpack streams per-entry
        // from the on-disk tempfile, so memory stays bounded by
        // tar/gunzip internal buffers (~64 KB), not by chart size.
        for (target_name, dep, handle) in handles.drain(..) {
            let (tempfile, digest) = handle
                .await
                .map_err(|e| FetchError::Oci(format!("fetch task panicked: {e}")))??;

            // Unpack first so failures don't leave a bad blob in the
            // cache. Use try_clone so the cache::put_file below can
            // still consume the original NamedTempFile.
            let reader = std::fs::File::open(tempfile.path())?;
            unpack_chart_tgz(reader, charts_dir, &target_name)?;

            // Atomic-rename the tempfile into the cache. On failure
            // (permission, disk full, etc.) the build still succeeds.
            if std::env::var_os("AKUA_NO_CACHE").is_none() {
                let _ = cache::put_file(&cache::key_for_dep(&dep), &digest, tempfile);
            }
        }
        Ok::<_, FetchError>(())
    })
}

fn copy_chart_dir(src: &Path, dest: &Path) -> Result<(), FetchError> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_chart_dir(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Streaming fetch: downloads to a tempfile in `scratch_dir`, returning
/// the handle + the sha256 digest computed on the fly. Peak memory is
/// one chunk (~16 KB) for both HTTP and OCI — neither path holds the
/// chart in RAM.
async fn fetch_one_to_file(
    dep: &Dependency,
    auth: &OciAuth,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    if dep.repository.starts_with("oci://") {
        fetch_oci_to_file(dep, auth, scratch_dir).await
    } else if dep.repository.starts_with("http://") || dep.repository.starts_with("https://") {
        fetch_http_to_file(dep, scratch_dir).await
    } else {
        Err(FetchError::UnsupportedRepo(dep.repository.clone()))
    }
}

async fn fetch_http_to_file(
    dep: &Dependency,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    let url = resolve_http_chart_url(dep).await?;
    let resp = reqwest::get(&url).await?.error_for_status()?;
    stream_response_to_file(resp, max_download_bytes(), scratch_dir).await
}

async fn resolve_http_chart_url(dep: &Dependency) -> Result<String, FetchError> {
    let index = fetch_repo_index(&dep.repository).await?;
    let entry = index
        .entries
        .get(&dep.name)
        .and_then(|versions| versions.iter().find(|e| e.version == dep.version))
        .ok_or_else(|| FetchError::VersionNotFound {
            name: dep.name.clone(),
            version: dep.version.clone(),
        })?;
    let chart_url = entry.urls.first().ok_or_else(|| FetchError::NoChartUrl {
        name: dep.name.clone(),
    })?;
    Ok(
        if chart_url.starts_with("http://") || chart_url.starts_with("https://") {
            chart_url.clone()
        } else {
            format!("{}/{}", dep.repository.trim_end_matches('/'), chart_url)
        },
    )
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
/// corrupt / wrong bytes and we surface that as `FetchError::Oci`.
async fn fetch_oci_to_file(
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
    let expected = layer
        .digest
        .strip_prefix("sha256:")
        .unwrap_or(&layer.digest);
    if !expected.eq_ignore_ascii_case(&digest) {
        return Err(FetchError::Oci(format!(
            "layer digest mismatch: manifest said {expected}, streamed {digest}"
        )));
    }
    Ok((temp, digest))
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
            .ok_or_else(|| FetchError::UnsupportedRepo(repo.to_string()))?;
        let trimmed = without_scheme.trim_end_matches('/');
        let (host, path) = trimmed
            .split_once('/')
            .ok_or_else(|| FetchError::Oci(format!("no repository path in {repo}")))?;
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

/// Pick the Helm chart layer from the manifest. Falls back to the
/// first layer when the registry omits the canonical media type — some
/// mirrors do, and the OCI spec doesn't require it.
fn pick_helm_layer(manifest: &OciManifestJson) -> Result<&OciManifestLayer, FetchError> {
    const HELM_MEDIA_TYPE: &str = "application/vnd.cncf.helm.chart.content.v1.tar+gzip";
    if manifest.layers.is_empty() {
        return Err(FetchError::Oci("OCI manifest has no layers".to_string()));
    }
    Ok(manifest
        .layers
        .iter()
        .find(|l| l.media_type == HELM_MEDIA_TYPE)
        .unwrap_or(&manifest.layers[0]))
}

/// Sync wrapper over [`fetch_oci_manifest_digest`] that manages its
/// own current-thread runtime. Prefer this from non-async callers.
pub fn fetch_oci_manifest_digest_blocking(
    parts: &OciRef,
    user_auth: &OciAuth,
) -> Result<String, FetchError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(FetchError::Unpack)?
        .block_on(fetch_oci_manifest_digest(parts, user_auth))
}

/// Fetch the manifest digest for an OCI reference via a single HEAD
/// request. Returns the `Docker-Content-Digest` header value (e.g.
/// `"sha256:abc…"`). Consumers use this for upstream-change detection
/// without pulling the chart.
pub async fn fetch_oci_manifest_digest(
    parts: &OciRef,
    user_auth: &OciAuth,
) -> Result<String, FetchError> {
    const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
         application/vnd.docker.distribution.manifest.v2+json";
    let auth_header = resolve_oci_auth(parts, user_auth).await;
    let url = format!(
        "https://{}/v2/{}/manifests/{}",
        parts.host, parts.repository, parts.tag
    );
    let http = reqwest::Client::new();
    let mut req = http
        .head(&url)
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT);
    if let Some(h) = &auth_header {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await?.error_for_status()?;
    resp.headers()
        .get("docker-content-digest")
        .ok_or_else(|| FetchError::Oci("registry did not return Docker-Content-Digest".into()))?
        .to_str()
        .map(ToString::to_string)
        .map_err(|e| FetchError::Oci(format!("non-ASCII digest header: {e}")))
}

async fn fetch_manifest_json(
    parts: &OciRef,
    auth_header: Option<&str>,
) -> Result<OciManifestJson, FetchError> {
    const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
         application/vnd.docker.distribution.manifest.v2+json";
    let url = format!(
        "https://{}/v2/{}/manifests/{}",
        parts.host, parts.repository, parts.tag
    );
    let http = reqwest::Client::new();
    let mut req = http
        .get(&url)
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT);
    if let Some(h) = auth_header {
        req = req.header(reqwest::header::AUTHORIZATION, h);
    }
    let resp = req.send().await?.error_for_status()?;
    resp.json::<OciManifestJson>()
        .await
        .map_err(|e| FetchError::Oci(format!("manifest parse: {e}")))
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
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| FetchError::Oci(format!("reqwest client: {e}")))?;
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
    let http = reqwest::Client::new();
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

struct BearerChallenge {
    realm: String,
    service: Option<String>,
    scope: Option<String>,
}

/// Parse the `WWW-Authenticate: Bearer …` header into realm/service/scope.
///
/// Grammar per RFC 6750 §3: `Bearer k1="v1", k2="v2"`. Values are
/// double-quoted, keys are case-insensitive. We ignore any keys beyond
/// the three we care about (e.g. `error`, `error_description`).
fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
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

async fn fetch_repo_index(repo: &str) -> Result<RepoIndex, FetchError> {
    let url = format!("{}/index.yaml", repo.trim_end_matches('/'));
    let resp = reqwest::get(&url).await?.error_for_status()?;
    let bytes = download_with_limit(resp, max_download_bytes()).await?;
    let index: RepoIndex = serde_yaml::from_slice(&bytes)?;
    Ok(index)
}

/// Stream a response body into memory, aborting once `limit` bytes
/// have been read. Used for small bodies (repo `index.yaml`); for
/// chart tarballs, prefer [`stream_response_to_file`] which never
/// holds the full payload in memory.
async fn download_with_limit(
    mut resp: reqwest::Response,
    limit: u64,
) -> Result<Vec<u8>, FetchError> {
    if resp.content_length().is_some_and(|d| d > limit) {
        return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
    }
    let initial = resp
        .content_length()
        .map(|n| n.min(limit) as usize)
        .unwrap_or(4096);
    let mut buf: Vec<u8> = Vec::with_capacity(initial);
    while let Some(chunk) = resp.chunk().await? {
        if (buf.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stream a response body to an on-disk tempfile, writing chunks as
/// they arrive and computing the sha256 on the fly. Peak memory is one
/// chunk (~16 KB typical for reqwest). Returns the tempfile handle plus
/// the hex-encoded digest so the caller can both unpack from disk and
/// promote the same file into the content-addressed cache without
/// reading its bytes a second time.
async fn stream_response_to_file(
    mut resp: reqwest::Response,
    limit: u64,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    if resp.content_length().is_some_and(|d| d > limit) {
        return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
    }
    std::fs::create_dir_all(scratch_dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(scratch_dir)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        if written.saturating_add(chunk.len() as u64) > limit {
            return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
        }
        temp.write_all(&chunk)?;
        hasher.update(&chunk);
        written += chunk.len() as u64;
    }
    temp.flush()?;
    let digest = hex_digest(&hasher.finalize());
    Ok((temp, digest))
}

/// Hex-encode a 32-byte sha256 digest. Shared by the streaming fetch +
/// the cache module (both want the same on-disk format).
fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn limit_exceeded(kind: LimitKind, limit: u64) -> FetchError {
    FetchError::LimitExceeded {
        kind,
        limit,
        env_var: kind.env_var(),
    }
}

#[derive(Debug, Deserialize)]
struct RepoIndex {
    #[serde(default)]
    entries: HashMap<String, Vec<RepoEntry>>,
}

#[derive(Debug, Deserialize)]
struct RepoEntry {
    version: String,
    #[serde(default)]
    urls: Vec<String>,
}

/// Unpack a `tar+gzip` chart tarball into `charts_dir/target_name/`.
///
/// Helm chart tarballs wrap the chart under a single top-level directory
/// named `<chart-name>/` (which may or may not match `target_name` —
/// e.g., `nginx-18.1.0.tgz` wraps `nginx/`). We extract the archive and
/// rename the top-level dir to `target_name` (so aliased deps land at
/// their alias).
fn unpack_chart_tgz<R: std::io::Read>(
    tgz: R,
    charts_dir: &Path,
    target_name: &str,
) -> Result<(), FetchError> {
    let tmp = tempfile::tempdir_in(charts_dir)?;
    // Cap the gunzip stream so a gzip bomb can't fill the disk.
    // `tar::Archive::unpack` reads until EOF, which for a bomb might be
    // terabytes of zeros. The `LimitedReader` surfaces EOF early, at
    // which point `tar` returns a clean error.
    let gz = flate2::read::GzDecoder::new(tgz);
    let extract_limit = max_extracted_bytes();
    let entry_limit = max_tar_entries();
    let capped = LimitedReader::new(gz, extract_limit);
    let mut archive = tar::Archive::new(capped);

    // Walk entries manually so we can (a) enforce the entry-count cap,
    // (b) reject path-traversal attempts, (c) count uncompressed bytes
    // explicitly. `archive.unpack()` would skip all three checks.
    let mut total_entries: u64 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        total_entries += 1;
        if total_entries > entry_limit {
            return Err(limit_exceeded(LimitKind::TarEntries, entry_limit));
        }
        let path = entry.path()?.into_owned();
        validate_tar_entry_path(&path)?;
        let dest_path = tmp.path().join(&path);
        // Per-entry `unpack` doesn't create intermediate dirs, unlike the
        // one-shot `archive.unpack()`. Do it ourselves.
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&dest_path).map_err(|e| {
            if error_chain_has_extract_signal(&e) {
                limit_exceeded(LimitKind::ExtractedBytes, extract_limit)
            } else {
                FetchError::Unpack(e)
            }
        })?;
    }

    // The archive contains one top-level dir — move it to our target name.
    let mut top: Option<PathBuf> = None;
    for entry in std::fs::read_dir(tmp.path())? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if top.is_some() {
                return Err(FetchError::Oci(
                    "chart tarball has >1 top-level directory".to_string(),
                ));
            }
            top = Some(entry.path());
        }
    }
    let top =
        top.ok_or_else(|| FetchError::Oci("chart tarball has no top-level directory".to_string()))?;
    let dest = charts_dir.join(target_name);
    std::fs::rename(&top, &dest)?;
    Ok(())
}

/// Reject tar entries that would write outside the target dir: absolute
/// paths, `..` components, Windows prefix/root. `entry.unpack()` doesn't
/// filter these on its own, so the caller must.
fn validate_tar_entry_path(path: &Path) -> Result<(), FetchError> {
    use std::path::Component;
    path.components().try_for_each(|c| match c {
        Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
            Err(FetchError::UnsafeEntryPath {
                path: path.display().to_string(),
            })
        }
        _ => Ok(()),
    })
}

/// Caps the gunzip stream so a decompression bomb fails early instead
/// of filling the disk.
struct LimitedReader<R> {
    inner: R,
    consumed: u64,
    limit: u64,
}

impl<R: std::io::Read> LimitedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            consumed: 0,
            limit,
        }
    }
}

/// Marker substring in the `io::Error` that `LimitedReader` emits. The
/// extract path recognises "chart too big" vs "chart broken" by
/// scanning the error chain for this string.
///
/// Why string-match rather than a typed downcast: `io::Error::source()`
/// delegates to the *inner* error's `source()`, so a boxed typed
/// payload is skipped by the chain walker — the payload is never
/// directly visible as a node in the chain. The substring is process-
/// internal; `FetchError::LimitExceeded` is what the caller actually
/// sees.
const EXTRACT_LIMIT_SIGNAL: &str = "akua extract limit";

fn error_chain_has_extract_signal(err: &std::io::Error) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if e.to_string().contains(EXTRACT_LIMIT_SIGNAL) {
            return true;
        }
        current = e.source();
    }
    false
}

impl<R: std::io::Read> std::io::Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.consumed >= self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{EXTRACT_LIMIT_SIGNAL} ({} bytes) exceeded", self.limit),
            ));
        }
        let remaining = self.limit - self.consumed;
        let to_read = std::cmp::min(buf.len() as u64, remaining) as usize;
        let n = self.inner.read(&mut buf[..to_read])?;
        self.consumed += n as u64;
        Ok(n)
    }
}

/// Content-addressed on-disk chart cache.
///
/// Layout under `$XDG_CACHE_HOME/akua/v1/` (or `$HOME/.cache/akua/v1/`):
///
/// ```text
/// refs/<sha256(key)>       # contents: hex sha256 of the blob
/// blobs/<sha256(blob)>.tgz # the cached tarball bytes
/// ```
///
/// Two-tier: `refs/` keys a lookup by `(repo, name, version)` → content
/// digest; `blobs/` stores tarballs content-addressed so identical
/// tarballs dedupe across names. Writes are atomic via `tempfile::persist`.
/// All failures are non-fatal — the caller falls back to live download.
mod cache {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::path::PathBuf;

    use crate::umbrella::Dependency;

    pub(super) fn key_for_dep(dep: &Dependency) -> String {
        format!("{}|{}|{}", dep.repository, dep.name, dep.version)
    }

    pub(super) fn get(key: &str) -> Option<Vec<u8>> {
        let root = root()?;
        let key_hash = hex_sha256(key.as_bytes());
        let ref_path = root.join("refs").join(&key_hash);
        let blob_digest = std::fs::read_to_string(&ref_path).ok()?;
        let blob_digest = blob_digest.trim();
        if blob_digest.len() != 64 || !blob_digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let blob_path = root.join("blobs").join(format!("{blob_digest}.tgz"));
        let bytes = std::fs::read(&blob_path).ok()?;
        // Integrity check: a corrupted blob is worse than a cache miss.
        if hex_sha256(&bytes) != blob_digest {
            return None;
        }
        Some(bytes)
    }

    pub(super) fn put(key: &str, bytes: &[u8]) -> std::io::Result<()> {
        let Some(root) = root() else {
            return Ok(());
        };
        let blob_digest = hex_sha256(bytes);
        let (blobs_dir, refs_dir) = ensure_dirs(&root)?;
        let blob_path = blobs_dir.join(format!("{blob_digest}.tgz"));
        if !blob_path.exists() {
            let mut tmp = tempfile::NamedTempFile::new_in(&blobs_dir)?;
            tmp.write_all(bytes)?;
            tmp.flush()?;
            tmp.persist(&blob_path).map_err(|e| e.error)?;
        }
        write_ref(&refs_dir, key, &blob_digest)
    }

    /// Cache-put from an already-written tempfile. The caller has
    /// streamed bytes to the tempfile and knows the sha256 digest;
    /// we atomic-rename the file into `blobs/<digest>.tgz` (zero
    /// extra reads). Preferred over [`put`] for chart-sized payloads.
    pub(super) fn put_file(
        key: &str,
        digest: &str,
        temp: tempfile::NamedTempFile,
    ) -> std::io::Result<()> {
        let Some(root) = root() else {
            return Ok(());
        };
        let (blobs_dir, refs_dir) = ensure_dirs(&root)?;
        let blob_path = blobs_dir.join(format!("{digest}.tgz"));
        if blob_path.exists() {
            // Someone else already raced us — their file is
            // content-addressed so identical bytes either way. Drop
            // the tempfile and reuse theirs.
            drop(temp);
        } else {
            temp.persist(&blob_path).map_err(|e| e.error)?;
        }
        write_ref(&refs_dir, key, digest)
    }

    /// Look up the cached blob path for `key` without reading its
    /// bytes. Integrity check happens at read time by the consumer
    /// (streaming unpack). Caller must re-verify if paranoid.
    pub(super) fn get_path(key: &str) -> Option<PathBuf> {
        let root = root()?;
        let ref_path = root.join("refs").join(hex_sha256(key.as_bytes()));
        let blob_digest = std::fs::read_to_string(&ref_path).ok()?;
        let blob_digest = blob_digest.trim();
        if blob_digest.len() != 64 || !blob_digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let blob_path = root.join("blobs").join(format!("{blob_digest}.tgz"));
        blob_path.exists().then_some(blob_path)
    }

    fn ensure_dirs(root: &std::path::Path) -> std::io::Result<(PathBuf, PathBuf)> {
        let blobs = root.join("blobs");
        let refs = root.join("refs");
        std::fs::create_dir_all(&blobs)?;
        std::fs::create_dir_all(&refs)?;
        Ok((blobs, refs))
    }

    fn write_ref(refs_dir: &std::path::Path, key: &str, digest: &str) -> std::io::Result<()> {
        let key_hash = hex_sha256(key.as_bytes());
        let ref_path = refs_dir.join(&key_hash);
        let mut tmp = tempfile::NamedTempFile::new_in(refs_dir)?;
        tmp.write_all(digest.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&ref_path).map_err(|e| e.error)?;
        Ok(())
    }

    /// Resolve the cache root, honouring `XDG_CACHE_HOME` and `HOME`.
    /// Returns `None` on systems where neither is set (e.g. some CI
    /// sandboxes) — caller falls back to no-cache behaviour.
    fn root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("AKUA_CACHE_DIR") {
            return Some(PathBuf::from(dir).join("v1"));
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(base.join("akua").join("v1"))
    }

    fn hex_sha256(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut out = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ScopedEnvVar;

    #[test]
    fn rejects_tar_entry_with_absolute_path() {
        let p = Path::new("/etc/passwd");
        let err = validate_tar_entry_path(p).unwrap_err();
        assert!(matches!(err, FetchError::UnsafeEntryPath { .. }));
    }

    #[test]
    fn rejects_tar_entry_with_parent_dir_component() {
        let p = Path::new("chart/../../etc/passwd");
        let err = validate_tar_entry_path(p).unwrap_err();
        assert!(matches!(err, FetchError::UnsafeEntryPath { .. }));
    }

    #[test]
    fn accepts_normal_tar_entry_paths() {
        validate_tar_entry_path(Path::new("mychart/Chart.yaml")).unwrap();
        validate_tar_entry_path(Path::new("mychart/templates/deploy.yaml")).unwrap();
        validate_tar_entry_path(Path::new("mychart/charts/sub/Chart.yaml")).unwrap();
    }

    #[test]
    fn limited_reader_stops_at_cap() {
        use std::io::Read;
        // A source with 1000 bytes, cap at 100.
        let data = vec![0u8; 1000];
        let mut reader = LimitedReader::new(&data[..], 100);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("extract limit"));
        // Up to the cap was read; not a byte more.
        assert_eq!(out.len(), 100);
    }

    #[test]
    fn unpack_rejects_oversized_extraction() {
        // Build a tar that DOES fit on disk uncompressed (small), but
        // set the extract cap so small that even the real content blows
        // past it. Same code path as a gzip bomb, just deterministic.
        let tgz = build_chart_tgz(&[("mychart/Chart.yaml", &[0u8; 10_000])]);
        let tmp = tempfile::tempdir().unwrap();
        let charts = tmp.path().join("charts");
        std::fs::create_dir_all(&charts).unwrap();

        let _lock = DL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVar::set("AKUA_MAX_EXTRACTED_BYTES", "1024");
        let err = unpack_chart_tgz(&tgz[..], &charts, "x").unwrap_err();
        assert!(
            matches!(
                err,
                FetchError::LimitExceeded {
                    kind: LimitKind::ExtractedBytes,
                    ..
                }
            ),
            "unexpected: {err:?}"
        );
    }

    // The happy-path "reject `..`" integration test is covered by the
    // `rejects_tar_entry_with_*` unit tests — `tar::Header::set_path`
    // itself refuses to pack `..` into an archive, so we can't build
    // a malicious tar via the library. Our runtime validator is
    // defense-in-depth for attacks that hand-craft raw tar bytes on
    // the wire, and unit-tested directly above.

    #[test]
    fn unpack_rejects_too_many_entries() {
        // 200 empty entries; cap the limit to 50.
        let mut pairs: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..200 {
            pairs.push((format!("mychart/f{i}.yaml"), b"a:1\n".to_vec()));
        }
        let pairs_refs: Vec<(&str, &[u8])> = pairs
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let tgz = build_chart_tgz(&pairs_refs);

        let tmp = tempfile::tempdir().unwrap();
        let charts = tmp.path().join("charts");
        std::fs::create_dir_all(&charts).unwrap();

        let _lock = DL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVar::set("AKUA_MAX_TAR_ENTRIES", "50");
        let err = unpack_chart_tgz(&tgz[..], &charts, "x").unwrap_err();
        assert!(
            matches!(
                err,
                FetchError::LimitExceeded {
                    kind: LimitKind::TarEntries,
                    ..
                }
            ),
            "unexpected: {err:?}"
        );
    }

    /// Build a minimal tgz chart with the given entries. Test helper —
    /// not meant for use outside tests.
    fn build_chart_tgz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (path, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(path).unwrap();
                header.set_size(body.len() as u64);
                header.set_cksum();
                tar.append(&header, *body).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.flush().unwrap();
        gz.finish().unwrap()
    }

    /// Tests that tweak `AKUA_MAX_*` env vars must serialize; the vars
    /// are process-global.
    static DL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn copies_file_repo_into_charts_dir() {
        // Simulate a materialised local subchart (what KCL / helmfile
        // engines produce) and confirm `fetch_dependencies` copies it
        // verbatim into `charts/<alias>/`.
        let tmp = tempfile::tempdir().unwrap();
        let src_chart = tmp.path().join("materialised/my-chart");
        std::fs::create_dir_all(src_chart.join("templates")).unwrap();
        std::fs::write(
            src_chart.join("Chart.yaml"),
            "apiVersion: v2\nname: my-chart\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(src_chart.join("templates/cm.yaml"), "kind: ConfigMap\n").unwrap();

        let charts_dir = tmp.path().join("umbrella/charts");
        let dep = Dependency {
            name: "my-chart".to_string(),
            version: "0.1.0".to_string(),
            repository: format!("file://{}", src_chart.display()),
            alias: Some("my-chart-alias".to_string()),
            condition: None,
        };
        fetch_dependencies(std::slice::from_ref(&dep), &charts_dir).unwrap();

        let dest = charts_dir.join("my-chart-alias");
        assert!(dest.join("Chart.yaml").exists());
        assert!(dest.join("templates/cm.yaml").exists());
    }

    #[test]
    fn rejects_unsupported_scheme() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dep = Dependency {
            name: "x".to_string(),
            version: "1.0.0".to_string(),
            repository: "git+ssh://example.com/repo".to_string(),
            alias: None,
            condition: None,
        };
        let scratch = tempfile::tempdir().unwrap();
        let err = rt
            .block_on(fetch_one_to_file(&dep, &OciAuth::default(), scratch.path()))
            .unwrap_err();
        assert!(matches!(err, FetchError::UnsupportedRepo(_)));
    }

    #[test]
    fn cache_roundtrip_returns_same_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _dir = ScopedEnvVar::set("AKUA_CACHE_DIR", tmp.path());
        let _bypass = ScopedEnvVar::remove("AKUA_NO_CACHE");
        let key = "https://example.com|nginx|18.1.0";
        let bytes = b"fake-tgz-content-1234567890".to_vec();
        assert!(cache::get(key).is_none(), "fresh dir should miss");
        cache::put(key, &bytes).unwrap();
        assert_eq!(cache::get(key).expect("hit after put"), bytes);
    }

    #[test]
    fn cache_miss_on_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _dir = ScopedEnvVar::set("AKUA_CACHE_DIR", tmp.path());
        assert!(cache::get("never-written").is_none());
    }

    #[test]
    fn cache_detects_corrupted_blob() {
        // Bad writes / disk flips shouldn't silently return tampered bytes.
        let tmp = tempfile::tempdir().unwrap();
        let _lock = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _dir = ScopedEnvVar::set("AKUA_CACHE_DIR", tmp.path());
        let key = "corrupt|test|1.0.0";
        cache::put(key, b"original-bytes").unwrap();
        let blob = std::fs::read_dir(tmp.path().join("v1/blobs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&blob, b"tampered!").unwrap();
        assert!(cache::get(key).is_none(), "integrity check must reject");
    }

    #[test]
    fn cache_dedupes_identical_blobs_under_different_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _dir = ScopedEnvVar::set("AKUA_CACHE_DIR", tmp.path());
        let bytes = b"identical-content".to_vec();
        cache::put("repo-a|chart|1.0.0", &bytes).unwrap();
        cache::put("repo-b|chart|1.0.0", &bytes).unwrap();
        let refs = std::fs::read_dir(tmp.path().join("v1/refs"))
            .unwrap()
            .count();
        let blobs = std::fs::read_dir(tmp.path().join("v1/blobs"))
            .unwrap()
            .count();
        assert_eq!(refs, 2);
        assert_eq!(blobs, 1);
    }

    /// Env vars are process-global; serialize cache tests that set
    /// `AKUA_CACHE_DIR` / `AKUA_NO_CACHE` so they don't race.
    static CACHE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_ghcr_bearer_challenge() {
        // Exact header string ghcr.io emits for a public pull.
        let hdr = r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:stefanprodan/charts/podinfo:pull""#;
        let c = parse_bearer_challenge(hdr).expect("parses");
        assert_eq!(c.realm, "https://ghcr.io/token");
        assert_eq!(c.service.as_deref(), Some("ghcr.io"));
        assert_eq!(
            c.scope.as_deref(),
            Some("repository:stefanprodan/charts/podinfo:pull")
        );
    }

    #[test]
    fn parses_bearer_challenge_ignores_unknown_keys() {
        let hdr = r#"Bearer realm="https://auth.example.com",error="invalid_token""#;
        let c = parse_bearer_challenge(hdr).expect("parses");
        assert_eq!(c.realm, "https://auth.example.com");
        assert!(c.service.is_none());
        assert!(c.scope.is_none());
    }

    #[test]
    fn rejects_non_bearer_challenge() {
        assert!(parse_bearer_challenge("Basic realm=\"x\"").is_none());
    }

    #[test]
    fn unpacks_tarball_under_target_name() {
        // Serialize with the other extract tests — they toggle
        // AKUA_MAX_EXTRACTED_BYTES globally and would trip this one if
        // we didn't hold the lock while it runs with default limits.
        let _g = DL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::Write;
        // Build a minimal chart tarball: nginx/Chart.yaml
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            let body = b"name: nginx\nversion: 1.0.0\n";
            header.set_path("nginx/Chart.yaml").unwrap();
            header.set_size(body.len() as u64);
            header.set_cksum();
            tar.append(&header, body.as_slice()).unwrap();
            tar.finish().unwrap();
        }
        gz.flush().unwrap();
        let tgz = gz.finish().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let charts = tmp.path().join("charts");
        std::fs::create_dir_all(&charts).unwrap();
        unpack_chart_tgz(&tgz[..], &charts, "web").unwrap(); // aliased
        let chart_yaml = std::fs::read_to_string(charts.join("web/Chart.yaml")).unwrap();
        assert!(chart_yaml.contains("nginx"));
    }
}
