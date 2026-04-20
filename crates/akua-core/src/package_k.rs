//! `Package.k` loader — read a KCL Package, inject inputs, execute it,
//! parse the rendered YAML into typed Rust data.
//!
//! Spec: [`docs/package-format.md`](../../../docs/package-format.md).
//!
//! # Scope
//!
//! This module handles **pure-KCL** Packages only: Packages whose body
//! is ordinary KCL code (schemas, dict literals, comprehensions). It
//! does **not** implement the engine callables (`helm.template`,
//! `kustomize.build`, `rgd.instantiate`, `pkg.render`) — those are
//! registered as KCL plugins in a later phase. Consequence for now:
//! the seven example Package.k files on disk cannot be rendered
//! through this path (they all import external engines); tests use
//! inline minimal fixtures.
//!
//! # Input injection
//!
//! `ExecProgramArgs` accepts multiple `(k_filename_list, k_code_list)`
//! entries that KCL evaluates as one module. To satisfy the `input:
//! Input` declaration in a Package, we synthesize a second KCL file
//! whose body is `input: Input = { … }` with the caller's values
//! rendered as KCL-literal syntax, and pass both files in the same
//! `exec_program` call.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// A loaded `Package.k` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageK {
    pub path: PathBuf,
    pub source: String,
}

/// Result of evaluating a `Package.k` with concrete inputs.
///
/// `resources` are the Kubernetes-shaped dicts the package emits
/// (opaque to this module — a reconciler or policy engine parses
/// them). `outputs` are the output specs declared in the package's
/// `outputs = [...]` list.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPackage {
    pub resources: Vec<Value>,
    pub outputs: Vec<OutputSpec>,
}

