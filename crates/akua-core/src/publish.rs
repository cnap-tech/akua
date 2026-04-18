//! Native OCI publish — no helm CLI required. Matches Helm v4's push
//! behaviour bit-for-bit (media types, tarball layout, config JSON shape,
//! OCI manifest annotations).
//!
//! Packages a built chart directory as a Helm OCI artifact and pushes it
//! to a registry using [`oci-client`] (pure Rust). Replaces what was
//! previously a shell to `helm package` + `helm push`.
//!
//! The emitted artifact follows Helm v4's OCI convention:
//!
//! - **Config**: `application/vnd.cncf.helm.config.v1+json` — the full
//!   Chart.yaml metadata, JSON-serialized (not just name/version).
//! - **Layer**: `application/vnd.cncf.helm.chart.content.v1.tar+gzip` —
//!   the chart packaged as `<chart-name>/…` at the root of the tarball.
//! - **Manifest annotations**: `org.opencontainers.image.{title, version,
//!   description, url, source, authors, created}` derived from chart meta,
//!   plus Chart.yaml's own `annotations:` map (minus the immutable
//!   `title` / `version` keys).
//! - **Ref**: `<target>/<chart-name>:<chart-version>`.
//!
//! Source reference: <https://github.com/helm/helm/blob/main/pkg/registry/client.go>
//! (`Push` + `generateOCIAnnotations`).
//!
//! Auth: explicit `BasicAuth` via `PublishOptions.auth`, or anonymous.
//! Docker-config-based credential resolution is a follow-up.

use std::path::{Path, PathBuf};

