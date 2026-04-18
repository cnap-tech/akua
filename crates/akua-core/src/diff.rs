//! Structural chart diff.
//!
//! Given two parsed charts (typically pulled from OCI or read from
//! disk), produce a [`ChartDiff`] that describes the **shape** of
//! what changed — not the rendered manifest, which `helm diff` owns
//! and requires both values and a cluster to produce. This diff is
//! about:
//!
//! - `Chart.yaml` metadata: version, appVersion, maintainers,
//!   dependencies added / removed / updated.
//! - `values.yaml` defaults: which keys gained, lost, or shifted.
//! - `values.schema.json`: which input fields were added, removed,
//!   retyped, flipped required ↔ optional, or had their CEL
//!   transforms rewired.
//!
//! Consumers: the CLI (`akua diff`) for operator triage ("what
//! changed between 6.6.0 and 6.7.1?"), the install wizard
//! (future — show the customer what will change when they upgrade),
//! and CI-embedded contract checks.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A chart snapshot suitable for [`compare`]. Produced from a
/// `ChartContents` (CLI), an `inspectChartBytes` result (SDK), or
/// hand-rolled from parsed YAML/JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartSnapshot {
    /// Parsed `Chart.yaml`. Missing is unexpected — caller should
    /// bail before building the snapshot.
    pub chart_yaml: Value,
    /// Parsed `values.yaml` (may be `Null` when the chart has no
    /// default values).
    pub values_yaml: Value,
    /// Parsed `values.schema.json` when the chart ships one, else
    /// `None`. Absent schema is fine — we diff what we have.
    pub values_schema: Option<Value>,
}

/// The full structural diff between two charts. All fields are
/// independently populated; a caller can render or serialize each
/// section without touching the others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChartDiff {
    pub metadata: MetadataDiff,
    pub dependencies: DependencyDiff,
    pub values: ValuesDiff,
    pub schema: SchemaDiff,
}

impl ChartDiff {
    /// True when no field changed — the two charts are structurally
    /// identical (templates may still differ; that's `helm diff`'s
    /// territory).
    pub fn is_empty(&self) -> bool {
        self.metadata.changes.is_empty()
            && self.dependencies.added.is_empty()
            && self.dependencies.removed.is_empty()
            && self.dependencies.updated.is_empty()
            && self.values.added.is_empty()
            && self.values.removed.is_empty()
            && self.values.changed.is_empty()
            && self.schema.fields_added.is_empty()
            && self.schema.fields_removed.is_empty()
            && self.schema.fields_changed.is_empty()
    }
}

/// Top-level `Chart.yaml` keys that shifted. Only tracks scalars /
/// short lists; `dependencies` has its own [`DependencyDiff`] because
/// the add/remove/update semantics are richer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetadataDiff {
    /// Field-by-field changes. Values are JSON-serialised for
    /// format-agnostic rendering (operator CLI, JSON machine output,
    /// JSR browser wizard).
    pub changes: BTreeMap<String, FieldChange>,
}

/// A paired before/after snapshot of a single scalar or small
/// collection. `None` means absent on that side — use that to
/// distinguish "field added" vs "field modified".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// Changes to `Chart.yaml::dependencies`. Keyed on `alias` when set,
/// else `name` — matches Helm's own uniqueness rule, and matches
/// how akua nests values under each source in the umbrella.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DependencyDiff {
    /// Dependencies that appear only on the `after` side.
    pub added: Vec<DependencyEntry>,
    /// Dependencies that appear only on the `before` side.
    pub removed: Vec<DependencyEntry>,
    /// Dependencies that changed between versions. Each entry carries
    /// the before/after snapshot so the caller can render exactly
    /// what shifted.
    pub updated: Vec<DependencyUpdate>,
}

/// Minimal dependency identity — enough for add/remove rendering
/// without re-serialising the full Chart.yaml entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEntry {
    pub key: String,
    pub name: String,
    pub version: String,
    pub repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyUpdate {
    pub key: String,
    pub before: DependencyEntry,
    pub after: DependencyEntry,
}

