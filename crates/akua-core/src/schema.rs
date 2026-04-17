//! JSON Schema (Draft 7) with Akua's `x-user-input` and `x-input` extensions.
//!
//! Ported from the TypeScript reference at `values_schema.ts`. Four exported
//! operations plus the [`JsonSchema`] newtype wrapper:
//!
//! - [`merge_values_schemas`] — combine schemas from multiple helm sources,
//!   honoring umbrella alias rules (nest Helm/OCI under alias keys, merge
//!   Git at root).
//! - [`extract_install_fields`] — walk a schema and collect all properties
//!   annotated with `x-user-input`, producing dot-notation paths.
//! - [`apply_install_transforms`] — validate + transform customer-provided
//!   values according to each field's `x-input` config (template + slugify +
//!   uniqueness hint; uniqueness check is left to the caller).
//! - [`validate_values_schema`] — structural validation with CNAP rules:
//!   root must be `type: object`; `x-user-input` only on leaves;
//!   `x-input.template` requires `{{value}}`.
//!
//! # Note on naming
//!
//! This module currently supports both the legacy CEP-0006 vocabulary
//! (`x-install`, `x-hostname`) and the generalized CEP-0008 names
//! (`x-user-input`, `x-input`). Legacy names are accepted as aliases for
//! one migration window, then removed.

use serde_json::{Map, Value};

use crate::source::{get_source_alias, HelmSource};
use crate::values::set_nested_value;

/// Thin wrapper around [`serde_json::Value`] to make schema APIs explicit.
pub type JsonSchema = Value;

/// A field extracted from a schema's `x-user-input` annotations.
///
/// `path` is a dot-notation path into the resolved values tree. `schema` is
/// the leaf's JSON Schema definition (type, title, description, default,
/// pattern, etc.). `hostname_template` is present when `x-input.template` or
/// legacy `x-hostname.template` is set.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedInstallField {
    pub path: String,
    pub schema: JsonSchema,
    pub required: bool,
    pub hostname_template: Option<String>,
    pub slugify: bool,
    pub unique_in: Option<String>,
    pub order: Option<i64>,
}

// ---------------------------------------------------------------------------
// Extension accessors
// ---------------------------------------------------------------------------

/// Returns `true` if the property is marked as customer-configurable.
/// Accepts both `x-user-input` (CEP-0008) and legacy `x-install` (CEP-0006).
fn has_user_input_marker(prop: &Value) -> bool {
    let marker = prop.get("x-user-input").or_else(|| prop.get("x-install"));
    match marker {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(_)) => true,
        Some(Value::Number(_)) => true, // legacy `{order: n}` normalization
        _ => false,
    }
}

/// Extract the order hint from `x-user-input: { order: N }` or
/// `x-install: { order: N }`. Returns `None` when the marker is just `true`.
fn get_install_order(prop: &Value) -> Option<i64> {
    let marker = prop.get("x-user-input").or_else(|| prop.get("x-install"))?;
    let obj = marker.as_object()?;
    obj.get("order")?.as_i64()
}

/// Extract the template string from `x-input.template` or legacy
/// `x-hostname.template`. Returns `None` if absent or malformed.
fn get_template(prop: &Value) -> Option<String> {
    let ext = prop.get("x-input").or_else(|| prop.get("x-hostname"))?;
    ext.get("template")?.as_str().map(String::from)
}

/// Extract the `slugify` flag. Legacy `x-hostname` implies `slugify: true`
/// (the CEP-0006 semantic); new `x-input` requires it to be opt-in.
fn get_slugify(prop: &Value) -> bool {
    if let Some(ext) = prop.get("x-input") {
        return ext
            .get("slugify")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    }
    if prop.get("x-hostname").is_some() {
        return true; // legacy semantic
    }
    false
}

