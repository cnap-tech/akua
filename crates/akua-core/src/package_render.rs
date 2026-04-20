//! Write a [`RenderedPackage`] to disk as raw YAML manifests.
//!
//! Spec: [`docs/package-format.md §5`](../../../docs/package-format.md#5-outputs--what-akua-emits)
//! + [`docs/cli.md` `akua render`](../../../docs/cli.md#akua-render).
//!
//! Walks `outputs[]`, emits each one, and returns a [`RenderSummary`]
//! the CLI shapes into its JSON verdict. Only the `RawManifests` kind
//! is wired up; other kinds return [`RenderError::UnsupportedKind`]
//! until their engine callables land.
//!
//! Filenames, write order, and hash inputs are derived deterministically
//! from resource position + shape, so identical inputs always produce
//! byte-identical files and summary.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::package_k::{OutputSpec, RenderedPackage};

/// Canonical format label for the `RawManifests` output kind. Flows
/// through to the JSON verdict — agents branch on it.
pub const FORMAT_RAW_MANIFESTS: &str = "raw-manifests";

/// The [`OutputSpec::kind`] string matched by the raw-manifests emitter.
const KIND_RAW_MANIFESTS: &str = "RawManifests";

/// Top-level result of rendering a Package. One entry per output that
/// was actually written (or would have been, under `--dry-run`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderSummary {
    pub outputs: Vec<OutputSummary>,
}

/// Per-output verdict: where manifests went, how many, and a hash that
/// makes "did the rendered output change?" a byte comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

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

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(
        "output kind `{kind}` is not implemented yet — only `RawManifests` is supported in Phase A"
    )]
    UnsupportedKind { kind: String },

    #[error("--output `{name}` matched no output declared by the Package")]
    OutputNotFound { name: String },

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

/// Render every output in `rendered` (optionally filtered by name) into
/// `out_root`. When `dry_run` is true, files are not written but the
/// summary (including hashes) is computed as if they had been.
pub fn render_outputs(
    rendered: &RenderedPackage,
    out_root: &Path,
    output_filter: Option<&str>,
    dry_run: bool,
) -> Result<RenderSummary, RenderError> {
    let selected = select_outputs(&rendered.outputs, output_filter)?;
    let mut summaries = Vec::with_capacity(selected.len());
    for output in selected {
        if output.kind != KIND_RAW_MANIFESTS {
            return Err(RenderError::UnsupportedKind {
                kind: output.kind.clone(),
            });
        }
        summaries.push(write_raw_manifests(
            output,
            &rendered.resources,
            out_root,
            dry_run,
        )?);
    }
    Ok(RenderSummary { outputs: summaries })
}

fn select_outputs<'a>(
    outputs: &'a [OutputSpec],
    filter: Option<&str>,
) -> Result<Vec<&'a OutputSpec>, RenderError> {
    match filter {
        Some(name) => {
            let hits: Vec<&OutputSpec> = outputs
                .iter()
                .filter(|o| o.name.as_deref() == Some(name))
                .collect();
            if hits.is_empty() {
                Err(RenderError::OutputNotFound {
                    name: name.to_string(),
                })
            } else {
                Ok(hits)
            }
        }
        None => Ok(outputs.iter().collect()),
    }
}

fn write_raw_manifests(
    output: &OutputSpec,
    resources: &[Value],
    out_root: &Path,
    dry_run: bool,
) -> Result<OutputSummary, RenderError> {
    let target = resolve_target(out_root, &output.target);
    if !dry_run {
        std::fs::create_dir_all(&target).map_err(|e| RenderError::Io {
            path: target.clone(),
            source: e,
        })?;
    }

    let mut files = Vec::with_capacity(resources.len());
    let mut hasher = Sha256::new();
    for (i, resource) in resources.iter().enumerate() {
        let filename = manifest_filename(i, resource);
        let yaml = serde_yaml::to_string(resource)
            .map_err(|e| RenderError::Yaml { index: i, source: e })?;
        hasher.update(filename.as_os_str().as_encoded_bytes());
        hasher.update(b"\n");
        hasher.update(yaml.as_bytes());
        if !dry_run {
            let full = target.join(&filename);
            std::fs::write(&full, yaml.as_bytes()).map_err(|e| RenderError::Io {
                path: full,
                source: e,
            })?;
        }
        files.push(filename);
    }

    Ok(OutputSummary {
        name: output.name.clone(),
        format: FORMAT_RAW_MANIFESTS,
        target,
        manifests: resources.len(),
        hash: format!("sha256:{}", hex_encode(&hasher.finalize())),
        files,
    })
}

