//! HTTP Helm repository fetch — resolve a chart via `index.yaml` and
//! stream the `.tgz`. sha256-verifies against the index's `digest`
//! when the repo publishes one (standard Helm convention).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::options::max_download_bytes;
use super::ssrf_client::{redact_userinfo, ssrf_safe_client, validate_repo_ssrf};
use super::streaming::{download_with_limit, stream_response_to_file};
use super::FetchError;
use crate::umbrella::Dependency;

pub(super) async fn fetch_http_to_file(
    dep: &Dependency,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    let resolved = resolve_http_chart_url(dep).await?;
    let resp = ssrf_safe_client(5)?
        .get(&resolved.url)
        .send()
        .await?
        .error_for_status()?;
    let (tempfile, digest) =
        stream_response_to_file(resp, max_download_bytes(), scratch_dir).await?;
    // Parity with the SDK's HTTP pull: verify the downloaded tarball
    // against `digest` from index.yaml when the repo publishes one.
    // Rejects a registry that serves tampered bytes alongside a
    // correct index entry.
    if let Some(advertised) = resolved.digest {
        super::digest::verify(&redact_userinfo(&resolved.url), &advertised, &digest)?;
    }
    Ok((tempfile, digest))
}

struct ResolvedHttpChart {
    url: String,
    digest: Option<String>,
}

async fn resolve_http_chart_url(dep: &Dependency) -> Result<ResolvedHttpChart, FetchError> {
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
    let url = if chart_url.starts_with("http://") || chart_url.starts_with("https://") {
        chart_url.clone()
    } else {
        format!("{}/{}", dep.repository.trim_end_matches('/'), chart_url)
    };
    let digest = if entry.digest.is_empty() {
        None
    } else {
        Some(entry.digest.clone())
    };
    Ok(ResolvedHttpChart { url, digest })
}

async fn fetch_repo_index(repo: &str) -> Result<RepoIndex, FetchError> {
    validate_repo_ssrf(repo)?;
    let url = format!("{}/index.yaml", repo.trim_end_matches('/'));
    let resp = ssrf_safe_client(5)?
        .get(&url)
        .send()
        .await?
        .error_for_status()?;
    let bytes = download_with_limit(resp, max_download_bytes()).await?;
    let index: RepoIndex = serde_yaml::from_slice(&bytes)?;
    Ok(index)
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
    /// Hex-encoded sha256 (Helm convention: bare hex, no `sha256:` prefix).
    /// Verified against the downloaded bytes when present.
    #[serde(default)]
    digest: String,
}
