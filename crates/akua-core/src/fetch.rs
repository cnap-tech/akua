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
}

/// Fetch every dep in `deps` into `charts_dir/<name>/`. Replaces any
/// existing contents. `charts_dir` is created if missing.
///
/// Dep entries are keyed by `alias` if present, else by `name` — matching
/// Helm's convention.
pub fn fetch_dependencies(
    deps: &[Dependency],
    charts_dir: &Path,
) -> Result<(), FetchError> {
    if deps.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(charts_dir)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(FetchError::Unpack)?;
    rt.block_on(async {
        for dep in deps {
            let target_name = dep.alias.as_deref().unwrap_or(&dep.name);
            let target_dir = charts_dir.join(target_name);
            if target_dir.exists() {
                std::fs::remove_dir_all(&target_dir)?;
            }
            let tgz = fetch_one(dep).await?;
            unpack_chart_tgz(&tgz, charts_dir, target_name)?;
        }
        Ok::<_, FetchError>(())
    })
}

async fn fetch_one(dep: &Dependency) -> Result<Vec<u8>, FetchError> {
    if dep.repository.starts_with("oci://") {
        fetch_oci(dep).await
    } else if dep.repository.starts_with("http://") || dep.repository.starts_with("https://") {
        fetch_http(dep).await
    } else {
        Err(FetchError::UnsupportedRepo(dep.repository.clone()))
    }
}

async fn fetch_oci(dep: &Dependency) -> Result<Vec<u8>, FetchError> {
    use oci_client::{client::Client, secrets::RegistryAuth, Reference};

    let reference = format!(
        "{}/{}:{}",
        dep.repository
            .trim_start_matches("oci://")
            .trim_end_matches('/'),
        dep.name,
        dep.version
    );
    let reference: Reference = reference
        .parse()
        .map_err(|e: oci_client::ParseError| FetchError::Oci(e.to_string()))?;

    let client = Client::default();
    let accepted = vec!["application/vnd.cncf.helm.chart.content.v1.tar+gzip"];
    let image = client
        .pull(&reference, &RegistryAuth::Anonymous, accepted)
        .await
        .map_err(|e| FetchError::Oci(e.to_string()))?;
    let layer = image
        .layers
        .into_iter()
        .next()
        .ok_or_else(|| FetchError::Oci("OCI manifest has no layers".to_string()))?;
    Ok(layer.data.to_vec())
}

async fn fetch_http(dep: &Dependency) -> Result<Vec<u8>, FetchError> {
    let index = fetch_repo_index(&dep.repository).await?;
    let entry = index
        .entries
        .get(&dep.name)
        .and_then(|versions| versions.iter().find(|e| e.version == dep.version))
        .ok_or_else(|| FetchError::VersionNotFound {
            name: dep.name.clone(),
            version: dep.version.clone(),
        })?;
    let chart_url = entry
        .urls
        .first()
        .ok_or_else(|| FetchError::NoChartUrl {
            name: dep.name.clone(),
        })?;
    let url = if chart_url.starts_with("http://") || chart_url.starts_with("https://") {
        chart_url.clone()
    } else {
        format!("{}/{}", dep.repository.trim_end_matches('/'), chart_url)
    };
    let resp = reqwest::get(&url).await?.error_for_status()?;
    Ok(resp.bytes().await?.to_vec())
}

async fn fetch_repo_index(repo: &str) -> Result<RepoIndex, FetchError> {
    let url = format!("{}/index.yaml", repo.trim_end_matches('/'));
    let bytes = reqwest::get(&url)
        .await?
        .error_for_status()?
        .bytes()
        .await?;
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
}

/// Unpack a `tar+gzip` chart tarball into `charts_dir/target_name/`.
///
/// Helm chart tarballs wrap the chart under a single top-level directory
/// named `<chart-name>/` (which may or may not match `target_name` —
/// e.g., `nginx-18.1.0.tgz` wraps `nginx/`). We extract the archive and
/// rename the top-level dir to `target_name` (so aliased deps land at
/// their alias).
fn unpack_chart_tgz(
    tgz: &[u8],
    charts_dir: &Path,
    target_name: &str,
) -> Result<(), FetchError> {
    let tmp = tempfile::tempdir_in(charts_dir)?;
    let gz = flate2::read::GzDecoder::new(tgz);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(tmp.path())?;

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
    let top = top.ok_or_else(|| {
        FetchError::Oci("chart tarball has no top-level directory".to_string())
    })?;
    let dest = charts_dir.join(target_name);
    std::fs::rename(&top, &dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = rt.block_on(fetch_one(&dep)).unwrap_err();
        assert!(matches!(err, FetchError::UnsupportedRepo(_)));
    }

    #[test]
    fn unpacks_tarball_under_target_name() {
        use std::io::Write;
        // Build a minimal chart tarball: nginx/Chart.yaml
        let mut gz =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
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
        unpack_chart_tgz(&tgz, &charts, "web").unwrap(); // aliased
        let chart_yaml = std::fs::read_to_string(charts.join("web/Chart.yaml")).unwrap();
        assert!(chart_yaml.contains("nginx"));
    }
}