/// Extract the `uniqueIn` registry name. Legacy `x-hostname` implies
/// `uniqueIn: "hostname"`; new `x-input` takes it as an explicit string.
fn get_unique_in(prop: &Value) -> Option<String> {
    if let Some(ext) = prop.get("x-input") {
        return ext.get("uniqueIn").and_then(|v| v.as_str()).map(String::from);
    }
    if prop.get("x-hostname").is_some() {
        return Some("hostname".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Alias computation for schema merge (mirrors values merge logic)
// ---------------------------------------------------------------------------

fn get_chart_name_from_source(source: &HelmSource) -> Option<String> {
    if let Some(c) = &source.chart.chart {
        if !c.is_empty() {
            return Some(c.clone());
        }
    }
    crate::source::extract_chart_name_from_oci(&source.chart.repo_url)
}

// ---------------------------------------------------------------------------
// merge_values_schemas
// ---------------------------------------------------------------------------

/// Combine schemas from multiple helm sources into one umbrella schema.
///
/// - Sources with no `valuesSchema` (or where the field is absent) are
///   skipped entirely.
/// - Single-source inputs return that source's schema unchanged.
/// - Multiple sources: Helm HTTP / OCI schemas nest under the source's alias
///   (or chart name if no ID); Git schemas merge their top-level properties
///   at the root; required arrays from Git sources concatenate.
///
/// The returned schema is always a `type: "object"` root.
pub fn merge_values_schemas(sources: &[SourceWithSchema]) -> JsonSchema {
    let with_schema: Vec<&SourceWithSchema> =
        sources.iter().filter(|s| s.schema.is_some()).collect();

    if with_schema.is_empty() {
        return Value::Object({
            let mut m = Map::new();
            m.insert("type".to_string(), Value::String("object".to_string()));
            m.insert("properties".to_string(), Value::Object(Map::new()));
            m
        });
    }

    if with_schema.len() == 1 {
        let s = with_schema[0];
        let schema = s.schema.clone().unwrap();
        let chart_name = get_chart_name_from_source(&s.source);
        if chart_name.is_none() {
            // Git source with single schema: return at root.
            return schema;
        }
        // Single Helm/OCI source: return as-is (no nesting needed).
        return schema;
    }

    // Multiple sources: merge.
    let mut merged_props: Map<String, Value> = Map::new();
    let mut merged_required: Vec<Value> = Vec::new();

    for s in &with_schema {
        let schema = s.schema.as_ref().unwrap();
        let chart_name = get_chart_name_from_source(&s.source);

        if let Some(chart_name) = chart_name {
            // Helm / OCI: nest under alias or chart name.
            let key = get_source_alias(&s.source).unwrap_or(chart_name);
            merged_props.insert(key, schema.clone());
        } else {
            // Git: merge properties at root.
            if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
                for (k, v) in props {
                    merged_props.insert(k.clone(), v.clone());
                }
            }
            if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
                for r in req {
                    merged_required.push(r.clone());
                }
            }
        }
    }

    let mut root = Map::new();
    root.insert("type".to_string(), Value::String("object".to_string()));
    root.insert("properties".to_string(), Value::Object(merged_props));
    if !merged_required.is_empty() {
        root.insert("required".to_string(), Value::Array(merged_required));
    }
    Value::Object(root)
}

/// Pairing of a helm source with its optional values schema, used by
/// [`merge_values_schemas`].
#[derive(Debug, Clone)]
pub struct SourceWithSchema {
    pub source: HelmSource,
    pub schema: Option<JsonSchema>,
}

// ---------------------------------------------------------------------------
// extract_install_fields
// ---------------------------------------------------------------------------

/// Walk a schema and extract all `x-user-input` fields with their dot paths.
///
/// Fields are returned sorted by `order` (ascending); fields without an
/// explicit order come last.
pub fn extract_install_fields(schema: &JsonSchema) -> Vec<ExtractedInstallField> {
    let mut out = Vec::new();
    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    walk_schema(schema, "", &required, &mut out);
    out.sort_by(|a, b| {
        let ao = a.order.unwrap_or(i64::MAX);
        let bo = b.order.unwrap_or(i64::MAX);
        ao.cmp(&bo)
    });
    out
}

fn walk_schema(
    schema: &Value,
    prefix: &str,
    parent_required: &[String],
    out: &mut Vec<ExtractedInstallField>,
) {
    let props = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return,
    };

    for (key, prop) in props {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        if has_user_input_marker(prop) {
            let order = get_install_order(prop);
            let hostname_template = get_template(prop);
            let slugify = get_slugify(prop);
            let unique_in = get_unique_in(prop);
            let required = parent_required.iter().any(|r| r == key);

            out.push(ExtractedInstallField {
                path: path.clone(),
                schema: prop.clone(),
                required,
                hostname_template,
                slugify,
                unique_in,
                order,
            });
        }

        // Recurse into nested objects (but not if this property is itself an install field).
        let is_object_with_children = prop.get("type").and_then(|v| v.as_str()) == Some("object")
            && prop
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|o| !o.is_empty())
                .unwrap_or(false);

        if is_object_with_children && !has_user_input_marker(prop) {
            let sub_required = prop
                .get("required")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            walk_schema(prop, &path, &sub_required, out);
        }
    }
}

