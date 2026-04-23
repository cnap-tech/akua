//! Convert a chart's `values.schema.json` (JSON Schema) into a
//! typed KCL `schema Values` declaration.
//!
//! Helm charts ship with an optional `values.schema.json` at the
//! chart root, used by `helm install --validate` to reject bad
//! `values.yaml` inputs. When present, we generate a KCL mirror so
//! Package authors get the same shape under their IDE / LSP:
//!
//! ```kcl
//! import charts.nginx as nginx
//!
//! _values = nginx.Values {
//!     replicaCount = 3
//!     image = nginx.ValuesImage { tag = "1.27" }
//! }
//! helm.template(helm.Template { chart = nginx.path, values = _values })
//! ```
//!
//! ## Scope
//!
//! Phase 2b slice C gets the common shapes: objects, primitives,
//! arrays, enums. Deferred:
//!
//! - `$ref` (needs a two-pass resolver)
//! - `allOf` / `oneOf` / `anyOf` (no clean KCL mapping short of
//!   generated union types)
//! - `pattern` / `format` validation (would need KCL `check:` blocks)
//! - `additionalProperties: false` (KCL is strict-by-default anyway)
//!
//! Unknown shapes collapse to `any` — the author can override field
//! by field if the generated schema isn't tight enough.

use serde::Deserialize;

/// Input JSON Schema — we model only the subset we handle. Other
/// keywords (`pattern`, `format`, `allOf`, …) are silently ignored
/// on a best-effort basis; stricter validation lives upstream in
/// helm's own `--validate`.
#[derive(Debug, Deserialize)]
struct JsonSchema {
    #[serde(default, rename = "type")]
    ty: Option<TypeSpec>,

    #[serde(default)]
    properties: std::collections::BTreeMap<String, JsonSchema>,

    #[serde(default)]
    required: Vec<String>,

    #[serde(default)]
    items: Option<Box<JsonSchema>>,

    #[serde(default)]
    default: Option<serde_json::Value>,

    #[serde(default, rename = "enum")]
    enum_values: Option<Vec<serde_json::Value>>,

    /// Trailing docstring; surfaced as KCL field doc.
    #[serde(default)]
    description: Option<String>,
}

/// `type:` in JSON Schema can be a string or an array (union).
/// We handle the string form directly; array form collapses to the
/// first non-null type (helm charts use `["string", "null"]` to
/// express optionality, which KCL represents as a non-required
/// field instead).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TypeSpec {
    Single(String),
    Union(Vec<String>),
}

impl TypeSpec {
    fn primary(&self) -> Option<&str> {
        match self {
            TypeSpec::Single(s) => Some(s.as_str()),
            TypeSpec::Union(v) => v.iter().map(String::as_str).find(|t| *t != "null"),
        }
    }
}

