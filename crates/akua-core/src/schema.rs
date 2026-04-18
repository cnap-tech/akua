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

use crate::source::{get_source_alias, Source};
use crate::values::set_nested_value;

/// Thin wrapper around [`serde_json::Value`] to make schema APIs explicit.
pub type JsonSchema = Value;

/// A field extracted from a schema walk.
///
/// Minimal, universal: just the leaf's dot-path, its raw JSON Schema node
/// (including any `x-*` extensions), and a denormalised `required` flag.
///
/// Akua-opinionated extensions (`x-input.cel`, `x-input.uniqueIn`,
/// `x-user-input.order`, …) are NOT first-class fields on this struct —
/// read them via the accessor methods, or directly from `schema` as raw
/// JSON. See [`spec-markers.md`](../../docs/spec-markers.md) for the
/// canonical marker vocabulary.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedInstallField {
    /// Dot-notation path into the resolved values tree.
    pub path: String,
    /// Raw JSON Schema node for this leaf, including any `x-*` extensions.
    pub schema: JsonSchema,
    /// Denormalised cache: true iff the leaf's key is in its parent's
    /// `required: [...]` array at extraction time. Convenience for
    /// flat-list consumers. Re-derivable from the raw schema tree.
    pub required: bool,
}

impl ExtractedInstallField {
    /// CEL expression from `x-input.cel`, if any. Akua's reference
    /// transform language. Third-party bundle assemblers that use a
    /// different transform language ignore this and read their own key.
    pub fn cel(&self) -> Option<&str> {
        self.schema.get("x-input")?.get("cel")?.as_str()
    }

    /// Uniqueness-registry hint from `x-input.uniqueIn`, if any.
    pub fn unique_in(&self) -> Option<&str> {
        self.schema.get("x-input")?.get("uniqueIn")?.as_str()
    }

    /// UI ordering hint from `x-user-input.order`, if any.
    pub fn order(&self) -> Option<i64> {
        self.schema
            .get("x-user-input")?
            .as_object()?
            .get("order")?
            .as_i64()
    }

    /// Standard JSON Schema `title`, if any.
    pub fn title(&self) -> Option<&str> {
        self.schema.get("title")?.as_str()
    }
}

// ---------------------------------------------------------------------------
// Extension accessors
// ---------------------------------------------------------------------------

/// Returns `true` if the property is marked as customer-configurable.
fn has_user_input_marker(prop: &Value) -> bool {
    match prop.get("x-user-input") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// merge_values_schemas
// ---------------------------------------------------------------------------

/// Combine schemas from multiple package sources into one umbrella schema.
///
/// - Sources with no `valuesSchema` (or where the field is absent) are
///   skipped entirely.
/// - Single-source inputs return that source's schema unchanged.
/// - Multiple sources: each schema nests under the source's alias.
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
        return with_schema[0].schema.clone().unwrap();
    }

    let mut merged_props: Map<String, Value> = Map::new();
    for s in &with_schema {
        let schema = s.schema.as_ref().unwrap();
        let key = get_source_alias(&s.source).unwrap_or_else(|| s.source.name.clone());
        merged_props.insert(key, schema.clone());
    }

    let mut root = Map::new();
    root.insert("type".to_string(), Value::String("object".to_string()));
    root.insert("properties".to_string(), Value::Object(merged_props));
    Value::Object(root)
}