// ---------------------------------------------------------------------------
// apply_install_transforms
// ---------------------------------------------------------------------------

/// Apply install-time transforms to user-provided string values.
///
/// For each field:
/// - Required but missing/empty → returns an error.
/// - Optional and empty → skipped.
/// - Has a template → slugify input (if requested) and substitute into
///   the template via `{{value}}` replacement.
/// - Otherwise → pass through (trimmed).
///
/// Uniqueness checks are **not** performed here — the caller handles those
/// (typically by querying CNAP's uniqueness registry for hostnames).
pub fn apply_install_transforms(
    fields: &[ExtractedInstallField],
    user_values: &std::collections::HashMap<String, String>,
) -> Result<Value, String> {
    let mut overrides = Value::Object(Map::new());

    for field in fields {
        let raw = user_values.get(&field.path).map(String::as_str).unwrap_or("");
        let trimmed = raw.trim();

        if field.required && trimmed.is_empty() {
            let label = field
                .schema
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&field.path);
            return Err(format!("Required field \"{label}\" is missing"));
        }

        if trimmed.is_empty() {
            continue;
        }

        let resolved = if let Some(template) = &field.hostname_template {
            let source_str = if field.slugify {
                slugify(trimmed)
            } else {
                trimmed.to_string()
            };
            template.replace("{{value}}", &source_str)
        } else {
            trimmed.to_string()
        };

        set_nested_value(&mut overrides, &field.path, Value::String(resolved));
    }

    Ok(overrides)
}

/// Slugify a string for use as a DNS label (RFC 1123): lowercase, replace
/// non-alphanumeric with hyphens, collapse consecutive hyphens, strip
/// leading/trailing hyphens, truncate to 63 chars.
pub fn slugify(input: &str) -> String {
    slugify_with_max_length(input, 63)
}

pub fn slugify_with_max_length(input: &str, max_length: usize) -> String {
    let lowered = input.to_lowercase();
    let hyphenated: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = collapse_hyphens(&hyphenated);
    let stripped = collapsed.trim_matches('-').to_string();
    stripped.chars().take(max_length).collect()
}

fn collapse_hyphens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                out.push('-');
            }
            prev_hyphen = true;
        } else {
            out.push(c);
            prev_hyphen = false;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// validate_values_schema
// ---------------------------------------------------------------------------

/// Validate a JSON Schema with Akua's CNAP extension rules.
///
/// Returns `None` if valid, `Some(error)` with a human-readable message if not.
///
/// Rules:
/// 1. Root must have `type: "object"`.
/// 2. `x-user-input` (or legacy `x-install`) must only appear on leaf
///    properties (not on objects that have their own nested `properties`).
/// 3. `x-input.template` (or legacy `x-hostname.template`) must exist and
///    contain `{{value}}`.
/// 4. `x-input` / `x-hostname` requires `x-user-input` / `x-install` on the
///    same property.
pub fn validate_values_schema(schema: &JsonSchema) -> Option<String> {
    if schema.get("type").and_then(|v| v.as_str()) != Some("object") {
        return Some("Values schema must have type: \"object\" at root level".to_string());
    }
    let empty_map = Map::new();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty_map);
    validate_properties(props)
}

