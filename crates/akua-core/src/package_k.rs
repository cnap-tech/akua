//! `Package.k` loader — read a KCL Package, inject inputs, execute it,
//! parse the rendered YAML into typed Rust data.
//!
//! Spec: [`docs/package-format.md`](../../../docs/package-format.md).
//!
//! Inputs flow through KCL's built-in `option()` mechanism (the
//! in-process equivalent of `kcl -D input=<json>`): the caller's
//! inputs are JSON-encoded and passed as an `ExecProgramArgs.args`
//! entry keyed by [`INPUT_OPTION_KEY`]. Packages bind it with
//!
//! ```kcl
//! input: Input = option("input") or Input {}
//! ```
//!
//! so a Package is standalone-valid KCL (`kcl fmt` / `kcl lint` / IDE
//! LSPs all work without akua-specific preprocessing).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// The `option()` key every Package uses for its `input` binding.
const INPUT_OPTION_KEY: &str = "input";

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

    #[error("failed to serialize inputs to JSON: {0}")]
    InputJson(#[from] serde_json::Error),

    #[error("kcl eval failed: {0}")]
    KclEval(String),

    #[error("kcl output is not valid YAML: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("rendered Package must set top-level `resources`; got no such key")]
    MissingResources,

    #[error("rendered Package must set top-level `outputs`; got no such key")]
    MissingOutputs,

    #[error("rendered Package must be a top-level mapping; got {got}")]
    TopLevelWrongShape { got: &'static str },

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
        let json = serde_json::to_string(inputs)?;
        let yaml = eval_kcl(&self.path, &self.source, &json)?;
        parse_rendered(&yaml)
    }
}