use oci_client::{
    client::{Client, ClientConfig, Config, ImageLayer, PushResponse},
    manifest::OciImageManifest,
    secrets::RegistryAuth,
    Reference,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid target URL `{0}` — expected `oci://<registry>/<namespace>`")]
    InvalidTarget(String),
    #[error("invalid chart reference: {0}")]
    InvalidReference(#[from] oci_client::ParseError),
    #[error("tokio runtime: {0}")]
    Runtime(String),
    #[error("OCI push: {0}")]
    Push(#[from] oci_client::errors::OciDistributionError),
    #[error("JSON serialize: {0}")]
    Json(#[from] serde_json::Error),
}

/// Options for the OCI publish.
#[derive(Debug, Clone, Default)]
pub struct PublishOptions {
    /// `oci://<registry>/<namespace>` — the chart's final repo becomes
    /// `<namespace>/<chart-name>` under the registry, tagged with the
    /// chart version.
    pub target: String,
    /// Optional basic auth. Anonymous if `None`.
    pub auth: Option<BasicAuth>,
}

#[derive(Debug, Clone)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct PublishOutcome {
    /// Fully qualified pushed reference, e.g.
    /// `ghcr.io/acme/charts/my-app:1.0.0`.
    pub pushed_ref: String,
    /// OCI manifest digest (`sha256:…`).
    pub digest: String,
}

// Helm's OCI media types — matches pkg/registry/constants.go in helm/helm.
const CONFIG_MEDIA_TYPE: &str = "application/vnd.cncf.helm.config.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.cncf.helm.chart.content.v1.tar+gzip";

/// Full Chart.yaml metadata. Mirrors `helm.sh/helm/v4/pkg/chart/v2.Metadata`
/// so the JSON config Helm sees on pull is identical to what `helm push`
/// would have emitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChartMetadata {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    home: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    maintainers: Vec<Maintainer>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    icon: String,
    #[serde(
        rename = "apiVersion",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    api_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    condition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tags: String,
    #[serde(
        rename = "appVersion",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    app_version: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deprecated: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    annotations: std::collections::BTreeMap<String, String>,
    #[serde(
        rename = "kubeVersion",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    kube_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<serde_json::Value>,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    chart_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Maintainer {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    url: String,
}

/// Outcome of [`package_chart`].
#[derive(Debug, Clone)]
pub struct PackageOutcome {
    /// Absolute path to the written `.tgz`.
    pub path: PathBuf,
    /// Chart name from `Chart.yaml`.
    pub name: String,
    /// Chart version from `Chart.yaml`.
    pub version: String,
    /// Byte length of the written archive.
    pub size: u64,
}

/// Package `chart_dir` as a gzipped tarball and write it to `out_dir` as
/// `<name>-<version>.tgz`. Matches Helm's packaging convention: the tarball
/// wraps a single top-level directory named `<chart-name>/`.
///
/// Creates `out_dir` if missing. Returns the written path and chart metadata
/// for display.
pub fn package_chart(chart_dir: &Path, out_dir: &Path) -> Result<PackageOutcome, PublishError> {
    let meta = read_chart_yaml(chart_dir)?;
    let tarball = package_chart_tgz(chart_dir, &meta.name)?;
    std::fs::create_dir_all(out_dir).map_err(|source| PublishError::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;
    let filename = format!("{}-{}.tgz", meta.name, meta.version);
    let out_path = out_dir.join(&filename);
    std::fs::write(&out_path, &tarball).map_err(|source| PublishError::Write {
        path: out_path.clone(),
        source,
    })?;
    Ok(PackageOutcome {
        path: out_path,
        name: meta.name,
        version: meta.version,
        size: tarball.len() as u64,
    })
}

/// Package `chart_dir` and push it to `opts.target`. Blocks until the push
/// completes. Returns the fully qualified ref + manifest digest.
///
/// Matches `helm push` v4 semantics: same media types, same config JSON
/// shape, same OCI manifest annotations.
pub fn publish_chart(
    chart_dir: &Path,
    opts: &PublishOptions,
) -> Result<PublishOutcome, PublishError> {
    let chart_meta = read_chart_yaml(chart_dir)?;
    let namespace_url = parse_target(&opts.target)?;
    let reference = format!(
        "{}/{}:{}",
        namespace_url, chart_meta.name, chart_meta.version
    );
    let reference: Reference = reference.parse().map_err(PublishError::InvalidReference)?;

    let tarball = package_chart_tgz(chart_dir, &chart_meta.name)?;
    let config_json = serde_json::to_vec(&chart_meta)?;
    let annotations = generate_oci_annotations(&chart_meta);

    let auth = match &opts.auth {
        Some(a) => RegistryAuth::Basic(a.username.clone(), a.password.clone()),
        None => RegistryAuth::Anonymous,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PublishError::Runtime(e.to_string()))?;

    rt.block_on(async move {
        let client = Client::new(ClientConfig::default());
        let config = Config {
            data: config_json.into(),
            media_type: CONFIG_MEDIA_TYPE.to_string(),
            annotations: None,
        };
        let layers = vec![ImageLayer {
            data: tarball.into(),
            media_type: LAYER_MEDIA_TYPE.to_string(),
            annotations: None,
        }];
        let mut manifest = OciImageManifest::build(&layers, &config, None);
        manifest.annotations = Some(annotations);
        let PushResponse { manifest_url, .. } = client
            .push(&reference, &layers, config, &auth, Some(manifest))
            .await?;
        Ok(PublishOutcome {
            pushed_ref: reference.whole(),
            digest: extract_digest(&manifest_url).unwrap_or_else(|| manifest_url.clone()),
        })
    })
}

/// Build the OCI manifest annotations from chart metadata. Mirrors
/// `generateOCIAnnotations` + `generateChartOCIAnnotations` in
/// `helm/helm/pkg/registry/chart.go`.
fn generate_oci_annotations(meta: &ChartMetadata) -> std::collections::BTreeMap<String, String> {
    use std::collections::BTreeMap;
    const TITLE: &str = "org.opencontainers.image.title";
    const VERSION: &str = "org.opencontainers.image.version";
    let immutable = [TITLE, VERSION];

    let mut out: BTreeMap<String, String> = BTreeMap::new();
    insert_if_nonempty(
        &mut out,
        "org.opencontainers.image.description",
        &meta.description,
    );
    insert_if_nonempty(&mut out, TITLE, &meta.name);
    insert_if_nonempty(&mut out, VERSION, &meta.version);
    insert_if_nonempty(&mut out, "org.opencontainers.image.url", &meta.home);

    let created = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    out.insert("org.opencontainers.image.created".to_string(), created);

    if let Some(first) = meta.sources.first() {
        insert_if_nonempty(&mut out, "org.opencontainers.image.source", first);
    }

    if !meta.maintainers.is_empty() {
        let mut authors = String::new();
        for (i, m) in meta.maintainers.iter().enumerate() {
            if !m.name.is_empty() {
                authors.push_str(&m.name);
            }
            if !m.email.is_empty() {
                authors.push_str(" (");
                authors.push_str(&m.email);
                authors.push(')');
            }
            if i + 1 < meta.maintainers.len() {
                authors.push_str(", ");
            }
        }
        insert_if_nonempty(&mut out, "org.opencontainers.image.authors", &authors);
    }

    // Copy chart's own annotations, skipping the immutable keys.
    for (k, v) in &meta.annotations {
        if !immutable.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }

    out
}

fn insert_if_nonempty(
    map: &mut std::collections::BTreeMap<String, String>,
    key: &str,
    value: &str,
) {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
        map.insert(key.to_string(), trimmed.to_string());
    }
}

fn parse_target(target: &str) -> Result<String, PublishError> {
    let without_scheme = target
        .strip_prefix("oci://")
        .ok_or_else(|| PublishError::InvalidTarget(target.to_string()))?;
    if without_scheme.is_empty() || without_scheme.contains(':') {
        return Err(PublishError::InvalidTarget(target.to_string()));
    }
    Ok(without_scheme.trim_end_matches('/').to_string())
}

fn read_chart_yaml(chart_dir: &Path) -> Result<ChartMetadata, PublishError> {
    let path = chart_dir.join("Chart.yaml");
    let bytes = std::fs::read(&path).map_err(|source| PublishError::Read {
        path: path.clone(),
        source,
    })?;
    serde_yaml::from_slice(&bytes).map_err(|source| PublishError::Parse { path, source })
}

/// Package the chart dir as a gzipped tarball per Helm's convention: a
/// single top-level directory named `<chart-name>/` containing all chart
/// files.
fn package_chart_tgz(chart_dir: &Path, chart_name: &str) -> Result<Vec<u8>, PublishError> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(chart_name, chart_dir)
            .map_err(|source| PublishError::Write {
                path: chart_dir.to_path_buf(),
                source,
            })?;
        tar.finish().map_err(|source| PublishError::Write {
            path: chart_dir.to_path_buf(),
            source,
        })?;
    }
    gz.finish().map_err(|source| PublishError::Write {
        path: chart_dir.to_path_buf(),
        source,
    })
}