/// Generated KCL source. The caller writes it to the chart's
/// per-render module next to `path` / `sha256`.
#[derive(Debug, Clone, Default)]
pub struct GeneratedKcl {
    /// Top-level schema declarations, in dependency order. The root
    /// schema is always named `Values`; nested object schemas are
    /// named `Values<Path>` (e.g. `ValuesImage`, `ValuesImageTag`).
    pub source: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ValuesSchemaError {
    #[error("values.schema.json not valid JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Convert `values.schema.json` bytes to a KCL `schema Values` block
/// plus any nested supporting schemas. Root schema is always named
/// `Values`; callers prefix with `<ChartName>Values` themselves if
/// they want a namespaced shape.
///
/// Returns an empty [`GeneratedKcl`] when the input schema is not
/// an object — helm's `values.yaml` is always a dict, so a non-
/// object root schema is a defect in the chart; we surface it as
/// "no typed schema generated" rather than erroring.
pub fn generate_from_bytes(bytes: &[u8]) -> Result<GeneratedKcl, ValuesSchemaError> {
    let schema: JsonSchema = serde_json::from_slice(bytes)?;
    Ok(generate(&schema))
}

fn generate(root: &JsonSchema) -> GeneratedKcl {
    let primary = root.ty.as_ref().and_then(TypeSpec::primary).unwrap_or("");
    if primary != "object" {
        return GeneratedKcl::default();
    }
    let mut gen = SchemaGen::default();
    gen.emit_object(root, "Values");
    GeneratedKcl { source: gen.finish() }
}

#[derive(Default)]
struct SchemaGen {
    /// Schemas produced so far, in emit order. Nested objects append
    /// after the parent so a forward-referencing root schema is OK
    /// as long as KCL's parser does full-module resolution (it does).
    out: Vec<String>,
}

impl SchemaGen {
    fn emit_object(&mut self, schema: &JsonSchema, name: &str) {
        let required: std::collections::HashSet<&str> =
            schema.required.iter().map(String::as_str).collect();

        let mut body = String::new();
        body.push_str(&format!("schema {name}:\n"));
        if let Some(doc) = schema.description.as_deref() {
            body.push_str(&format_docstring(doc, 4));
            body.push('\n');
        }

        if schema.properties.is_empty() {
            // No declared fields — KCL requires at least one statement
            // in a schema body. Emit a passthrough wildcard dict
            // field; callers can still construct an empty `{}`.
            body.push_str("    _: any = None\n\n");
            self.out.push(body);
            return;
        }

        for (prop_name, prop_schema) in &schema.properties {
            self.emit_field(&mut body, name, prop_name, prop_schema, &required);
        }
        body.push('\n');
        self.out.push(body);
    }

    fn emit_field(
        &mut self,
        out: &mut String,
        parent_name: &str,
        prop_name: &str,
        prop_schema: &JsonSchema,
        required: &std::collections::HashSet<&str>,
    ) {
        let is_required = required.contains(prop_name);
        let nested_name = format!("{parent_name}{}", pascal_case(prop_name));
        let ty = self.render_type(&nested_name, prop_schema);

        let default = default_literal(prop_schema);
        let opt_marker = if is_required || default.is_some() { "" } else { "?" };
        let assignment = default.map(|d| format!(" = {d}")).unwrap_or_default();

        out.push_str(&format!(
            "    {prop_name}{opt_marker}: {ty}{assignment}\n"
        ));

        if let Some(desc) = prop_schema.description.as_deref() {
            out.push_str(&format_docstring(desc, 8));
            out.push('\n');
        }
    }

    /// Decide the KCL type for a field. Nested objects emit a new
    /// schema and return its name; primitives return the built-in
    /// type; arrays recurse on the element type.
    fn render_type(&mut self, nested_name: &str, schema: &JsonSchema) -> String {
        let primary = schema
            .ty
            .as_ref()
            .and_then(TypeSpec::primary)
            .unwrap_or("");
        match primary {
            "object" => {
                // Nested object — emit a support schema.
                self.emit_object(schema, nested_name);
                nested_name.to_string()
            }
            "array" => {
                let item_ty = match schema.items.as_deref() {
                    Some(inner) => {
                        let inner_name = format!("{nested_name}Item");
                        self.render_type(&inner_name, inner)
                    }
                    None => "any".to_string(),
                };
                format!("[{item_ty}]")
            }
            "string" => "str".to_string(),
            "integer" => "int".to_string(),
            "number" => "float".to_string(),
            "boolean" => "bool".to_string(),
            "null" => "any".to_string(),
            _ => "any".to_string(),
        }
    }

    fn finish(self) -> String {
        self.out.join("\n")
    }
}

/// Render a JSON default value as a KCL literal. Returns `None`
/// for shapes we can't render (arbitrary nested dicts, etc.) —
/// the caller falls back to an unpopulated optional field.
fn default_literal(schema: &JsonSchema) -> Option<String> {
    let v = schema.default.as_ref()?;
    json_value_to_kcl(v)
}

fn json_value_to_kcl(v: &serde_json::Value) -> Option<String> {
    Some(match v {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => kcl_string_literal(s),
        serde_json::Value::Array(items) => {
            let parts: Option<Vec<String>> = items.iter().map(json_value_to_kcl).collect();
            format!("[{}]", parts?.join(", "))
        }
        serde_json::Value::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (k, val) in map {
                let rendered = json_value_to_kcl(val)?;
                entries.push(format!("{}: {rendered}", kcl_string_literal(k)));
            }
            format!("{{{}}}", entries.join(", "))
        }
    })
}

/// Format as a KCL string literal — quote + escape `\` and `"`.
fn kcl_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Indent a docstring block at `indent` spaces. KCL docstrings use
/// triple-quoted strings on the line below the field.
fn format_docstring(text: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let trimmed = text.trim();
    // Single-line doc → single-line docstring, multi-line → block.
    if !trimmed.contains('\n') {
        format!("{pad}\"\"\"{trimmed}\"\"\"")
    } else {
        let body = trimmed
            .lines()
            .map(|l| format!("{pad}{l}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{pad}\"\"\"\n{body}\n{pad}\"\"\"")
    }
}

/// `replicaCount` / `image_pull_policy` → `ReplicaCount` / `ImagePullPolicy`.
/// Used to name nested schemas off property names.
fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
            continue;
        }
        if capitalize_next {
            out.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_object_root_produces_empty_output() {
        let input = br#"{ "type": "string" }"#;
        let out = generate_from_bytes(input).unwrap();
        assert_eq!(out.source, "");
    }

    #[test]
    fn primitive_fields_render() {
        let input = br#"{
            "type": "object",
            "properties": {
                "replicaCount": { "type": "integer", "default": 1 },
                "name":         { "type": "string",  "default": "hello" },
                "debug":        { "type": "boolean", "default": false },
                "ratio":        { "type": "number",  "default": 0.5 }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("replicaCount: int = 1"), "{}", out.source);
        assert!(out.source.contains("name: str = \"hello\""), "{}", out.source);
        assert!(out.source.contains("debug: bool = False"), "{}", out.source);
        assert!(out.source.contains("ratio: float = 0.5"), "{}", out.source);
        assert!(out.source.starts_with("schema Values:"));
    }

    #[test]
    fn required_fields_have_no_question_mark() {
        let input = br#"{
            "type": "object",
            "properties": {
                "host":    { "type": "string" },
                "replicas":{ "type": "integer", "default": 2 }
            },
            "required": ["host"]
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("host: str"), "{}", out.source);
        // Required fields don't get the `?`; neither do fields with
        // a default (they resolve without input).
        assert!(!out.source.contains("host?:"), "{}", out.source);
        assert!(!out.source.contains("replicas?:"), "{}", out.source);
    }

