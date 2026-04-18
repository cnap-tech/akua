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

use std::path::{Path, PathBuf};

use crate::umbrella::Dependency;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("unsupported repository scheme in `{0}` (expected `oci://` or `https://`/`http://`)")]
    UnsupportedRepo(String),
    #[error("HTTP request: {0}")]
    Http(#[from] HttpError),
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
    /// SSRF guard tripped — the repo's host resolves to a private /
    /// loopback / link-local IP range. Bypass with
    /// `AKUA_ALLOW_PRIVATE_HOSTS=1` for local development.
    #[error("refusing to fetch from private-range host `{host}` (set AKUA_ALLOW_PRIVATE_HOSTS=1 for local dev)")]
    PrivateHost { host: String },
    /// Malformed `oci://…` reference — bad host, missing path, invalid
    /// characters. The embedded message is already scrubbed of user
    /// info via [`redact_userinfo`].
    #[error("invalid OCI reference: {0}")]
    InvalidOciRef(String),
    /// Registry served content whose sha256 doesn't match the digest
    /// advertised by the manifest / index.yaml. Don't trust the bytes.
    #[error("digest mismatch for {url}: advertised {expected}, got {actual}")]
    DigestMismatch {
        url: String,
        expected: String,
        actual: String,
    },
    /// The OCI manifest has no Helm layer: either zero layers, or
    /// no layer with the canonical `vnd.cncf.helm.chart.content.v1`
    /// media type. The registry may have served a non-Helm artifact.
    #[error("OCI manifest has no Helm layer: {0}")]
    MissingHelmLayer(String),
    /// Manifest response was malformed — missing required headers,
    /// non-ASCII digest, JSON parse failure.
    #[error("malformed manifest response: {0}")]
    MalformedManifest(String),
    /// The chart tarball violates a Helm-layout invariant
    /// (no top-level directory, multiple top-level directories).
    #[error("malformed chart tarball: {0}")]
    MalformedChart(String),
    /// Failed to build the HTTP client — either a reqwest config issue
    /// or a TLS backend initialisation error. Not a transport error.
    #[error("HTTP client configuration: {0}")]
    ClientConfig(String),
    /// Internal invariant broken — e.g., a `tokio::spawn` task panicked.
    /// Report upstream: it's probably a bug, not a user error.
    #[error("internal: {0}")]
    Internal(String),
}

mod cache;
mod digest;
mod hex;
mod http_helm;
mod oci;
mod options;
mod ssrf_client;
mod streaming;
// Named `tar_unpack` rather than `tar` to avoid shadowing the `tar`
// crate — callers in the tests module and elsewhere use unqualified
// `tar::Archive` / `tar::Builder`, which would resolve here first.
mod tar_unpack;

pub use oci::{
    fetch_oci_manifest_digest, fetch_oci_manifest_digest_blocking, OciAuth, OciRef,
    RegistryCredentials, HELM_LAYER_MEDIA_TYPE,
};
pub use ssrf_client::{redact_userinfo, HttpError};
use http_helm::fetch_http_to_file;
use oci::fetch_oci_to_file;
use ssrf_client::validate_repo_ssrf;
use tar_unpack::unpack_chart_tgz;

impl From<reqwest::Error> for FetchError {
    fn from(e: reqwest::Error) -> Self {
        FetchError::Http(HttpError::from(e))
    }
}

pub use options::{FetchOptions, LimitKind};
use options::{cache_disabled, OptionsGuard};


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

/// Like [`fetch_dependencies_with_auth`] but with per-call safety
/// limits (`max_download_bytes`, `max_extracted_bytes`, `max_tar_entries`)
/// and a cache-bypass toggle. Overrides the `AKUA_MAX_*` / `AKUA_NO_CACHE`
/// env vars for the duration of this call only. Multi-tenant hosts
/// (Temporal workers with many concurrent activities) can set
/// per-tenant limits without process-global state.
pub fn fetch_dependencies_with_options(
    deps: &[Dependency],
    charts_dir: &Path,
    auth: &OciAuth,
    options: FetchOptions,
) -> Result<(), FetchError> {
    let _guard = OptionsGuard::install(options);
    fetch_dependencies_with_auth(deps, charts_dir, auth)
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
    let no_cache = cache_disabled();
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
                .map_err(|e| FetchError::Internal(format!("fetch task panicked: {e}")))??;

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
    validate_repo_ssrf(&dep.repository)?;
    if dep.repository.starts_with("oci://") {
        fetch_oci_to_file(dep, auth, scratch_dir).await
    } else if dep.repository.starts_with("http://") || dep.repository.starts_with("https://") {
        fetch_http_to_file(dep, scratch_dir).await
    } else {
        Err(FetchError::UnsupportedRepo(dep.repository.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::oci::parse_bearer_challenge;
    use super::tar_unpack::{validate_tar_entry_path, LimitedReader};
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

    #[test]
    fn unpack_rejects_symlink_entry() {
        // Hand-build a tar archive with a symlink pointing at /etc/passwd.
        // Real Helm charts never ship links; this would otherwise let a
        // malicious package read arbitrary host files through inspect.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut chart = tar::Header::new_gnu();
            chart.set_path("mychart/Chart.yaml").unwrap();
            chart.set_size(4);
            chart.set_cksum();
            tar.append(&chart, &b"a:1\n"[..]).unwrap();

            let mut sym = tar::Header::new_gnu();
            sym.set_entry_type(tar::EntryType::Symlink);
            sym.set_path("mychart/evil.yaml").unwrap();
            sym.set_link_name("/etc/passwd").unwrap();
            sym.set_size(0);
            sym.set_cksum();
            tar.append(&sym, &[][..]).unwrap();
            tar.finish().unwrap();
        }
        use std::io::Write;
        gz.flush().unwrap();
        let tgz = gz.finish().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let charts = tmp.path().join("charts");
        std::fs::create_dir_all(&charts).unwrap();
        let err = unpack_chart_tgz(&tgz[..], &charts, "x").unwrap_err();
        assert!(
            matches!(err, FetchError::UnsafeEntryPath { .. }),
            "expected UnsafeEntryPath, got {err:?}"
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