fn extract_digest(manifest_url: &str) -> Option<String> {
    // manifest_url often looks like https://registry/v2/.../manifests/sha256:abc
    manifest_url
        .rsplit('/')
        .next()
        .filter(|s| s.starts_with("sha256:"))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oci_target() {
        assert_eq!(
            parse_target("oci://ghcr.io/acme/charts").unwrap(),
            "ghcr.io/acme/charts"
        );
    }

    #[test]
    fn parses_oci_target_with_trailing_slash() {
        assert_eq!(parse_target("oci://ghcr.io/acme/").unwrap(), "ghcr.io/acme");
    }

    #[test]
    fn rejects_non_oci_target() {
        assert!(parse_target("https://ghcr.io/acme").is_err());
        assert!(parse_target("ghcr.io/acme").is_err());
    }

    #[test]
    fn rejects_target_with_tag() {
        // Target should be the namespace only — chart name + tag are derived.
        assert!(parse_target("oci://ghcr.io/acme/charts:v1").is_err());
    }

    #[test]
    fn packages_chart_dir_as_tgz_with_chart_named_root() {
        let tmp = tempfile::tempdir().unwrap();
        let chart_dir = tmp.path().join("mychart");
        std::fs::create_dir_all(chart_dir.join("templates")).unwrap();
        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: mychart\nversion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(
            chart_dir.join("templates/deploy.yaml"),
            "kind: Deployment\n",
        )
        .unwrap();

        let tgz = package_chart_tgz(&chart_dir, "mychart").unwrap();
        // Validate: read back the tarball, check the entries.
        let gz = flate2::read::GzDecoder::new(&tgz[..]);
        let mut archive = tar::Archive::new(gz);
        let mut paths: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().display().to_string())
            .collect();
        paths.sort();
        assert!(paths.iter().any(|p| p == "mychart/Chart.yaml"));
        assert!(paths.iter().any(|p| p == "mychart/templates/deploy.yaml"));
    }

    #[test]
    fn package_chart_writes_named_tgz_and_reports_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let chart_dir = tmp.path().join("src");
        std::fs::create_dir_all(chart_dir.join("templates")).unwrap();
        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.3.1\n",
        )
        .unwrap();
        std::fs::write(
            chart_dir.join("templates/deploy.yaml"),
            "kind: Deployment\n",
        )
        .unwrap();

        let out_dir = tmp.path().join("dist");
        let outcome = package_chart(&chart_dir, &out_dir).unwrap();
        assert_eq!(outcome.name, "demo");
        assert_eq!(outcome.version, "0.3.1");
        assert_eq!(outcome.path, out_dir.join("demo-0.3.1.tgz"));
        assert!(outcome.path.exists());
        assert_eq!(
            outcome.size,
            std::fs::metadata(&outcome.path).unwrap().len()
        );

        // Re-open and confirm the tarball layout is still `demo/…`.
        let bytes = std::fs::read(&outcome.path).unwrap();
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        let paths: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().display().to_string())
            .collect();
        assert!(paths.iter().any(|p| p == "demo/Chart.yaml"));
    }

    #[test]
    fn package_chart_creates_out_dir_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let chart_dir = tmp.path().join("src");
        std::fs::create_dir_all(&chart_dir).unwrap();
        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: x\nversion: 0.0.1\n",
        )
        .unwrap();
        let out_dir = tmp.path().join("deeply/nested/out");
        assert!(!out_dir.exists());
        package_chart(&chart_dir, &out_dir).unwrap();
        assert!(out_dir.join("x-0.0.1.tgz").exists());
    }

    #[test]
    fn extracts_digest_from_manifest_url() {
        assert_eq!(
            extract_digest("https://ghcr.io/v2/acme/charts/manifests/sha256:abc"),
            Some("sha256:abc".to_string())
        );
    }

    #[test]
    fn extracts_digest_returns_full_url_when_not_digest() {
        assert_eq!(extract_digest("https://ghcr.io/v2/acme/manifests/v1"), None);
    }

    #[test]
    fn parses_chart_yaml_into_full_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Chart.yaml"),
            r#"apiVersion: v2