fn parse_rendered(yaml: &str) -> Result<RenderedPackage, PackageKError> {
    let top: Value = serde_yaml::from_str(yaml)?;
    let got = value_kind(&top);
    let Value::Mapping(map) = top else {
        return Err(PackageKError::TopLevelWrongShape { got });
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

/// Structured issue reported by [`lint_kcl`]. Mirrors KCL's own
/// `Error.messages[*]` shape, flattened one-row-per-message so
/// consumers don't need to walk a two-level tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LintIssue {
    /// `"Error"` or `"Warning"` as reported by KCL.
    pub level: String,

    /// KCL error code (e.g. `"E1001"`). Empty string when KCL emits no
    /// code — preserved verbatim.
    pub code: String,

    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
}

/// Parse a KCL file via kcl_lang and return any parse / load errors.
/// Pure; no execution.
pub fn lint_kcl(path: &Path) -> Result<Vec<LintIssue>, PackageKError> {
    use kcl_lang::{ParseProgramArgs, API};

    let api = API::default();
    let args = ParseProgramArgs {
        paths: vec![path.to_string_lossy().into_owned()],
        ..Default::default()
    };
    match api.parse_program(&args) {
        Ok(result) => Ok(result
            .errors
            .into_iter()
            .flat_map(|e| {
                let level = e.level;
                let code = e.code;
                e.messages.into_iter().map(move |m| {
                    let pos = m.pos.unwrap_or_default();
                    LintIssue {
                        level: level.clone(),
                        code: code.clone(),
                        message: m.msg,
                        file: (!pos.filename.is_empty()).then_some(pos.filename),
                        line: (pos.line > 0).then_some(pos.line),
                        column: (pos.column > 0).then_some(pos.column),
                    }
                })
            })
            .collect()),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

/// A single `option()` call-site in a parsed Package, surfaced for
/// inspection without executing the program. Mirrors kcl_lang's
/// `OptionHelp` shape with idiomatic Rust optionals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionInfo {
    /// The option key (first argument to `option("…")`).
    pub name: String,

    /// The declared type of the binding receiving the option — e.g.
    /// `"Input"` for `input: Input = option("input") or Input {}`.
    /// Empty when the option is used without a type annotation.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub r#type: String,

    pub required: bool,

    /// Default value (literal form) when the option call includes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// `help="…"` text attached to the option call, surfaced in docs
    /// tooling; absent when the authoring site didn't provide any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// List every `option()` call-site declared in the KCL program at
/// `path`. Parse-only — the program is not executed. Used by
/// `akua inspect` to introspect a Package's input surface.
pub fn list_options_kcl(path: &Path) -> Result<Vec<OptionInfo>, PackageKError> {
    use kcl_lang::{ParseProgramArgs, API};

    let api = API::default();
    let args = ParseProgramArgs {
        paths: vec![path.to_string_lossy().into_owned()],
        ..Default::default()
    };
    match api.list_options(&args) {
        Ok(result) => Ok(result
            .options
            .into_iter()
            .map(|o| OptionInfo {
                name: o.name,
                r#type: o.r#type,
                required: o.required,
                default: (!o.default_value.is_empty()).then_some(o.default_value),
                help: (!o.help.is_empty()).then_some(o.help),
            })
            .collect()),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

/// Format a KCL source string via kcl_lang's formatter. Used by
/// `akua fmt` — pure function, no filesystem access.
pub fn format_kcl(source: &str) -> Result<String, PackageKError> {
    use kcl_lang::{FormatCodeArgs, API};

    let api = API::default();
    match api.format_code(&FormatCodeArgs {
        source: source.to_string(),
    }) {
        Ok(result) => String::from_utf8(result.formatted)
            .map_err(|e| PackageKError::KclEval(format!("format output not utf-8: {e}"))),
        Err(e) => Err(PackageKError::KclEval(e.to_string())),
    }
}

fn eval_kcl(path: &Path, code: &str, option_json: &str) -> Result<String, PackageKError> {
    use kcl_lang::{Argument, ExecProgramArgs, API};

    let api = API::default();
    let args = ExecProgramArgs {
        k_filename_list: vec![path.to_string_lossy().into_owned()],
        k_code_list: vec![code.to_string()],
        args: vec![Argument {
            name: INPUT_OPTION_KEY.to_string(),
            value: option_json.to_string(),
        }],
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

    /// Pure-KCL fixture: no engine imports, no external charts. Emits
    /// one ConfigMap whose `data.count` reflects `input.replicas`. Uses
    /// the spec's canonical `option("input")` binding so it runs under
    /// vanilla `kcl` as well as through this loader.
    const MINIMAL_FIXTURE: &str = r#"
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

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
        assert_eq!(
            rendered.resources[0]["data"]["count"],
            Value::String("2".into())
        );
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

input: Input = option("input") or Input {}

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

input: Input = option("input") or Input {}

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

input: Input = option("input") or Input {}

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

input: Input = option("input") or Input {}

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

    #[test]
    fn render_threads_nested_input_through_schema() {
        let fixture = r#"
schema Database:
    user: str = "app"
    port: int = 5432

schema Input:
    appName: str = "demo"
    database: Database = Database {}

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.user: input.database.user
    data.port: str(input.database.port)
}]

outputs = [{ kind: "RawManifests", target: "./" }]
"#;
        let (_tmp, path) = write_fixture(fixture);
        let pkg = PackageK::load(&path).expect("load");

        // Nested input value across two schema levels.
        let mut db = Mapping::new();
        db.insert(
            Value::String("user".into()),
            Value::String("checkout_app".into()),
        );
        db.insert(Value::String("port".into()), Value::Number(6543.into()));

        let rendered = pkg
            .render(&inputs(&[
                ("appName", Value::String("checkout".into())),
                ("database", Value::Mapping(db)),
            ]))
            .expect("render");

        let cm = &rendered.resources[0];
        assert_eq!(cm["metadata"]["name"], Value::String("checkout".into()));
        assert_eq!(cm["data"]["user"], Value::String("checkout_app".into()));
        assert_eq!(cm["data"]["port"], Value::String("6543".into()));
    }
}
