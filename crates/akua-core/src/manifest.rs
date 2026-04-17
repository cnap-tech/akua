//! Package manifest (`package.yaml`) — the user-authored entry point.
//!
//! A package manifest is a YAML file listing the package's identity, its Helm
//! sources, and an optional pointer to `values.schema.json`. It's what `akua`
//! reads to assemble the umbrella chart.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::source::HelmSource;

/// The root of a `package.yaml` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub sources: Vec<HelmSource>,
}

/// Errors raised while loading a package manifest from disk.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

/// Load and parse `<package_dir>/package.yaml`.
pub fn load_manifest(package_dir: &Path) -> Result<PackageManifest, LoadError> {
    let path = package_dir.join("package.yaml");
    let bytes = std::fs::read(&path).map_err(|e| LoadError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_yaml::from_slice(&bytes).map_err(|e| LoadError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_minimal_manifest() {
        let yaml = r#"
name: hello-package
version: 0.1.0
sources:
  - id: app
    chart:
      repoUrl: https://charts.example.com
      chart: redis
      targetRevision: 7.0.0
"#;
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(m.name, "hello-package");
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.sources[0].id.as_deref(), Some("app"));
        assert_eq!(m.sources[0].chart.chart.as_deref(), Some("redis"));
    }

    #[test]
    fn parses_manifest_with_values_and_oci() {
        let yaml = r#"
name: demo
version: 1.0.0
description: Example
sources:
  - id: web
    chart:
      repoUrl: oci://ghcr.io/org/charts
      targetRevision: 1.2.3
    values:
      replicaCount: 3
"#;
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(m.description.as_deref(), Some("Example"));
        assert_eq!(m.sources[0].values, Some(json!({"replicaCount": 3})));
    }

    #[test]
    fn empty_sources_list_allowed() {
        let yaml = "name: x\nversion: 0.0.1\n";
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        assert!(m.sources.is_empty());
    }
}