/// Resolve an `OutputSpec::target` against the CLI's `--out` root.
/// Absolute targets (`/tmp/x`) pass through unchanged; relative targets
/// (`./deploy/static`, `./`) join onto `out_root`. Leading `./`
/// components are normalised out so `target: "./"` resolves to
/// `out_root` itself — the path `<root>/./` trips `create_dir_all`
/// on some platforms.
fn resolve_target(out_root: &Path, target_spec: &str) -> PathBuf {
    let t = Path::new(target_spec);
    if t.is_absolute() {
        return t.to_path_buf();
    }
    let clean: PathBuf = t
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    if clean.as_os_str().is_empty() {
        out_root.to_path_buf()
    } else {
        out_root.join(clean)
    }
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
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn mk_resource(kind: &str, name: &str) -> Value {
        let mut meta = serde_yaml::Mapping::new();
        meta.insert(Value::String("name".into()), Value::String(name.into()));
        let mut r = serde_yaml::Mapping::new();
        r.insert(Value::String("apiVersion".into()), Value::String("v1".into()));
        r.insert(Value::String("kind".into()), Value::String(kind.into()));
        r.insert(Value::String("metadata".into()), Value::Mapping(meta));
        Value::Mapping(r)
    }

    fn raw_manifests_output(name: Option<&str>, target: &str) -> OutputSpec {
        OutputSpec {
            kind: KIND_RAW_MANIFESTS.into(),
            target: target.into(),
            name: name.map(str::to_string),
            extras: BTreeMap::new(),
        }
    }

    fn package(resources: Vec<Value>, outputs: Vec<OutputSpec>) -> RenderedPackage {
        RenderedPackage { resources, outputs }
    }

    #[test]
    fn writes_one_file_per_resource_into_target_dir() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![
                mk_resource("ConfigMap", "alpha"),
                mk_resource("Service", "beta"),
            ],
            vec![raw_manifests_output(None, "./")],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, false).expect("render");
        assert_eq!(summary.outputs.len(), 1);
        let out = &summary.outputs[0];
        assert_eq!(out.manifests, 2);
        assert_eq!(out.files.len(), 2);

        let root = tmp.path();
        assert!(root.join("000-configmap-alpha.yaml").is_file());
        assert!(root.join("001-service-beta.yaml").is_file());

        let body = std::fs::read_to_string(root.join("000-configmap-alpha.yaml")).unwrap();
        assert!(body.contains("kind: ConfigMap"));
        assert!(body.contains("name: alpha"));
    }

    #[test]
    fn target_subpath_joins_onto_out_root() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "x")],
            vec![raw_manifests_output(None, "./deploy/static")],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, false).expect("render");
        let target = &summary.outputs[0].target;
        assert!(target.ends_with("deploy/static"));
        assert!(target.join("000-configmap-x.yaml").is_file());
    }

    #[test]
    fn dry_run_computes_summary_without_touching_disk() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "x")],
            vec![raw_manifests_output(None, "./")],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, true).expect("render");
        assert_eq!(summary.outputs[0].manifests, 1);
        assert!(summary.outputs[0].hash.starts_with("sha256:"));

        // No files on disk (the target dir itself wasn't created either).
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn hash_is_deterministic_for_identical_inputs() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let pkg = package(
            vec![
                mk_resource("ConfigMap", "a"),
                mk_resource("Service", "b"),
            ],
            vec![raw_manifests_output(None, "./")],
        );
        let a = render_outputs(&pkg, tmp1.path(), None, false).unwrap();
        let b = render_outputs(&pkg, tmp2.path(), None, false).unwrap();
        assert_eq!(a.outputs[0].hash, b.outputs[0].hash);
    }

    #[test]
    fn hash_changes_when_resource_content_changes() {
        let tmp = TempDir::new().unwrap();
        let outputs = vec![raw_manifests_output(None, "./")];

        let mut cm = mk_resource("ConfigMap", "x");
        // Attach a data field to the second variant.
        let mut data = serde_yaml::Mapping::new();
        data.insert(Value::String("k".into()), Value::String("v".into()));
        if let Value::Mapping(m) = &mut cm {
            m.insert(Value::String("data".into()), Value::Mapping(data));
        }
        let pkg_a = package(vec![mk_resource("ConfigMap", "x")], outputs.clone());
        let pkg_b = package(vec![cm], outputs);

        let a = render_outputs(&pkg_a, tmp.path(), None, true).unwrap();
        let b = render_outputs(&pkg_b, tmp.path(), None, true).unwrap();
        assert_ne!(a.outputs[0].hash, b.outputs[0].hash);
    }

    #[test]
    fn output_filter_selects_named_output() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "x")],
            vec![
                raw_manifests_output(Some("static"), "./static"),
                raw_manifests_output(Some("audit"), "./audit"),
            ],
        );
        let summary = render_outputs(&pkg, tmp.path(), Some("static"), false).unwrap();
        assert_eq!(summary.outputs.len(), 1);
        assert_eq!(summary.outputs[0].name.as_deref(), Some("static"));
        assert!(tmp.path().join("static").exists());
        assert!(!tmp.path().join("audit").exists(), "audit should not be rendered");
    }

    #[test]
    fn output_filter_no_match_is_typed_error() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "x")],
            vec![raw_manifests_output(Some("static"), "./")],
        );
        let err = render_outputs(&pkg, tmp.path(), Some("runtime"), false).unwrap_err();
        assert!(matches!(err, RenderError::OutputNotFound { ref name } if name == "runtime"));
    }

    #[test]
    fn unsupported_kind_is_typed_error() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "x")],
            vec![OutputSpec {
                kind: "ResourceGraphDefinition".into(),
                target: "./".into(),
                name: None,
                extras: BTreeMap::new(),
            }],
        );
        let err = render_outputs(&pkg, tmp.path(), None, false).unwrap_err();
        assert!(matches!(err, RenderError::UnsupportedKind { ref kind }
            if kind == "ResourceGraphDefinition"));
    }

    #[test]
    fn resource_without_metadata_name_uses_unnamed_filename() {
        let tmp = TempDir::new().unwrap();
        let mut r = serde_yaml::Mapping::new();
        r.insert(Value::String("kind".into()), Value::String("Namespace".into()));
        let pkg = package(
            vec![Value::Mapping(r)],
            vec![raw_manifests_output(None, "./")],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, false).unwrap();
        assert_eq!(
            summary.outputs[0].files[0],
            PathBuf::from("000-namespace-unnamed.yaml")
        );
    }

    #[test]
    fn resource_name_with_special_chars_is_sanitized() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![mk_resource("ConfigMap", "weird/name:with spaces")],
            vec![raw_manifests_output(None, "./")],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, false).unwrap();
        let fname = summary.outputs[0].files[0].to_string_lossy().into_owned();
        // Slash, colon, and space all collapse to `-`.
        assert!(fname.contains("weird-name-with-spaces"), "{fname}");
    }

    #[test]
    fn empty_resource_list_produces_empty_output_summary() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(vec![], vec![raw_manifests_output(None, "./")]);
        let summary = render_outputs(&pkg, tmp.path(), None, false).unwrap();
        assert_eq!(summary.outputs[0].manifests, 0);
        assert!(summary.outputs[0].files.is_empty());
        // Hash of nothing is still a stable sha256.
        assert!(summary.outputs[0].hash.starts_with("sha256:"));
    }

    #[test]
    fn multiple_outputs_each_receive_all_resources() {
        let tmp = TempDir::new().unwrap();
        let pkg = package(
            vec![
                mk_resource("ConfigMap", "a"),
                mk_resource("Service", "b"),
            ],
            vec![
                raw_manifests_output(Some("one"), "./one"),
                raw_manifests_output(Some("two"), "./two"),
            ],
        );
        let summary = render_outputs(&pkg, tmp.path(), None, false).unwrap();
        assert_eq!(summary.outputs.len(), 2);
        assert_eq!(summary.outputs[0].manifests, 2);
        assert_eq!(summary.outputs[1].manifests, 2);
        assert!(tmp.path().join("one/000-configmap-a.yaml").is_file());
        assert!(tmp.path().join("two/000-configmap-a.yaml").is_file());
    }
}
