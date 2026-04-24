//! Write a [`RenderedPackage`] to disk as raw YAML manifests.
//!
//! Spec: [`docs/package-format.md`](../../../docs/package-format.md) +
//! [`docs/cli.md` `akua render`](../../../docs/cli.md#akua-render).
//!
//! akua's sole render output is a directory of YAML files, one per
//! resource. Downstream systems that want different shapes (Helm charts,
//! OCI bundles, kro RGDs) get them via transformation functions *inside*
//! the Package body, or via future distribution verbs like `akua publish`.
//!
//! Filenames, write order, and hash inputs are derived deterministically
//! from resource position + shape, so identical inputs always produce
//! byte-identical files and summary.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::package_k::RenderedPackage;

/// Canonical format label for the render output. Flows through to the
/// JSON verdict — agents branch on it.
pub const FORMAT_RAW_MANIFESTS: &str = "raw-manifests";

crate::contract_type! {
/// Verdict for a single render: where manifests went, how many, and a
/// hash that makes "did the rendered output change?" a byte comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderSummary {
    pub format: &'static str,

    /// Directory the manifests were written into (or would be, on
    /// `--dry-run`). Absolute or relative depending on the `out_root`
    /// the caller passed.
    pub target: PathBuf,

    pub manifests: usize,

    /// `sha256:<hex>` of the concatenated `<filename>\n<yaml>` blocks.
    /// Filename participates in the hash so two identical resources
    /// routed to different filenames produce different hashes.
    pub hash: String,

    /// Relative filenames, in write order. Empty vec is serialized as
    /// omitted to keep the JSON shape tight.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PathBuf>,
}
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize resource #{index} to YAML: {source}")]
    Yaml {
        index: usize,
        #[source]
        source: serde_yaml::Error,
    },
}

/// Render `rendered.resources` into `out_root` as raw YAML. When
/// `dry_run` is true, files are not written but the summary
/// (including the hash) is computed as if they had been.
pub fn render(
    rendered: &RenderedPackage,
    out_root: &Path,
    dry_run: bool,
) -> Result<RenderSummary, RenderError> {
    if !dry_run {
        std::fs::create_dir_all(out_root).map_err(|e| RenderError::Io {
            path: out_root.to_path_buf(),
            source: e,
        })?;
    }

    let mut files = Vec::with_capacity(rendered.resources.len());
    let mut hasher = Sha256::new();
    for (i, resource) in rendered.resources.iter().enumerate() {
        let filename = manifest_filename(i, resource);
        let yaml = serde_yaml::to_string(resource).map_err(|e| RenderError::Yaml {
            index: i,
            source: e,
        })?;
        hasher.update(filename.as_os_str().as_encoded_bytes());
        hasher.update(b"\n");
        hasher.update(yaml.as_bytes());
        if !dry_run {
            let full = out_root.join(&filename);
            std::fs::write(&full, yaml.as_bytes()).map_err(|e| RenderError::Io {
                path: full,
                source: e,
            })?;
        }
        files.push(filename);
    }

    Ok(RenderSummary {
        format: FORMAT_RAW_MANIFESTS,
        target: out_root.to_path_buf(),
        manifests: rendered.resources.len(),
        hash: format!("sha256:{}", hex_encode(&hasher.finalize())),
        files,
    })
}

/// Deterministic filename: `<NNN>-<kind>-<name>.yaml`. Unknown kinds
/// fall back to `"resource"`; unnamed resources to `"unnamed"`. Non-
/// filename-safe characters in `name` collapse to `-`.
fn manifest_filename(index: usize, resource: &Value) -> PathBuf {
    let kind = resource
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("resource")
        .to_ascii_lowercase();
    let name = resource
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unnamed");
    PathBuf::from(format!(
        "{index:03}-{kind}-{sanitized}.yaml",
        sanitized = sanitize_for_filename(name),
    ))
}

fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mk_resource(kind: &str, name: &str) -> Value {
        let mut meta = serde_yaml::Mapping::new();
        meta.insert(Value::String("name".into()), Value::String(name.into()));
        let mut r = serde_yaml::Mapping::new();
        r.insert(
            Value::String("apiVersion".into()),
            Value::String("v1".into()),
        );
        r.insert(Value::String("kind".into()), Value::String(kind.into()));
        r.insert(Value::String("metadata".into()), Value::Mapping(meta));
        Value::Mapping(r)
    }

    fn pkg(resources: Vec<Value>) -> RenderedPackage {
        RenderedPackage { resources }
    }

    #[test]
    fn writes_one_file_per_resource_into_out_root() {
        let tmp = TempDir::new().unwrap();
        let p = pkg(vec![
            mk_resource("ConfigMap", "alpha"),
            mk_resource("Service", "beta"),
        ]);
        let summary = render(&p, tmp.path(), false).expect("render");
        assert_eq!(summary.manifests, 2);
        assert_eq!(summary.files.len(), 2);

        let root = tmp.path();
        assert!(root.join("000-configmap-alpha.yaml").is_file());
        assert!(root.join("001-service-beta.yaml").is_file());

        let body = std::fs::read_to_string(root.join("000-configmap-alpha.yaml")).unwrap();
        assert!(body.contains("kind: ConfigMap"));
        assert!(body.contains("name: alpha"));
    }

    #[test]
    fn dry_run_computes_summary_without_touching_disk() {
        let tmp = TempDir::new().unwrap();
        let p = pkg(vec![mk_resource("ConfigMap", "x")]);
        let summary = render(&p, tmp.path(), true).expect("render");
        assert_eq!(summary.manifests, 1);
        assert!(summary.hash.starts_with("sha256:"));
        // No files on disk (out_root itself wasn't created either).
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn hash_is_deterministic_for_identical_inputs() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let p = pkg(vec![
            mk_resource("ConfigMap", "a"),
            mk_resource("Service", "b"),
        ]);
        let a = render(&p, tmp1.path(), false).unwrap();
        let b = render(&p, tmp2.path(), false).unwrap();
        assert_eq!(a.hash, b.hash);
    }

    #[test]
    fn hash_changes_when_resource_content_changes() {
        let tmp = TempDir::new().unwrap();

        let mut cm = mk_resource("ConfigMap", "x");
        let mut data = serde_yaml::Mapping::new();
        data.insert(Value::String("k".into()), Value::String("v".into()));
        if let Value::Mapping(m) = &mut cm {
            m.insert(Value::String("data".into()), Value::Mapping(data));
        }
        let a = render(&pkg(vec![mk_resource("ConfigMap", "x")]), tmp.path(), true).unwrap();
        let b = render(&pkg(vec![cm]), tmp.path(), true).unwrap();
        assert_ne!(a.hash, b.hash);
    }

    #[test]
    fn resource_without_metadata_name_uses_unnamed_filename() {
        let tmp = TempDir::new().unwrap();
        let mut r = serde_yaml::Mapping::new();
        r.insert(
            Value::String("kind".into()),
            Value::String("Namespace".into()),
        );
        let summary = render(&pkg(vec![Value::Mapping(r)]), tmp.path(), false).unwrap();
        assert_eq!(
            summary.files[0],
            PathBuf::from("000-namespace-unnamed.yaml")
        );
    }

    #[test]
    fn resource_name_with_special_chars_is_sanitized() {
        let tmp = TempDir::new().unwrap();
        let summary = render(
            &pkg(vec![mk_resource("ConfigMap", "weird/name:with spaces")]),
            tmp.path(),
            false,
        )
        .unwrap();
        let fname = summary.files[0].to_string_lossy().into_owned();
        assert!(fname.contains("weird-name-with-spaces"), "{fname}");
    }

    #[test]
    fn empty_resource_list_produces_empty_summary() {
        let tmp = TempDir::new().unwrap();
        let summary = render(&pkg(vec![]), tmp.path(), false).unwrap();
        assert_eq!(summary.manifests, 0);
        assert!(summary.files.is_empty());
        assert!(summary.hash.starts_with("sha256:"));
    }
}
