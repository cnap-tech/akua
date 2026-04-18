//! Package manifest (`package.yaml`) — the user-authored entry point.
//!
//! v1alpha1 shape:
//!
//! ```yaml
//! apiVersion: akua.dev/v1alpha1
//! name: hello-package
//! version: 0.1.0
//! description: Optional human-readable description.
//! schema: ./values.schema.json
//! sources:
//!   - name: app
//!     helm:
//!       repo: https://charts.bitnami.com/bitnami
//!       chart: nginx
//!       version: 18.1.0
//!     values:
//!       replicaCount: 1
//! ```
//!
//! See `docs/design-package-yaml-v1.md` for the full spec.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::source::{Source, SourceValidationError};

/// The single currently-supported API version.
pub const API_VERSION: &str = "akua.dev/v1alpha1";

/// The root of a `package.yaml` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    /// Schema discriminator. Must equal [`API_VERSION`] — unknown values
    /// are rejected with a clear error so forward-compat upgrades are
    /// explicit.
    pub api_version: String,

    /// Package identity.
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Relative path (from the package directory) to the customer-input
    /// JSON Schema file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    #[serde(default)]
    pub sources: Vec<Source>,
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
    #[error(
        "unknown apiVersion `{found}` in {path}; this akua version supports: {}",
        API_VERSION
    )]
    UnsupportedApiVersion { path: String, found: String },
    #[error("in {path}: {source}")]
    SourceValidation {
        path: String,
        #[source]
        source: SourceValidationError,
    },
    #[error("in {path}: duplicate source name `{name}` (source names must be unique)")]
    DuplicateSourceName { path: String, name: String },
    #[error("in {path}: schema `{value}` must be a relative path inside the package (no absolute paths, no `..`)")]
    UnsafeSchemaPath { path: String, value: String },
}

/// Load and validate `<package_dir>/package.yaml`.
pub fn load_manifest(package_dir: &Path) -> Result<PackageManifest, LoadError> {
    let path = package_dir.join("package.yaml");
    let path_str = path.display().to_string();
    let bytes = std::fs::read(&path).map_err(|e| LoadError::Io {
        path: path_str.clone(),
        source: e,
    })?;
    let manifest: PackageManifest =
        serde_yaml::from_slice(&bytes).map_err(|e| LoadError::Parse {
            path: path_str.clone(),
            source: e,
        })?;
    validate_manifest(&manifest, &path_str)?;
    Ok(manifest)
}

/// Cross-field validation — runs after YAML parsing.
fn validate_manifest(m: &PackageManifest, path: &str) -> Result<(), LoadError> {
    if m.api_version != API_VERSION {
        return Err(LoadError::UnsupportedApiVersion {
            path: path.to_string(),
            found: m.api_version.clone(),
        });
    }
    let mut seen = HashSet::new();
    for source in &m.sources {
        source
            .kind()
            .map_err(|source| LoadError::SourceValidation {
                path: path.to_string(),
                source,
            })?;
        if !seen.insert(source.name.clone()) {
            return Err(LoadError::DuplicateSourceName {
                path: path.to_string(),
                name: source.name.clone(),
            });
        }
    }
    if let Some(schema) = m.schema.as_deref() {
        validate_schema_path(schema, path)?;
    }
    Ok(())
}