name: myapp
version: 1.2.3
appVersion: "2.0"
description: A test chart
type: application
home: https://example.com
sources:
  - https://github.com/acme/myapp
maintainers:
  - name: Alice
    email: alice@example.com
keywords:
  - test
annotations:
  category: Application
"#,
        )
        .unwrap();
        let m = read_chart_yaml(tmp.path()).unwrap();
        assert_eq!(m.name, "myapp");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.app_version, "2.0");
        assert_eq!(m.api_version, "v2");
        assert_eq!(m.chart_type, "application");
        assert_eq!(m.home, "https://example.com");
        assert_eq!(m.sources, vec!["https://github.com/acme/myapp"]);
        assert_eq!(m.maintainers[0].name, "Alice");
        assert_eq!(m.annotations.get("category").unwrap(), "Application");
    }

    #[test]
    fn config_json_shape_matches_helm_metadata() {
        let meta = ChartMetadata {
            name: "mychart".to_string(),
            version: "1.0.0".to_string(),
            description: "hi".to_string(),
            api_version: "v2".to_string(),
            chart_type: "application".to_string(),
            ..ChartMetadata {
                name: String::new(),
                home: String::new(),
                sources: vec![],
                version: String::new(),
                description: String::new(),
                keywords: vec![],
                maintainers: vec![],
                icon: String::new(),
                api_version: String::new(),
                condition: String::new(),
                tags: String::new(),
                app_version: String::new(),
                deprecated: false,
                annotations: Default::default(),
                kube_version: String::new(),
                dependencies: vec![],
                chart_type: String::new(),
            }
        };
        let json = serde_json::to_string(&meta).unwrap();
        // Helm's JSON: apiVersion, appVersion, kubeVersion are camelCased.
        assert!(json.contains("\"apiVersion\":\"v2\""));
        assert!(json.contains("\"name\":\"mychart\""));
        assert!(json.contains("\"version\":\"1.0.0\""));
        assert!(json.contains("\"type\":\"application\""));
        // Empty fields omitted — matches Helm's `omitempty` tags.
        assert!(!json.contains("\"home\""));
        assert!(!json.contains("\"maintainers\""));
    }

    #[test]
    fn oci_annotations_include_core_fields() {
        let meta = ChartMetadata {
            name: "mychart".to_string(),
            version: "1.0.0".to_string(),
            description: "A chart".to_string(),
            home: "https://example.com".to_string(),
            sources: vec!["https://github.com/a/b".to_string()],
            maintainers: vec![Maintainer {
                name: "Alice".to_string(),
                email: "a@b.c".to_string(),
                url: String::new(),
            }],
            ..ChartMetadata {
                name: String::new(),
                home: String::new(),
                sources: vec![],
                version: String::new(),
                description: String::new(),
                keywords: vec![],
                maintainers: vec![],
                icon: String::new(),
                api_version: String::new(),
                condition: String::new(),
                tags: String::new(),
                app_version: String::new(),
                deprecated: false,
                annotations: Default::default(),
                kube_version: String::new(),
                dependencies: vec![],
                chart_type: String::new(),
            }
        };
        let a = generate_oci_annotations(&meta);
        assert_eq!(a.get("org.opencontainers.image.title").unwrap(), "mychart");
        assert_eq!(a.get("org.opencontainers.image.version").unwrap(), "1.0.0");
        assert_eq!(
            a.get("org.opencontainers.image.description").unwrap(),
            "A chart"
        );
        assert_eq!(
            a.get("org.opencontainers.image.url").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            a.get("org.opencontainers.image.source").unwrap(),
            "https://github.com/a/b"
        );
        assert_eq!(
            a.get("org.opencontainers.image.authors").unwrap(),
            "Alice (a@b.c)"
        );
        assert!(a.contains_key("org.opencontainers.image.created"));
    }

    #[test]
    fn oci_annotations_preserve_chart_annotations_except_immutable() {
        let mut chart_anns = std::collections::BTreeMap::new();
        chart_anns.insert("category".to_string(), "App".to_string());
        chart_anns.insert(
            "org.opencontainers.image.title".to_string(),
            "SHOULD_NOT_OVERRIDE".to_string(),
        );
        let meta = ChartMetadata {
            name: "real-name".to_string(),
            version: "1.0.0".to_string(),
            annotations: chart_anns,
            ..ChartMetadata {
                name: String::new(),
                home: String::new(),
                sources: vec![],
                version: String::new(),
                description: String::new(),
                keywords: vec![],
                maintainers: vec![],
                icon: String::new(),
                api_version: String::new(),
                condition: String::new(),
                tags: String::new(),
                app_version: String::new(),
                deprecated: false,
                annotations: Default::default(),
                kube_version: String::new(),
                dependencies: vec![],
                chart_type: String::new(),
            }
        };
        let a = generate_oci_annotations(&meta);
        assert_eq!(a.get("category").unwrap(), "App");
        // Title is immutable — chart.annotations[title] doesn't win.
        assert_eq!(
            a.get("org.opencontainers.image.title").unwrap(),
            "real-name"
        );
    }
}