/// Changes to `values.yaml` defaults, expressed as dot-notation paths.
/// Nested maps are flattened so the output is tool-friendly (`akua
/// diff --format json | jq` is a first-class use case).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ValuesDiff {
    pub added: BTreeMap<String, Value>,
    pub removed: BTreeMap<String, Value>,
    pub changed: BTreeMap<String, FieldChange>,
}

/// Changes to `values.schema.json`. Field paths follow the same
/// dot-notation convention as [`crate::schema::extract_install_fields`]
/// so the install wizard can cross-reference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaDiff {
    pub fields_added: BTreeMap<String, SchemaField>,
    pub fields_removed: BTreeMap<String, SchemaField>,
    /// Field-level changes. Tracks the sub-keys the install wizard
    /// cares about — `type`, `required`, `default`, `enum`, and the
    /// `x-input`/`x-user-input` extensions — not the full schema
    /// node, since most JSON Schema keys (`description`, `title`) are
    /// copy that doesn't affect install behaviour.
    pub fields_changed: BTreeMap<String, SchemaFieldChanges>,
}

/// Summary of one leaf schema field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaField {
    pub type_: Option<String>,
    pub required: bool,
    pub default: Option<Value>,
}

/// Each populated field means "this aspect of the schema node
/// changed". Aspects left as `None` are unchanged between before
/// and after.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaFieldChanges {
    pub type_changed: Option<FieldChange>,
    pub required_changed: Option<FieldChange>,
    pub default_changed: Option<FieldChange>,
    pub enum_changed: Option<FieldChange>,
    /// Change to the `x-input` transform bag (CEL expression, etc).
    /// Surfaced because a transform rewrite changes install semantics
    /// even when the nominal type is unchanged.
    pub x_input_changed: Option<FieldChange>,
}