/// Pairing of a package source with its optional values schema, used by
/// [`merge_values_schemas`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceWithSchema {
    pub source: Source,
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
        let ao = a.order().unwrap_or(i64::MAX);
        let bo = b.order().unwrap_or(i64::MAX);
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
            let required = parent_required.iter().any(|r| r == key);
            out.push(ExtractedInstallField {
                path: path.clone(),
                schema: prop.clone(),
                required,
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
/// For each field (in `order`):
/// - Required but missing/empty → error.
/// - Optional and empty → skipped.
/// - `cel` set → evaluate CEL with `value` (trimmed raw input) and `values`
///   (resolved-so-far) in scope. Result must be a string.
/// - `cel` unset → trimmed raw input passes through unchanged.
///
/// CEL environment includes standard functions plus Akua registered:
/// `slugify(s)` (RFC 1123 DNS label, max 63 chars) and `slugifyMax(s, n)`
/// (same with custom max length).
///
/// Uniqueness checks are **not** performed here — `unique_in` is surfaced
/// to the caller as a hint; registry integration is a platform concern.
pub fn apply_install_transforms(
    fields: &[ExtractedInstallField],
    user_values: &std::collections::HashMap<String, String>,
) -> Result<Value, String> {
    let mut overrides = Value::Object(Map::new());

    for field in fields {
        let raw = user_values
            .get(&field.path)
            .map(String::as_str)
            .unwrap_or("");
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

        let resolved = match field.cel() {
            Some(expr) => eval_cel(expr, trimmed, &overrides, &field.path)?,
            None => trimmed.to_string(),
        };

        set_nested_value(&mut overrides, &field.path, Value::String(resolved));
    }

    Ok(overrides)
}

/// Evaluate a CEL expression with `value` (trimmed raw input) and `values`
/// (the resolved-so-far object) bound in scope. `slugify` / `slugifyMax`
/// are registered as CEL custom functions.
fn eval_cel(
    expression: &str,
    value: &str,
    values_so_far: &Value,
    path: &str,
) -> Result<String, String> {
    use cel_interpreter::{Context, Program, Value as CelValue};

    let program = Program::compile(expression)
        .map_err(|e| format!("field `{path}`: invalid CEL expression: {e}"))?;

    let mut ctx = Context::default();
    ctx.add_function(
        "slugify",
        |s: std::sync::Arc<String>| -> std::sync::Arc<String> { std::sync::Arc::new(slugify(&s)) },
    );
    ctx.add_function(
        "slugifyMax",
        |s: std::sync::Arc<String>, max: i64| -> std::sync::Arc<String> {
            let n = if max < 0 { 0 } else { max as usize };
            std::sync::Arc::new(slugify_with_max_length(&s, n))
        },
    );
    ctx.add_variable_from_value("value", value.to_string());
    ctx.add_variable("values", values_so_far.clone())
        .map_err(|e| format!("field `{path}`: binding `values`: {e}"))?;

    match program.execute(&ctx) {
        Ok(CelValue::String(s)) => Ok(s.to_string()),
        Ok(other) => Err(format!(
            "field `{path}`: CEL expression must return a string, got {other:?}"
        )),
        Err(e) => Err(format!("field `{path}`: CEL evaluation failed: {e}")),
    }
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

/// Validate a JSON Schema against Akua's structural rules.
///
/// Returns `None` if valid, `Some(error)` with a human-readable message if not.
///
/// Rules (see `docs/spec-markers.md`):
/// 1. Root must have `type: "object"`.
/// 2. `x-user-input` must only appear on leaf properties — not on objects
///    with their own nested `properties`.
/// 3. `x-input.cel`, if present, must be a non-empty string.
///
/// `x-input` does NOT require `x-user-input` — derived fields (computed
/// from other values, not shown to users) are a valid combination. See
/// the four-combinations matrix in the marker spec.
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

        if let Some(cel) = prop.get("x-input").and_then(|x| x.get("cel")) {
            match cel.as_str() {
                Some("") => {
                    return Some(format!(
                        "Property \"{key}\": x-input.cel must be a non-empty string"
                    ))
                }
                Some(_) => {}
                None => return Some(format!("Property \"{key}\": x-input.cel must be a string")),
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
    fn legacy_x_install_no_longer_recognised() {
        // x-install was the CEP-0006 vocabulary — deprecated in v1alpha1.
        // Only x-user-input is recognised now.
        let schema = json!({
            "type": "object",
            "properties": {"foo": {"type": "string", "x-install": true}}
        });
        let fields = extract_install_fields(&schema);
        assert!(fields.is_empty(), "x-install should not produce a field");
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
    fn extract_reads_cel_and_unique_in_via_accessors() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": { "order": 10 },
                    "x-input": {
                        "cel": "slugify(value) + '.example.com'",
                        "uniqueIn": "hostname"
                    }
                }
            }
        });
        let fields = extract_install_fields(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].cel(), Some("slugify(value) + '.example.com'"));
        assert_eq!(fields[0].unique_in(), Some("hostname"));
        assert_eq!(fields[0].order(), Some(10));
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
    fn transform_cel_with_slugify_function() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": { "cel": "slugify(value) + '.lando.health'" }
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("sub", "My Clinic")])).unwrap();
        assert_eq!(result, json!({"sub": "my-clinic.lando.health"}));
    }

    #[test]
    fn transform_cel_expression_passes_value_through() {
        let schema = json!({
            "type": "object",
            "properties": {
                "email": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"cel": "value"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result =
            apply_install_transforms(&fields, &inputs(&[("email", "admin@example.com")])).unwrap();
        assert_eq!(result, json!({"email": "admin@example.com"}));
    }

    #[test]
    fn transform_cel_expression_composes_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "env": {
                    "type": "string",
                    "x-user-input": {"order": 1}
                },
                "subdomain": {
                    "type": "string",
                    "x-user-input": {"order": 2},
                    "x-input": {"cel": "value + '.' + values.env + '.apps.example.com'"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(
            &fields,
            &inputs(&[("env", "staging"), ("subdomain", "acme")]),
        )
        .unwrap();
        assert_eq!(
            result,
            json!({"env": "staging", "subdomain": "acme.staging.apps.example.com"})
        );
    }

    #[test]
    fn transform_cel_slugify_then_append() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": { "cel": "slugify(value) + '-prod'" }
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("name", "My App!")])).unwrap();
        assert_eq!(result, json!({"name": "my-app-prod"}));
    }

    #[test]
    fn transform_cel_takes_precedence_over_template() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"template": "{{value}}.OLD", "cel": "value + '.NEW'"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let result = apply_install_transforms(&fields, &inputs(&[("x", "foo")])).unwrap();
        assert_eq!(result, json!({"x": "foo.NEW"}));
    }

    #[test]
    fn transform_cel_invalid_expression_errors() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"cel": "this is not valid CEL @#$"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let err = apply_install_transforms(&fields, &inputs(&[("x", "foo")])).unwrap_err();
        assert!(err.contains("invalid CEL"));
    }

    #[test]
    fn transform_cel_non_string_result_errors() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"cel": "42"}
                }
            }
        });
        let fields = extract_install_fields(&schema);
        let err = apply_install_transforms(&fields, &inputs(&[("x", "foo")])).unwrap_err();
        assert!(err.contains("must return a string"));
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
        let result =
            apply_install_transforms(&fields, &inputs(&[("config.email", "admin@example.com")]))
                .unwrap();
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
    fn validate_rejects_empty_cel_string() {
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"cel": ""}
                }
            }
        });
        let err = validate_values_schema(&s).unwrap();
        assert!(err.contains("cel"));
    }

    #[test]
    fn validate_rejects_non_string_cel() {
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"cel": 42}
                }
            }
        });
        let err = validate_values_schema(&s).unwrap();
        assert!(err.contains("cel"));
    }

    #[test]
    fn validate_accepts_derived_field_without_x_user_input() {
        // x-input without x-user-input IS allowed — derived fields (computed
        // from other inputs, not shown to users) are a valid combination.
        // See the four-combinations matrix in spec-markers.md.
        let s = json!({
            "type": "object",
            "properties": {
                "derived": {
                    "type": "string",
                    "x-input": {"cel": "values.env + '.apps.example.com'"}
                }
            }
        });
        assert!(validate_values_schema(&s).is_none());
    }

    #[test]
    fn validate_accepts_x_input_with_only_unique_in() {
        // x-input without x-input.cel is fine — only uniqueIn is set.
        let s = json!({
            "type": "object",
            "properties": {
                "sub": {
                    "type": "string",
                    "x-user-input": true,
                    "x-input": {"uniqueIn": "hostname"}
                }
            }
        });
        assert!(validate_values_schema(&s).is_none());
    }

    // --- merge_values_schemas ---

    fn make_source(name: &str, repo: &str, chart: Option<&str>) -> Source {
        Source {
            name: name.to_string(),
            helm: Some(crate::source::HelmBlock {
                repo: repo.to_string(),
                chart: chart.map(String::from),
                version: "1.0.0".to_string(),
            }),
            kcl: None,
            helmfile: None,
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
            source: make_source("id1", "https://charts.example.com", Some("redis")),
            schema: Some(schema.clone()),
        }];
        assert_eq!(merge_values_schemas(&input), schema);
    }

    #[test]
    fn merge_nests_helm_oci_under_alias_keys() {
        let s1 = SourceWithSchema {
            source: make_source("cache", "https://charts.example.com", Some("redis")),
            schema: Some(json!({"type": "object", "properties": {"port": {"type": "number"}}})),
        };
        let s2 = SourceWithSchema {
            source: make_source("db", "oci://ghcr.io/org/postgres", None),
            schema: Some(json!({"type": "object", "properties": {"mem": {"type": "string"}}})),
        };
        let merged = merge_values_schemas(&[s1, s2]);
        let props = merged.get("properties").unwrap().as_object().unwrap();
        assert_eq!(props.len(), 2);
        assert!(props.contains_key("cache"));
        assert!(props.contains_key("db"));
    }

    #[test]
    fn merge_empty_when_no_sources_have_schema() {
        let s = SourceWithSchema {
            source: make_source("id", "https://charts.example.com", Some("redis")),
            schema: None,
        };
        let merged = merge_values_schemas(&[s]);
        assert_eq!(merged, json!({"type": "object", "properties": {}}));
    }
}