    #[test]
    fn optional_without_default_has_question_mark() {
        let input = br#"{
            "type": "object",
            "properties": {
                "note": { "type": "string" }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("note?: str"), "{}", out.source);
    }

    #[test]
    fn nested_object_generates_support_schema() {
        let input = br#"{
            "type": "object",
            "properties": {
                "image": {
                    "type": "object",
                    "properties": {
                        "repository": { "type": "string" },
                        "tag":        { "type": "string", "default": "latest" }
                    },
                    "required": ["repository"]
                }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("image?: ValuesImage"), "{}", out.source);
        assert!(out.source.contains("schema ValuesImage:"), "{}", out.source);
        assert!(out.source.contains("repository: str"), "{}", out.source);
        // `tag` is optional in JSON Schema but carries a default, so
        // the KCL field resolves without input — no `?` needed.
        assert!(
            out.source.contains("tag: str = \"latest\""),
            "{}",
            out.source
        );
    }

    #[test]
    fn arrays_render_with_item_type() {
        let input = br#"{
            "type": "object",
            "properties": {
                "hosts": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("hosts?: [str]"), "{}", out.source);
    }

    #[test]
    fn array_without_items_is_any() {
        let input = br#"{
            "type": "object",
            "properties": {
                "stuff": { "type": "array" }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("stuff?: [any]"), "{}", out.source);
    }

    #[test]
    fn nullable_type_union_uses_primary() {
        let input = br#"{
            "type": "object",
            "properties": {
                "maybe": { "type": ["string", "null"] }
            }
        }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("maybe?: str"), "{}", out.source);
    }

    #[test]
    fn pascal_case_handles_snake_and_kebab() {
        assert_eq!(pascal_case("replicaCount"), "ReplicaCount");
        assert_eq!(pascal_case("image_pull_policy"), "ImagePullPolicy");
        assert_eq!(pascal_case("node-selector"), "NodeSelector");
    }

    #[test]
    fn kcl_string_literal_escapes() {
        assert_eq!(kcl_string_literal("plain"), r#""plain""#);
        assert_eq!(kcl_string_literal(r#"a"b"#), r#""a\"b""#);
        assert_eq!(kcl_string_literal(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn empty_object_schema_gets_passthrough_field() {
        // Empty objects are legal JSON Schema but KCL schemas need a
        // body. Ensure we emit the placeholder field without crashing.
        let input = br#"{ "type": "object", "properties": {} }"#;
        let out = generate_from_bytes(input).unwrap();
        assert!(out.source.contains("schema Values:"), "{}", out.source);
        assert!(out.source.contains("_: any = None"), "{}", out.source);
    }

    #[test]
    fn malformed_json_surfaces_parse_error() {
        let input = b"not json {{{";
        let err = generate_from_bytes(input).unwrap_err();
        assert!(matches!(err, ValuesSchemaError::Parse(_)));
    }
}