impl SchemaFieldChanges {
    pub fn is_empty(&self) -> bool {
        self.type_changed.is_none()
            && self.required_changed.is_none()
            && self.default_changed.is_none()
            && self.enum_changed.is_none()
            && self.x_input_changed.is_none()
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Produce a [`ChartDiff`] from two snapshots. Pure function — the
/// caller owns I/O (pulling, reading from disk).
pub fn compare(before: &ChartSnapshot, after: &ChartSnapshot) -> ChartDiff {
    ChartDiff {
        metadata: compare_metadata(&before.chart_yaml, &after.chart_yaml),
        dependencies: compare_dependencies(&before.chart_yaml, &after.chart_yaml),
        values: compare_values(&before.values_yaml, &after.values_yaml),
        schema: compare_schema(
            before.values_schema.as_ref(),
            after.values_schema.as_ref(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Metadata (Chart.yaml scalar-ish fields)
// ---------------------------------------------------------------------------

/// Keys we diff explicitly. Fields like `description` are deliberately
/// excluded — they change constantly and don't affect install
/// behaviour. Maintainers in/out is worth flagging (trust signal).
const METADATA_KEYS: &[&str] = &[
    "version",
    "appVersion",
    "kubeVersion",
    "type",
    "apiVersion",
    "deprecated",
    "icon",
    "home",
    "maintainers",
    "keywords",
    "annotations",
];

fn compare_metadata(before: &Value, after: &Value) -> MetadataDiff {
    let mut changes = BTreeMap::new();
    for key in METADATA_KEYS {
        let b = before.get(*key).cloned();
        let a = after.get(*key).cloned();
        if b != a {
            changes.insert((*key).to_string(), FieldChange { before: b, after: a });
        }
    }
    MetadataDiff { changes }
}

// ---------------------------------------------------------------------------
// Dependencies
// ---------------------------------------------------------------------------

fn compare_dependencies(before: &Value, after: &Value) -> DependencyDiff {
    let b_map = dependency_map(before);
    let a_map = dependency_map(after);

    let b_keys: BTreeSet<_> = b_map.keys().collect();
    let a_keys: BTreeSet<_> = a_map.keys().collect();

    let added = a_keys
        .difference(&b_keys)
        .map(|k| a_map[*k].clone())
        .collect();
    let removed = b_keys
        .difference(&a_keys)
        .map(|k| b_map[*k].clone())
        .collect();
    let updated = b_keys
        .intersection(&a_keys)
        .filter_map(|k| {
            let before = &b_map[*k];
            let after = &a_map[*k];
            (before != after).then(|| DependencyUpdate {
                key: (*k).clone(),
                before: before.clone(),
                after: after.clone(),
            })
        })
        .collect();

    DependencyDiff {
        added,
        removed,
        updated,
    }
}

fn dependency_map(chart_yaml: &Value) -> BTreeMap<String, DependencyEntry> {
    let mut out = BTreeMap::new();
    let Some(deps) = chart_yaml.get("dependencies").and_then(Value::as_array) else {
        return out;
    };
    for dep in deps {
        let name = dep
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let alias = dep.get("alias").and_then(Value::as_str).map(str::to_string);
        let version = dep
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let repository = dep
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let key = alias.clone().unwrap_or_else(|| name.clone());
        out.insert(
            key.clone(),
            DependencyEntry {
                key,
                name,
                version,
                repository,
            },
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Values diff
// ---------------------------------------------------------------------------

fn compare_values(before: &Value, after: &Value) -> ValuesDiff {
    let mut b_flat = BTreeMap::new();
    let mut a_flat = BTreeMap::new();
    flatten_values(before, "", &mut b_flat);
    flatten_values(after, "", &mut a_flat);

    let mut diff = ValuesDiff::default();
    for (path, b_val) in &b_flat {
        match a_flat.get(path) {
            None => {
                diff.removed.insert(path.clone(), b_val.clone());
            }
            Some(a_val) if a_val != b_val => {
                diff.changed.insert(
                    path.clone(),
                    FieldChange {
                        before: Some(b_val.clone()),
                        after: Some(a_val.clone()),
                    },
                );
            }
            Some(_) => {}
        }
    }
    for (path, a_val) in a_flat {
        if !b_flat.contains_key(&path) {
            diff.added.insert(path, a_val);
        }
    }
    diff
}

/// Flatten a values tree into `dot.path -> leaf` entries. Arrays are
/// kept opaque — diffing positional changes inside arrays is rarely
/// useful for Helm values (most arrays of any size are lists of
/// structured items), and the leaf comparison catches total-replace
/// correctly.
fn flatten_values(value: &Value, prefix: &str, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.insert(prefix.to_string(), Value::Object(map.clone()));
                return;
            }
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_values(v, &path, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Schema diff
// ---------------------------------------------------------------------------

fn compare_schema(before: Option<&Value>, after: Option<&Value>) -> SchemaDiff {
    let b_fields = before.map(flatten_schema_fields).unwrap_or_default();
    let a_fields = after.map(flatten_schema_fields).unwrap_or_default();

    let b_keys: BTreeSet<_> = b_fields.keys().collect();
    let a_keys: BTreeSet<_> = a_fields.keys().collect();

    let mut diff = SchemaDiff::default();
    for path in a_keys.difference(&b_keys) {
        diff.fields_added
            .insert((*path).clone(), a_fields[*path].summary());
    }
    for path in b_keys.difference(&a_keys) {
        diff.fields_removed
            .insert((*path).clone(), b_fields[*path].summary());
    }
    for path in b_keys.intersection(&a_keys) {
        let before = &b_fields[*path];
        let after = &a_fields[*path];
        let changes = SchemaFieldChanges {
            type_changed: (before.type_ != after.type_).then(|| FieldChange {
                before: before.type_.clone().map(Value::String),
                after: after.type_.clone().map(Value::String),
            }),
            required_changed: (before.required != after.required).then(|| FieldChange {
                before: Some(Value::Bool(before.required)),
                after: Some(Value::Bool(after.required)),
            }),
            default_changed: (before.default != after.default).then(|| FieldChange {
                before: before.default.clone(),
                after: after.default.clone(),
            }),
            enum_changed: (before.enum_ != after.enum_).then(|| FieldChange {
                before: before.enum_.clone(),
                after: after.enum_.clone(),
            }),
            x_input_changed: (before.x_input != after.x_input).then(|| FieldChange {
                before: before.x_input.clone(),
                after: after.x_input.clone(),
            }),
        };
        if !changes.is_empty() {
            diff.fields_changed.insert((*path).clone(), changes);
        }
    }
    diff
}

/// Rich per-field snapshot used internally when diffing; surfaced to
/// the caller as the less-detailed [`SchemaField`].
#[derive(Debug, Clone, PartialEq)]
struct SchemaFieldDetail {
    type_: Option<String>,
    required: bool,
    default: Option<Value>,
    enum_: Option<Value>,
    x_input: Option<Value>,
}

impl SchemaFieldDetail {
    fn summary(&self) -> SchemaField {
        SchemaField {
            type_: self.type_.clone(),
            required: self.required,
            default: self.default.clone(),
        }
    }
}

/// Walk a JSON Schema object tree and produce leaf-field details
/// keyed by dot-notation path. Matches the convention in
/// [`crate::schema::extract_install_fields`].
fn flatten_schema_fields(schema: &Value) -> BTreeMap<String, SchemaFieldDetail> {
    let mut out = BTreeMap::new();
    walk_schema(schema, "", &[], &mut out);
    out
}

fn walk_schema(
    node: &Value,
    path: &str,
    parent_required: &[String],
    out: &mut BTreeMap<String, SchemaFieldDetail>,
) {
    let Some(obj) = node.as_object() else { return };
    let is_object = obj
        .get("type")
        .and_then(Value::as_str)
        .map(|t| t == "object")
        .unwrap_or(false);

    let required: Vec<String> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if is_object {
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (key, child) in props {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk_schema(child, &child_path, &required, out);
            }
        }
        return;
    }

    // Leaf or array — record it.
    let name = path
        .rsplit_once('.')
        .map(|(_, k)| k)
        .unwrap_or(path)
        .to_string();
    let is_required = parent_required.iter().any(|r| r == &name);

    out.insert(
        path.to_string(),
        SchemaFieldDetail {
            type_: obj
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string),
            required: is_required,
            default: obj.get("default").cloned(),
            enum_: obj.get("enum").cloned(),
            x_input: obj.get("x-input").cloned(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(chart_yaml: Value, values: Value, schema: Option<Value>) -> ChartSnapshot {
        ChartSnapshot {
            chart_yaml,
            values_yaml: values,
            values_schema: schema,
        }
    }

    #[test]
    fn identical_charts_produce_empty_diff() {
        let snap = snapshot(json!({"name": "x", "version": "1.0.0"}), json!({}), None);
        let diff = compare(&snap, &snap);
        assert!(diff.is_empty());
    }

    #[test]
    fn version_bump_surfaces_as_metadata_change() {
        let before = snapshot(
            json!({"name": "x", "version": "1.0.0", "appVersion": "1.0"}),
            json!({}),
            None,
        );
        let after = snapshot(
            json!({"name": "x", "version": "1.1.0", "appVersion": "1.1"}),
            json!({}),
            None,
        );
        let diff = compare(&before, &after);
        assert!(!diff.is_empty());
        let version = diff.metadata.changes.get("version").unwrap();
        assert_eq!(version.before, Some(json!("1.0.0")));
        assert_eq!(version.after, Some(json!("1.1.0")));
        assert!(diff.metadata.changes.contains_key("appVersion"));
    }

    #[test]
    fn added_and_removed_dependencies_partition_correctly() {
        let before = snapshot(
            json!({
                "name": "x",
                "dependencies": [
                    {"name": "old", "version": "1.0", "repository": "oci://r/a"},
                    {"name": "kept", "version": "1.0", "repository": "oci://r/b"}
                ]
            }),
            json!({}),
            None,
        );
        let after = snapshot(
            json!({
                "name": "x",
                "dependencies": [
                    {"name": "kept", "version": "1.0", "repository": "oci://r/b"},
                    {"name": "new", "version": "1.0", "repository": "oci://r/c"}
                ]
            }),
            json!({}),
            None,
        );
        let diff = compare(&before, &after);
        assert_eq!(diff.dependencies.added.len(), 1);
        assert_eq!(diff.dependencies.added[0].name, "new");
        assert_eq!(diff.dependencies.removed.len(), 1);
        assert_eq!(diff.dependencies.removed[0].name, "old");
        assert!(diff.dependencies.updated.is_empty());
    }

    #[test]
    fn dependency_version_change_surfaces_as_update() {
        let before = snapshot(
            json!({
                "name": "x",
                "dependencies": [{"name": "db", "version": "1.0", "repository": "r"}]
            }),
            json!({}),
            None,
        );
        let after = snapshot(
            json!({
                "name": "x",
                "dependencies": [{"name": "db", "version": "2.0", "repository": "r"}]
            }),
            json!({}),
            None,
        );
        let diff = compare(&before, &after);
        assert_eq!(diff.dependencies.updated.len(), 1);
        let u = &diff.dependencies.updated[0];
        assert_eq!(u.before.version, "1.0");
        assert_eq!(u.after.version, "2.0");
    }

    #[test]
    fn dependency_alias_is_the_dedup_key() {
        let before = snapshot(
            json!({
                "name": "x",
                "dependencies": [
                    {"name": "postgresql", "alias": "primary", "version": "15.0", "repository": "r"},
                    {"name": "postgresql", "alias": "replica", "version": "15.0", "repository": "r"}
                ]
            }),
            json!({}),
            None,
        );
        let after = snapshot(
            json!({
                "name": "x",
                "dependencies": [
                    {"name": "postgresql", "alias": "primary", "version": "15.1", "repository": "r"},
                    {"name": "postgresql", "alias": "replica", "version": "15.0", "repository": "r"}
                ]
            }),
            json!({}),
            None,
        );
        let diff = compare(&before, &after);
        assert_eq!(diff.dependencies.updated.len(), 1);
        assert_eq!(diff.dependencies.updated[0].key, "primary");
    }

    #[test]
    fn nested_value_change_surfaces_with_dot_path() {
        let before = snapshot(
            json!({"name": "x"}),
            json!({"service": {"port": 80, "type": "ClusterIP"}}),
            None,
        );
        let after = snapshot(
            json!({"name": "x"}),
            json!({"service": {"port": 8080, "type": "ClusterIP"}}),
            None,
        );
        let diff = compare(&before, &after);
        let change = diff.values.changed.get("service.port").unwrap();
        assert_eq!(change.before, Some(json!(80)));
        assert_eq!(change.after, Some(json!(8080)));
    }

    #[test]
    fn schema_field_removed_flipped_to_required() {
        let before_schema = json!({
            "type": "object",
            "properties": {
                "host": {"type": "string"},
                "port": {"type": "integer", "default": 8080}
            }
        });
        let after_schema = json!({
            "type": "object",
            "required": ["host"],
            "properties": {
                "host": {"type": "string"}
            }
        });
        let before = snapshot(json!({"name": "x"}), json!({}), Some(before_schema));
        let after = snapshot(json!({"name": "x"}), json!({}), Some(after_schema));
        let diff = compare(&before, &after);
        // port removed from the schema → fields_removed.
        assert!(diff.schema.fields_removed.contains_key("port"));
        // host flipped optional → required.
        let host_changes = diff.schema.fields_changed.get("host").unwrap();
        let req = host_changes.required_changed.as_ref().unwrap();
        assert_eq!(req.before, Some(Value::Bool(false)));
        assert_eq!(req.after, Some(Value::Bool(true)));
    }

    #[test]
    fn x_input_transform_rewrite_surfaces_as_schema_change() {
        let before = snapshot(
            json!({"name": "x"}),
            json!({}),
            Some(json!({
                "type": "object",
                "properties": {
                    "host": {
                        "type": "string",
                        "x-user-input": {"order": 10},
                        "x-input": {"cel": "value + '.old.example.com'"}
                    }
                }
            })),
        );
        let after = snapshot(
            json!({"name": "x"}),
            json!({}),
            Some(json!({
                "type": "object",
                "properties": {
                    "host": {
                        "type": "string",
                        "x-user-input": {"order": 10},
                        "x-input": {"cel": "slugify(value) + '.new.example.com'"}
                    }
                }
            })),
        );
        let diff = compare(&before, &after);
        let host_changes = diff.schema.fields_changed.get("host").unwrap();
        assert!(host_changes.x_input_changed.is_some());
    }
}