fn validate_properties(props: &Map<String, Value>) -> Option<String> {
    for (key, prop) in props {
        let has_input_marker = has_user_input_marker(prop);
        let has_transform_ext =
            prop.get("x-input").is_some() || prop.get("x-hostname").is_some();

        let is_object_with_children = prop.get("type").and_then(|v| v.as_str()) == Some("object")
            && prop
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|o| !o.is_empty())
                .unwrap_or(false);

        if has_input_marker && is_object_with_children {
            return Some(format!(
                "Property \"{key}\": x-user-input must only be on leaf properties, not objects with nested properties"
            ));
        }

        if has_transform_ext && !has_input_marker {
            return Some(format!(
                "Property \"{key}\": x-input requires x-user-input to also be set"
            ));
        }

        if has_transform_ext {
            // Check template shape on whichever extension is present.
            let ext = prop.get("x-input").or_else(|| prop.get("x-hostname"));
            if let Some(ext) = ext {
                let template = ext.get("template").and_then(|v| v.as_str());
                match template {
                    Some(t) if t.is_empty() => {
                        return Some(format!(
                            "Property \"{key}\": x-input must have a \"template\" string"
                        ))
                    }
                    Some(t) if !t.contains("{{value}}") => {
                        return Some(format!(
                            "Property \"{key}\": x-input template must contain \"{{{{value}}}}\""
                        ))
                    }
                    Some(_) => {}
                    None => {
                        // legacy x-hostname without template -> error.
                        // new x-input without template is allowed if it only uses slugify/uniqueIn.
                        if prop.get("x-hostname").is_some() {
                            return Some(format!(
                                "Property \"{key}\": x-input must have a \"template\" string"
                            ));
                        }
                    }
                }
            }
        }

        if is_object_with_children {
            if let Some(sub) = prop.get("properties").and_then(|v| v.as_object()) {
                if let Some(e) = validate_properties(sub) {
                    return Some(e);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- extract_install_fields ---

    #[test]
    fn extract_with_x_user_input_true() {
        let schema = json!({
            "type": "object",
            "properties": {
                "appName": {"type": "string", "title": "App", "x-user-input": true}
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].path, "appName");
    }

    #[test]
    fn extract_legacy_x_install() {
        let schema = json!({
            "type": "object",
            "properties": {"foo": {"type": "string", "x-install": true}}
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].path, "foo");
    }

    #[test]
    fn extract_nested_with_dot_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "email": {"type": "string", "x-user-input": true}
                    }
                }
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].path, "config.email");
    }

    #[test]
    fn extract_legacy_x_hostname_template() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-install": true,
                    "x-hostname": {"template": "{{value}}.example.com"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].hostname_template,
            Some("{{value}}.example.com".to_string())
        );
        assert!(fields[0].slugify, "x-hostname implies slugify");
        assert_eq!(fields[0].unique_in.as_deref(), Some("hostname"));
    }

    #[test]
    fn extract_new_x_input_with_orthogonal_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {
                        "template": "{{value}}.example.com",
                        "slugify": true,
                        "uniqueIn": "hostname"
                    }
                }
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields[0].hostname_template.as_deref(), Some("{{value}}.example.com"));
        assert!(fields[0].slugify);
        assert_eq!(fields[0].unique_in.as_deref(), Some("hostname"));
    }

    #[test]
    fn extract_marks_required_from_parent() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string", "x-user-input": true},
                "opt": {"type": "string", "x-user-input": true}
            }
        });
        let fields = extract_install_fields(&schema);
        let by_path: std::collections::HashMap<_, _> =
            fields.into_iter().map(|f| (f.path.clone(), f)).collect();
        assert!(by_path["name"].required);
        assert!(!by_path["opt"].required);
    }

    #[test]
    fn extract_respects_order() {
        let schema = json!({
            "type": "object",
            "properties": {
                "second": {"type": "string", "x-user-input": {"order": 2}},
                "first": {"type": "string", "x-user-input": {"order": 1}},
                "third": {"type": "string", "x-user-input": true}
            }
        });
        let fields = extract_install_fields(&schema);
        let paths: Vec<_> = fields.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["first", "second", "third"]);
    }

    #[test]
    fn extract_ignores_false_marker() {
        let schema = json!({
            "type": "object",
            "properties": {
                "disabled": {"type": "string", "x-user-input": false},
                "enabled": {"type": "string", "x-user-input": true}
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].path, "enabled");
    }

    #[test]
    fn extract_returns_empty_when_no_properties() {
        let schema = json!({"type": "object"});
        assert_eq!(extract_install_fields(&schema), vec![]);
    }

    // --- apply_install_transforms ---

    fn inputs(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn transform_plain_string_passthrough() {
        let schema = json!({
            "type": "object",
            "properties": {"appName": {"type": "string", "x-user-input": true}}
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("appName", "My App")])).unwrap();
        assert_eq!(result, json!({"appName": "My App"}));
    }

    #[test]
    fn transform_hostname_slugifies_and_substitutes() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"template": "{{value}}.lando.health", "slugify": true}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("sub", "My Clinic")])).unwrap();
        assert_eq!(result, json!({"sub": "my-clinic.lando.health"}));
    }

    #[test]
    fn transform_missing_required_errors() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {"name": {"type": "string", "title": "Name", "x-user-input": true}}
        });
        let fields = extract_install_fields(&schema);
        let err = apply_install_transforms(&fields, &inputs(&[])).unwrap_err();
        assert!(err.contains("Name"));
    }

    #[test]
    fn transform_skips_optional_empty() {
        let schema = json!({
            "type": "object",
            "properties": {"opt": {"type": "string", "x-user-input": true}}
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[])).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn transform_empty_string_required_errors() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {"name": {"type": "string", "title": "Name", "x-user-input": true}}
        });
        let fields = extract_install_fields(&schema);
        let err = apply_install_transforms(&fields, &inputs(&[("name", "   ")])).unwrap_err();
        assert!(err.contains("Name"));
    }

    #[test]
    fn transform_nested_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {"email": {"type": "string", "x-user-input": true}}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("config.email", "admin@example.com")])).unwrap();
        assert_eq!(result, json!({"config": {"email": "admin@example.com"}}));
    }

    // --- slugify ---

    #[test]
    fn slugify_lowercases_and_hyphens_special() {
        assert_eq!(slugify("My App Name"), "my-app-name");
    }

    #[test]
    fn slugify_collapses_hyphens() {
        assert_eq!(slugify("my--app---name"), "my-app-name");
    }

    #[test]
    fn slugify_strips_ends() {
        assert_eq!(slugify("-my-app-"), "my-app");
    }

    #[test]
    fn slugify_truncates_to_63() {
        let long = "a".repeat(100);
        assert_eq!(slugify(&long).len(), 63);
    }

    #[test]
    fn slugify_custom_max_length() {
        assert_eq!(slugify_with_max_length("abcdefgh", 5), "abcde");
    }

    #[test]
    fn slugify_handles_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_handles_all_special() {
        assert_eq!(slugify("!@#$%^&*()"), "");
    }

    #[test]
    fn slugify_preserves_numbers() {
        assert_eq!(slugify("App 123 Test"), "app-123-test");
    }

    // --- validate_values_schema ---

    #[test]
    fn validate_rejects_non_object_root() {
        let s = json!({"type": "string"});
        assert!(validate_values_schema(&s).is_some());
    }

    #[test]
    fn validate_accepts_minimal_schema() {
        let s = json!({"type": "object", "properties": {}});
        assert!(validate_values_schema(&s).is_none());
    }

    #[test]
    fn validate_rejects_x_user_input_on_object_with_children() {
        let s = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "x-user-input": true,
                    "properties": {"nested": {"type": "string"}}
                }
            }
        });
        let err = validate_values_schema(&s).unwrap();
        assert!(err.contains("leaf"));
    }

    #[test]
    fn validate_rejects_x_hostname_without_x_install() {
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-hostname": {"template": "{{value}}.example.com"}
                }
            }
        });
        assert!(validate_values_schema(&s).is_some());
    }

    #[test]
    fn validate_rejects_x_hostname_missing_template() {
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {"type": "string", "x-install": true, "x-hostname": {}}
            }
        });
        assert!(validate_values_schema(&s).is_some());
    }

    #[test]
    fn validate_rejects_template_without_value_placeholder() {
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"template": "static.example.com"}
                }
            }
        });
        let err = validate_values_schema(&s).unwrap();
        assert!(err.contains("{{value}}"));
    }

    #[test]
    fn validate_accepts_x_input_with_only_slugify() {
        // New x-input without template is OK if it only sets slugify/uniqueIn.
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"slugify": true, "uniqueIn": "hostname"}
                }
            }
        });
        assert!(validate_values_schema(&s).is_none());
    }

    // --- merge_values_schemas ---

    fn make_source(
        id: Option<&str>,
        repo: &str,
        chart: Option<&str>,
        path: Option<&str>,
    ) -> HelmSource {
        HelmSource {
            id: id.map(String::from),
            chart: crate::source::ChartRef {
                repo_url: repo.to_string(),
                chart: chart.map(String::from),
                target_revision: "1.0.0".to_string(),
                path: path.map(String::from),
            },
            values: None,
        }
    }

    #[test]
    fn merge_single_source_returns_schema_as_is() {
        let schema = json!({
            "type": "object",
            "properties": {"replicas": {"type": "number"}}
        });
        let input = vec![SourceWithSchema {
            source: make_source(Some("id1"), "https://charts.example.com", Some("redis"), None),
            schema: Some(schema.clone()),
        }];
        assert_eq!(merge_values_schemas(&input), schema);
    }

    #[test]
    fn merge_nests_helm_oci_under_alias_keys() {
        let s1 = SourceWithSchema {
            source: make_source(Some("id_a"), "https://charts.example.com", Some("redis"), None),
            schema: Some(json!({"type": "object", "properties": {"port": {"type": "number"}}})),
        };
        let s2 = SourceWithSchema {
            source: make_source(Some("id_b"), "oci://ghcr.io/org/postgres", None, None),
            schema: Some(json!({"type": "object", "properties": {"mem": {"type": "string"}}})),
        };
        let merged = merge_values_schemas(&[s1, s2]);
        let props = merged.get("properties").unwrap().as_object().unwrap();
        assert_eq!(props.len(), 2);
        for key in props.keys() {
            assert!(key.starts_with("redis-") || key.starts_with("postgres-"));
        }
    }

    #[test]
    fn merge_puts_git_source_at_root() {
        let git = SourceWithSchema {
            source: make_source(None, "https://github.com/org/repo", None, Some("chart")),
            schema: Some(json!({"type": "object", "properties": {"global": {"type": "boolean"}}})),
        };
        let helm = SourceWithSchema {
            source: make_source(Some("id1"), "https://charts.example.com", Some("redis"), None),
            schema: Some(json!({"type": "object", "properties": {"replicas": {"type": "number"}}})),
        };
        let merged = merge_values_schemas(&[git, helm]);
        let props = merged.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("global"));
        let alias_keys: Vec<_> = props.keys().filter(|k| k.starts_with("redis-")).collect();
        assert_eq!(alias_keys.len(), 1);
    }

    #[test]
    fn merge_empty_when_no_sources_have_schema() {
        let s = SourceWithSchema {
            source: make_source(Some("id"), "https://charts.example.com", Some("redis"), None),
            schema: None,
        };
        let merged = merge_values_schemas(&[s]);
        assert_eq!(merged, json!({"type": "object", "properties": {}}));
    }
}