/// One entry in the Package's `outputs` list. Known fields are typed;
/// any extras (e.g. `chartName` on a `HelmChart` output) survive in
/// [`extras`] so the render pipeline can pass them through to the
/// relevant format emitter.
///
/// See [`docs/package-format.md §5`](../../../docs/package-format.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    pub kind: String,
    pub target: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(flatten)]
    pub extras: BTreeMap<String, Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageKError {
    #[error("Package.k not found at {path}")]
    Missing { path: PathBuf },

    #[error("i/o error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize inputs to KCL: {0}")]
    InputSerialize(String),

    #[error("kcl eval failed: {0}")]
    KclEval(String),

    #[error("kcl output is not valid YAML: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("rendered Package must set top-level `resources`; got no such key")]
    MissingResources,

    #[error("rendered Package must set top-level `outputs`; got no such key")]
    MissingOutputs,

    #[error("`resources` must be a sequence; got {got}")]
    ResourcesWrongShape { got: &'static str },

    #[error("`outputs` must be a sequence of output specs; got {got}")]
    OutputsWrongShape { got: &'static str },
}

impl PackageK {
    /// Read the file from disk. Maps `NotFound` to [`PackageKError::Missing`]
    /// so callers can distinguish "workspace not set up" from "disk broke."
    pub fn load(path: &Path) -> Result<Self, PackageKError> {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PackageKError::Missing {
                    path: path.to_path_buf(),
                });
            }
            Err(e) => {
                return Err(PackageKError::Io {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };
        Ok(PackageK {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Execute the Package against the given inputs, returning the
    /// typed resource + output lists.
    pub fn render(&self, inputs: &Value) -> Result<RenderedPackage, PackageKError> {
        let kcl_inputs = yaml_to_kcl(inputs)
            .map_err(|e| PackageKError::InputSerialize(e))?;
        let combined = combine_source(&self.source, &kcl_inputs);
        let yaml = eval_kcl(&self.path, &combined)?;
        parse_rendered(&yaml)
    }
}

/// Rewrite the Package source for KCL evaluation:
///
/// 1. Strip the bare `input: Input` declaration — KCL rejects type-
///    annotated module-level bindings without an assignment.
/// 2. Prepend the binding we actually want: `input = Input { … }`
///    with the caller's values rendered as a KCL literal.
///
/// KCL evaluates files as independent modules, so the injected binding
/// has to share the same `k_code_list` entry as the body that
/// references `input.*`.
fn combine_source(source: &str, kcl_inputs: &str) -> String {
    let stripped: String = source
        .lines()
        .filter(|line| line.trim() != "input: Input")
        .collect::<Vec<_>>()
        .join("\n");

    // Emit the binding *after* the schema definitions so `Input` is in
    // scope. Anchoring to the end of the declarations is tricky without
    // a parser, but placing the binding at the top relies on KCL's
    // hoisting of schema types — which it supports. Prepending works.
    format!("{stripped}\n\n_akua_input = Input {kcl_inputs}\ninput = _akua_input\n")
}

fn parse_rendered(yaml: &str) -> Result<RenderedPackage, PackageKError> {
    let top: Value = serde_yaml::from_str(yaml)?;
    let Value::Mapping(map) = top else {
        return Err(PackageKError::ResourcesWrongShape {
            got: value_kind(&Value::Null),
        });
    };

    let resources_val = map
        .get(Value::String("resources".into()))
        .ok_or(PackageKError::MissingResources)?;
    let resources = match resources_val {
        Value::Sequence(s) => s.clone(),
        other => {
            return Err(PackageKError::ResourcesWrongShape {
                got: value_kind(other),
            });
        }
    };

    let outputs_val = map
        .get(Value::String("outputs".into()))
        .ok_or(PackageKError::MissingOutputs)?;
    let outputs_seq = match outputs_val {
        Value::Sequence(s) => s,
        other => {
            return Err(PackageKError::OutputsWrongShape {
                got: value_kind(other),
            });
        }
    };

    let mut outputs = Vec::with_capacity(outputs_seq.len());
    for entry in outputs_seq {
        let spec: OutputSpec = serde_yaml::from_value(entry.clone())?;
        outputs.push(spec);
    }

    Ok(RenderedPackage { resources, outputs })
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged",
    }
}

/// Serialize a YAML value into KCL-literal syntax. Primitives, lists,
/// and maps are the only shapes we need; anchors/tags are rejected.
fn yaml_to_kcl(v: &Value) -> Result<String, String> {
    match v {
        Value::Null => Ok("None".to_string()),
        Value::Bool(true) => Ok("True".to_string()),
        Value::Bool(false) => Ok("False".to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::String(s) => Ok(kcl_string_literal(s)),
        Value::Sequence(seq) => {
            let items: Result<Vec<_>, _> = seq.iter().map(yaml_to_kcl).collect();
            Ok(format!("[{}]", items?.join(", ")))
        }
        Value::Mapping(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (k, val) in map {
                let key = match k {
                    Value::String(s) => s.clone(),
                    other => {
                        return Err(format!(
                            "non-string map key not supported: {:?}",
                            other
                        ));
                    }
                };
                // KCL uses identifier-or-string keys; wrap in quotes
                // unconditionally for safety.
                entries.push(format!("{} = {}", kcl_string_literal(&key), yaml_to_kcl(val)?));
            }
            Ok(format!("{{{}}}", entries.join(", ")))
        }
        Value::Tagged(_) => Err("tagged YAML values not supported in inputs".to_string()),
    }
}

fn kcl_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:04x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Invoke `kcl-lang`'s `exec_program` with a single `(path, code)`
/// pair, returning the rendered YAML string. Mirrors
/// `engine::kcl::eval_kcl` but exposes the preprocessed code string
/// rather than re-reading from disk.
fn eval_kcl(path: &Path, code: &str) -> Result<String, PackageKError> {
    use kcl_lang::{ExecProgramArgs, API};

    let api = API::default();
    let args = ExecProgramArgs {
        k_filename_list: vec![path.to_string_lossy().into_owned()],
        k_code_list: vec![code.to_string()],
        ..Default::default()
    };
    match api.exec_program(&args) {
        Ok(result) => {
            if !result.err_message.is_empty() {
                return Err(PackageKError::KclEval(result.err_message));
            }
            Ok(result.yaml_result)
        }
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Mapping;

    /// Pure-KCL fixture: no engine imports, no external charts. Tests a
    /// Package that emits a single ConfigMap whose `data.count` reflects
    /// `input.replicas`.
    const MINIMAL_FIXTURE: &str = r#"
schema Input:
    replicas: int = 2

input: Input

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "test"
    data.count: str(input.replicas)
}]

outputs = [{ kind: "RawManifests", target: "./" }]
"#;

    fn write_fixture(source: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("tmp");
        let path = dir.path().join("package.k");
        std::fs::write(&path, source).expect("write fixture");
        (dir, path)
    }

    fn inputs(pairs: &[(&str, Value)]) -> Value {
        let mut m = Mapping::new();
        for (k, v) in pairs {
            m.insert(Value::String((*k).to_string()), v.clone());
        }
        Value::Mapping(m)
    }

    fn empty_inputs() -> Value {
        Value::Mapping(Mapping::new())
    }

    #[test]
    fn load_reads_file_content() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        assert_eq!(pkg.path, path);
        assert_eq!(pkg.source, MINIMAL_FIXTURE);
    }

    #[test]
    fn load_missing_file_returns_typed_error() {
        let err = PackageK::load(Path::new("/does/not/exist.k")).unwrap_err();
        assert!(matches!(err, PackageKError::Missing { .. }));
    }

    #[test]
    fn render_minimal_configmap_fixture() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg
            .render(&inputs(&[("replicas", Value::Number(3.into()))]))
            .expect("render");

        assert_eq!(rendered.resources.len(), 1, "one ConfigMap emitted");
        let cm = &rendered.resources[0];
        assert_eq!(cm["kind"], Value::String("ConfigMap".into()));
        assert_eq!(cm["data"]["count"], Value::String("3".into()));

        assert_eq!(rendered.outputs.len(), 1);
        assert_eq!(rendered.outputs[0].kind, "RawManifests");
        assert_eq!(rendered.outputs[0].target, "./");
    }

    #[test]
    fn render_uses_default_when_input_absent() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&empty_inputs()).expect("render");
        // Default is `replicas: int = 2`.
        assert_eq!(rendered.resources[0]["data"]["count"], Value::String("2".into()));
    }

    #[test]
    fn render_is_deterministic() {
        let (_tmp, path) = write_fixture(MINIMAL_FIXTURE);
        let pkg = PackageK::load(&path).expect("load");
        let ins = inputs(&[("replicas", Value::Number(5.into()))]);
        let a = pkg.render(&ins).expect("a");
        let b = pkg.render(&ins).expect("b");
        assert_eq!(a, b, "same inputs must produce byte-identical output");
    }

    #[test]
    fn render_kcl_syntax_error_surfaces_typed() {
        let (_tmp, path) = write_fixture("this is not valid kcl !!!\n");
        let pkg = PackageK::load(&path).expect("load");
        let err = pkg.render(&empty_inputs()).unwrap_err();
        assert!(
            matches!(err, PackageKError::KclEval(_)),
            "expected KclEval, got {err:?}"
        );
    }

    #[test]
    fn render_missing_resources_typed() {
        // Declares outputs but no resources.
        let bad = r#"
schema Input:
    x: int = 0

input: Input

outputs = [{ kind: "RawManifests", target: "./" }]
"#;
        let (_tmp, path) = write_fixture(bad);
        let pkg = PackageK::load(&path).expect("load");
        let err = pkg.render(&empty_inputs()).unwrap_err();
        assert!(
            matches!(err, PackageKError::MissingResources),
            "expected MissingResources, got {err:?}"
        );
    }

    #[test]
    fn render_missing_outputs_typed() {
        let bad = r#"
schema Input:
    x: int = 0

input: Input

resources = []
"#;
        let (_tmp, path) = write_fixture(bad);
        let pkg = PackageK::load(&path).expect("load");
        let err = pkg.render(&empty_inputs()).unwrap_err();
        assert!(
            matches!(err, PackageKError::MissingOutputs),
            "expected MissingOutputs, got {err:?}"
        );
    }

    #[test]
    fn render_output_extras_are_preserved() {
        // HelmChart output with chartName + appVersion extras.
        let fixture = r#"
schema Input:
    x: int = 0

input: Input

resources = []

outputs = [{
    kind: "HelmChart"
    target: "oci://pkg.example.com/my-app"
    chartName: "my-app"
    appVersion: "1.0.0"
}]
"#;
        let (_tmp, path) = write_fixture(fixture);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&empty_inputs()).expect("render");
        let o = &rendered.outputs[0];
        assert_eq!(o.kind, "HelmChart");
        assert_eq!(o.name, None);
        assert_eq!(
            o.extras.get("chartName"),
            Some(&Value::String("my-app".into()))
        );
        assert_eq!(
            o.extras.get("appVersion"),
            Some(&Value::String("1.0.0".into()))
        );
    }

    #[test]
    fn render_named_output_routes_via_name_field() {
        let fixture = r#"
schema Input:
    x: int = 0

input: Input

resources = []

outputs = [
    { name: "static",  kind: "RawManifests",            target: "./deploy/static" }
    { name: "runtime", kind: "ResourceGraphDefinition", target: "./deploy/rgd"    }
]
"#;
        let (_tmp, path) = write_fixture(fixture);
        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&empty_inputs()).expect("render");
        assert_eq!(rendered.outputs.len(), 2);
        assert_eq!(rendered.outputs[0].name.as_deref(), Some("static"));
        assert_eq!(rendered.outputs[1].name.as_deref(), Some("runtime"));
    }

    // ----- yaml_to_kcl unit tests --------------------------------------

    #[test]
    fn yaml_to_kcl_primitives() {
        assert_eq!(yaml_to_kcl(&Value::Null).unwrap(), "None");
        assert_eq!(yaml_to_kcl(&Value::Bool(true)).unwrap(), "True");
        assert_eq!(yaml_to_kcl(&Value::Bool(false)).unwrap(), "False");
        assert_eq!(yaml_to_kcl(&Value::Number(42.into())).unwrap(), "42");
        assert_eq!(yaml_to_kcl(&Value::String("hi".into())).unwrap(), "\"hi\"");
    }

    #[test]
    fn yaml_to_kcl_escapes_strings() {
        assert_eq!(
            yaml_to_kcl(&Value::String("a\"b\\c\nd".into())).unwrap(),
            "\"a\\\"b\\\\c\\nd\"",
        );
    }

    #[test]
    fn yaml_to_kcl_sequence_and_mapping() {
        let seq = Value::Sequence(vec![
            Value::Number(1.into()),
            Value::Number(2.into()),
            Value::String("three".into()),
        ]);
        assert_eq!(yaml_to_kcl(&seq).unwrap(), "[1, 2, \"three\"]");

        let mut m = Mapping::new();
        m.insert(Value::String("k".into()), Value::String("v".into()));
        let map = Value::Mapping(m);
        assert_eq!(yaml_to_kcl(&map).unwrap(), "{\"k\" = \"v\"}");
    }

    #[test]
    fn yaml_to_kcl_rejects_non_string_keys() {
        let mut m = Mapping::new();
        m.insert(Value::Number(1.into()), Value::String("v".into()));
        let err = yaml_to_kcl(&Value::Mapping(m)).unwrap_err();
        assert!(err.contains("non-string"), "{err}");
    }
}