/// `manifest.schema` isn't wired up to any I/O today — but we validate
/// it now so the day someone reads that path they can't be tricked
/// into reading outside the package dir.
fn validate_schema_path(schema: &str, path: &str) -> Result<(), LoadError> {
    let p = Path::new(schema);
    if p.is_absolute() {
        return Err(LoadError::UnsafeSchemaPath {
            path: path.to_string(),
            value: schema.to_string(),
        });
    }
    for component in p.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(LoadError::UnsafeSchemaPath {
                path: path.to_string(),
                value: schema.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(yaml: &str) -> PackageManifest {
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        validate_manifest(&m, "<test>").unwrap();
        m
    }

    #[test]
    fn parses_minimal_helm_manifest() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: hello-package
version: 0.1.0
sources:
  - name: app
    helm:
      repo: https://charts.example.com
      chart: redis
      version: 7.0.0
"#;
        let m = parse(yaml);
        assert_eq!(m.name, "hello-package");
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.sources[0].name, "app");
        let helm = m.sources[0].helm.as_ref().unwrap();
        assert_eq!(helm.chart.as_deref(), Some("redis"));
        assert_eq!(helm.version, "7.0.0");
    }

    #[test]
    fn parses_manifest_with_values() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: demo
version: 1.0.0
description: Example
schema: ./values.schema.json
sources:
  - name: web
    helm:
      repo: oci://ghcr.io/org/charts
      chart: nginx
      version: 1.2.3
    values:
      replicaCount: 3
"#;
        let m = parse(yaml);
        assert_eq!(m.description.as_deref(), Some("Example"));
        assert_eq!(m.schema.as_deref(), Some("./values.schema.json"));
        assert_eq!(m.sources[0].values, Some(json!({"replicaCount": 3})));
    }

    #[test]
    fn parses_kcl_and_helmfile_sources() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: multi
version: 0.1.0
sources:
  - name: app
    kcl:
      entrypoint: ./app.k
      version: 0.1.0
  - name: stack
    helmfile:
      path: ./helmfile.yaml
      version: 0.1.0
"#;
        let m = parse(yaml);
        assert!(m.sources[0].kcl.is_some());
        assert!(m.sources[1].helmfile.is_some());
    }

    #[test]
    fn empty_sources_list_allowed() {
        let yaml = "apiVersion: akua.dev/v1alpha1\nname: x\nversion: 0.0.1\n";
        let m = parse(yaml);
        assert!(m.sources.is_empty());
    }

    #[test]
    fn rejects_missing_api_version() {
        let yaml = "name: x\nversion: 0.0.1\n";
        let err = serde_yaml::from_str::<PackageManifest>(yaml).unwrap_err();
        assert!(err.to_string().contains("apiVersion"));
    }

    #[test]
    fn rejects_unknown_api_version() {
        let yaml = "apiVersion: akua.dev/v2\nname: x\nversion: 0.0.1\n";
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        let err = validate_manifest(&m, "<test>").unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedApiVersion { .. }));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: x
version: 0.0.1
environments: []
"#;
        let err = serde_yaml::from_str::<PackageManifest>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_source_with_no_engine() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: x
version: 0.0.1
sources:
  - name: lonely
"#;
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        let err = validate_manifest(&m, "<test>").unwrap_err();
        assert!(matches!(
            err,
            LoadError::SourceValidation {
                source: SourceValidationError::NoEngine { .. },
                ..
            }
        ));
    }

    #[test]
    fn rejects_source_with_two_engines() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: x
version: 0.0.1
sources:
  - name: app
    helm:
      repo: https://a
      version: 1.0.0
    kcl:
      entrypoint: ./a.k
      version: 1.0.0
"#;
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        let err = validate_manifest(&m, "<test>").unwrap_err();
        assert!(matches!(
            err,
            LoadError::SourceValidation {
                source: SourceValidationError::MultipleEngines { .. },
                ..
            }
        ));
    }

    #[test]
    fn rejects_duplicate_source_names() {
        let yaml = r#"
apiVersion: akua.dev/v1alpha1
name: x
version: 0.0.1
sources:
  - name: app
    helm: { repo: https://a, version: 1.0.0 }
  - name: app
    helm: { repo: https://b, version: 1.0.0 }
"#;
        let m: PackageManifest = serde_yaml::from_str(yaml).unwrap();
        let err = validate_manifest(&m, "<test>").unwrap_err();
        assert!(matches!(err, LoadError::DuplicateSourceName { .. }));
    }
}
