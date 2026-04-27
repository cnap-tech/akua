//! `akua export` — convert a Package's `Input` schema to a standard
//! interchange format. Implementation backs the verb; UI form
//! renderers (rjsf, JSONForms), API doc generators (Swagger UI,
//! Redoc), client SDK toolchains, and admission-webhook schema
//! validators all consume the output as-is.
//!
//! Two output formats today:
//! - **JSON Schema 2020-12** — raw schema for the Package's `Input`.
//! - **OpenAPI 3.1** — the same schema, wrapped in
//!   `components.schemas.Input`. OpenAPI 3.1 *is* JSON Schema
//!   2020-12 at the body level, so the wrapper is the only delta.
//!
//! ## How it works
//!
//! Two KCL APIs feed the export. `get_schema_type_mapping` resolves
//! the Package and returns a `KclType` per top-level schema (types,
//! defaults, required[]). `parse_program` returns the raw AST, which
//! we walk to recover field-level docstrings and `@ui(...)`
//! decorators — neither of which the resolver propagates onto
//! `KclType` (the resolver rejects unknown decorators silently and
//! attribute docstrings live in a sibling node, not on the attr).
//!
//! ## `@ui(...)` decorators
//!
//! KCL accepts `@ui(...)` syntax on schema attributes (the parser
//! permits any decorator name). The resolver flags `ui` as an
//! unknown decorator but doesn't fail compilation. We pull the
//! decorator's keyword args straight from the AST and fold them
//! into an `x-ui` object on the corresponding JSON Schema property —
//! pure pass-through, OpenAPI-3.1-compliant `x-` extension.
//! Renderers that understand `x-ui` use it; renderers that don't,
//! ignore it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::package_k::PackageKError;

/// Schema name akua expects every Package to declare for its public
/// inputs. Matches the convention documented in package-format.md §3.
const INPUT_SCHEMA_NAME: &str = "Input";

/// `@ui` decorator keyword args land here on the JSON Schema property,
/// per OpenAPI's `x-` extension convention. Renderers that recognise
/// the prefix consume; others ignore.
const X_UI_EXTENSION: &str = "x-ui";

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("kcl introspection failed: {0}")]
    Kcl(String),

    /// The Package source has no schema named `Input`. akua export
    /// only operates against the public inputs surface.
    #[error("no `Input` schema declared in `{filename}` — `akua export` operates on the public inputs surface (see docs/package-format.md §3)")]
    NoInputSchema { filename: String },

    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl From<ExportError> for PackageKError {
    fn from(e: ExportError) -> Self {
        PackageKError::KclEval(e.to_string())
    }
}

/// AST-derived metadata for a single schema attribute that the KCL
/// resolver doesn't surface on `KclType`.
#[derive(Default, Debug)]
struct AttrAst {
    description: Option<String>,
    x_ui: Option<Value>,
}

/// AST-derived metadata for a single schema, keyed by attribute name
/// plus the schema's own docstring.
#[derive(Default, Debug)]
struct SchemaAst {
    doc: Option<String>,
    attrs: BTreeMap<String, AttrAst>,
}

type SchemaAstMap = BTreeMap<String, SchemaAst>;

/// Emit JSON Schema 2020-12 for the `Input` schema in `source`.
/// Filename is used for KCL diagnostic rendering only; KCL doesn't
/// touch it on disk when sources are provided in-memory.
pub fn export_input_schema(filename: &str, source: &str) -> Result<Value, ExportError> {
    let mapping = get_schema_mapping(filename, source)?;
    let ast = parse_schemas(filename, source)?;
    let input = mapping
        .get(INPUT_SCHEMA_NAME)
        .ok_or_else(|| ExportError::NoInputSchema {
            filename: filename.to_string(),
        })?;
    let mut schema = kcl_type_to_json_schema(input, INPUT_SCHEMA_NAME, &ast);
    if let Some(obj) = schema.as_object_mut() {
        obj.insert(
            "$schema".to_string(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        obj.entry("title".to_string())
            .or_insert_with(|| json!(INPUT_SCHEMA_NAME));
    }
    Ok(schema)
}

/// Emit OpenAPI 3.1 wrapping the `Input` schema. The body is JSON
/// Schema 2020-12 (OpenAPI 3.1 unified its schema dialect with JSON
/// Schema). `components.schemas.Input` carries the actual schema;
/// callers reference it as `#/components/schemas/Input`.
pub fn export_input_openapi(filename: &str, source: &str) -> Result<Value, ExportError> {
    let mut schema = export_input_schema(filename, source)?;
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
    }
    Ok(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Package input contract",
            "version": "1.0.0",
        },
        "jsonSchemaDialect": "https://json-schema.org/draft/2020-12/schema",
        "components": {
            "schemas": {
                INPUT_SCHEMA_NAME: schema,
            }
        }
    }))
}

fn get_schema_mapping(
    filename: &str,
    source: &str,
) -> Result<BTreeMap<String, kcl_lang::KclType>, ExportError> {
    use kcl_lang::{ExecProgramArgs, GetSchemaTypeMappingArgs, API};

    let api = API::default();
    let exec_args = ExecProgramArgs {
        k_filename_list: vec![filename.to_string()],
        k_code_list: vec![source.to_string()],
        external_pkgs: crate::package_k::akua_external_pkgs(),
        compile_only: true,
        ..Default::default()
    };
    let args = GetSchemaTypeMappingArgs {
        exec_args: Some(exec_args),
        schema_name: INPUT_SCHEMA_NAME.to_string(),
    };
    let result = api
        .get_schema_type_mapping(&args)
        .map_err(|e| ExportError::Kcl(e.to_string()))?;
    Ok(result.schema_type_mapping.into_iter().collect())
}

/// Parse the Package source and walk every top-level schema to
/// recover docstrings + `@ui` decorators that the resolver drops.
fn parse_schemas(filename: &str, source: &str) -> Result<SchemaAstMap, ExportError> {
    use kcl_lang::{ParseProgramArgs, API};

    let api = API::default();
    let result = api
        .parse_program(&ParseProgramArgs {
            paths: vec![filename.to_string()],
            sources: vec![source.to_string()],
            ..Default::default()
        })
        .map_err(|e| ExportError::Kcl(e.to_string()))?;
    let ast: Value = serde_json::from_str(&result.ast_json)?;
    Ok(extract_schema_ast(&ast))
}

/// Walk the parsed-program JSON and pull out per-schema metadata.
/// AST shape (only the bits we read):
///
/// ```text
/// pkgs.__main__[].nodes[].node.{
///     type: "Schema",
///     doc: NodeRef<String>?,
///     name: NodeRef<String>,
///     body: [NodeRef<Stmt>]
/// }
/// ```
///
/// Inside `body` we look for `SchemaAttr` nodes (carrying decorators)
/// followed by an `Expr` wrapping a `StringLit` — KCL parses the
/// `"""docstring"""` as a sibling `Expr` after the attr it documents.
fn extract_schema_ast(ast: &Value) -> SchemaAstMap {
    let mut out = SchemaAstMap::new();
    let modules = ast
        .pointer("/pkgs/__main__")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for module in modules {
        let nodes = module
            .pointer("/body")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for stmt in nodes {
            let inner = stmt.pointer("/node");
            if inner
                .and_then(|v| v.pointer("/type"))
                .and_then(Value::as_str)
                != Some("Schema")
            {
                continue;
            }
            let Some(node) = inner else { continue };
            let name = node
                .pointer("/name/node")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let mut info = SchemaAst {
                doc: node
                    .pointer("/doc/node")
                    .and_then(Value::as_str)
                    .map(unquote_docstring)
                    .filter(|s| !s.is_empty()),
                ..Default::default()
            };
            let body = node
                .pointer("/body")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            // Pair each SchemaAttr with the immediately-following
            // StringLit Expr (KCL's per-attribute docstring shape).
            for (i, body_stmt) in body.iter().enumerate() {
                let body_node = body_stmt.pointer("/node");
                if body_node
                    .and_then(|v| v.pointer("/type"))
                    .and_then(Value::as_str)
                    != Some("SchemaAttr")
                {
                    continue;
                }
                let Some(body_node) = body_node else { continue };
                let attr_name = body_node
                    .pointer("/name/node")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if attr_name.is_empty() {
                    continue;
                }
                let mut entry = AttrAst::default();
                if let Some(decorators) =
                    body_node.pointer("/decorators").and_then(|v| v.as_array())
                {
                    entry.x_ui = decorators_to_x_ui(decorators);
                }
                if let Some(next) = body.get(i + 1) {
                    if let Some(doc) = string_lit_expr(next.pointer("/node")) {
                        entry.description = Some(doc);
                    }
                }
                info.attrs.insert(attr_name, entry);
            }
            out.insert(name, info);
        }
    }
    out
}

/// Strip a leading/trailing triple-quote (or single-quote) wrapper.
/// Schema-level `doc.node` carries the raw source token (e.g.
/// `"""…"""`); attribute-level docstrings on `StringLit.value` are
/// already unwrapped, so this only applies at the schema level.
fn unquote_docstring(raw: &str) -> String {
    let trimmed = raw.trim();
    for q in ["\"\"\"", "'''", "\"", "'"] {
        if trimmed.starts_with(q) && trimmed.ends_with(q) && trimmed.len() >= 2 * q.len() {
            return trimmed[q.len()..trimmed.len() - q.len()].trim().to_string();
        }
    }
    trimmed.to_string()
}

/// If `node` is an `Expr` wrapping a single `StringLit`, return the
/// string value. KCL renders attribute docstrings as a free-floating
/// string expression in the schema body.
fn string_lit_expr(node: Option<&Value>) -> Option<String> {
    let node = node?;
    if node.pointer("/type")?.as_str()? != "Expr" {
        return None;
    }
    let exprs = node.pointer("/exprs")?.as_array()?;
    let first = exprs.first()?.pointer("/node")?;
    if first.pointer("/type")?.as_str()? != "StringLit" {
        return None;
    }
    Some(first.pointer("/value")?.as_str()?.to_string())
}

/// Pick the `@ui(...)` decorator from an AST decorator list and
/// project its keyword args into a JSON object suitable for `x-ui`.
/// Each decorator is a CallExpr at `[i].node.{func, args, keywords}`.
fn decorators_to_x_ui(decorators: &[Value]) -> Option<Value> {
    let ui = decorators.iter().find(|d| {
        d.pointer("/node/func/node/names/0/node")
            .and_then(Value::as_str)
            == Some("ui")
    })?;
    let mut obj = serde_json::Map::new();
    if let Some(keywords) = ui.pointer("/node/keywords").and_then(|v| v.as_array()) {
        for kw in keywords {
            let Some(name) = kw
                .pointer("/node/arg/node/names/0/node")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(value) = kw.pointer("/node/value/node") else {
                continue;
            };
            if let Some(v) = literal_value(value) {
                obj.insert(name.to_string(), v);
            }
        }
    }
    if obj.is_empty() {
        None
    } else {
        Some(Value::Object(obj))
    }
}

/// Convert a literal AST node (NumberLit / StringLit / NameConstantLit
/// / ListExpr-of-literals) to a JSON value. Non-literal expressions
/// are dropped — `@ui(...)` is a UI hint, not a computation.
fn literal_value(node: &Value) -> Option<Value> {
    match node.pointer("/type")?.as_str()? {
        "StringLit" => Some(json!(node.pointer("/value")?.as_str()?)),
        "NumberLit" => {
            let v = node.pointer("/value")?;
            match v.pointer("/type")?.as_str()? {
                "Int" => Some(json!(v.pointer("/value")?.as_i64()?)),
                "Float" => Some(json!(v.pointer("/value")?.as_f64()?)),
                _ => None,
            }
        }
        "NameConstantLit" => match node.pointer("/value")?.as_str()? {
            "True" => Some(json!(true)),
            "False" => Some(json!(false)),
            "None" => Some(Value::Null),
            _ => None,
        },
        "List" => {
            let items: Vec<Value> = node
                .pointer("/elts")?
                .as_array()?
                .iter()
                .filter_map(|e| literal_value(e.pointer("/node")?))
                .collect();
            Some(Value::Array(items))
        }
        _ => None,
    }
}

/// Split a KCL literal-type string like `str(apps/v1)` / `int(42)` /
/// `bool(True)` into `(base, literal)`. Returns `None` if `s` isn't
/// in `<base>(<literal>)` shape.
fn parse_literal_type(s: &str) -> Option<(&str, &str)> {
    let open = s.find('(')?;
    if !s.ends_with(')') {
        return None;
    }
    Some((&s[..open], &s[open + 1..s.len() - 1]))
}

/// Recursively map a `KclType` to a JSON Schema 2020-12 property.
/// `attr_name` is the parent property name (used to look up
/// AST-derived decorators + docstrings); pass `INPUT_SCHEMA_NAME`
/// for the root schema, the attribute name for nested calls.
fn kcl_type_to_json_schema(t: &kcl_lang::KclType, attr_name: &str, ast: &SchemaAstMap) -> Value {
    let mut out = serde_json::Map::new();

    match t.r#type.as_str() {
        "schema" => {
            out.insert("type".to_string(), json!("object"));
            let schema_ast = ast.get(&t.schema_name).or_else(|| ast.get(attr_name));
            if let Some(doc) = schema_ast.and_then(|s| s.doc.as_ref()) {
                out.insert("description".to_string(), json!(doc));
            }
            if !t.properties.is_empty() {
                let props: serde_json::Map<String, Value> = t
                    .properties
                    .iter()
                    .map(|(name, prop_ty)| {
                        (name.clone(), kcl_type_to_json_schema(prop_ty, name, ast))
                    })
                    .collect();
                // Apply AST-derived per-property metadata.
                let props = if let Some(s) = schema_ast {
                    let mut props = props;
                    for (name, prop) in props.iter_mut() {
                        let Some(attr) = s.attrs.get(name) else {
                            continue;
                        };
                        if let Some(obj) = prop.as_object_mut() {
                            if let Some(desc) = &attr.description {
                                obj.entry("description".to_string())
                                    .or_insert_with(|| json!(desc));
                            }
                            if let Some(x_ui) = &attr.x_ui {
                                obj.insert(X_UI_EXTENSION.to_string(), x_ui.clone());
                            }
                        }
                    }
                    props
                } else {
                    props
                };
                out.insert("properties".to_string(), Value::Object(props));
            }
            if !t.required.is_empty() {
                out.insert(
                    "required".to_string(),
                    Value::Array(t.required.iter().map(|s| json!(s)).collect()),
                );
            }
        }
        "str" => {
            out.insert("type".to_string(), json!("string"));
        }
        "int" => {
            out.insert("type".to_string(), json!("integer"));
        }
        "float" | "number" | "number_multiplier" => {
            out.insert("type".to_string(), json!("number"));
        }
        "bool" => {
            out.insert("type".to_string(), json!("boolean"));
        }
        "list" => {
            out.insert("type".to_string(), json!("array"));
            if let Some(item) = &t.item {
                out.insert(
                    "items".to_string(),
                    kcl_type_to_json_schema(item, attr_name, ast),
                );
            }
        }
        "dict" => {
            out.insert("type".to_string(), json!("object"));
            if let Some(val) = &t.item {
                out.insert(
                    "additionalProperties".to_string(),
                    kcl_type_to_json_schema(val, attr_name, ast),
                );
            }
        }
        "union" => {
            let variants: Vec<Value> = t
                .union_types
                .iter()
                .map(|v| kcl_type_to_json_schema(v, attr_name, ast))
                .collect();
            if !variants.is_empty() {
                out.insert("oneOf".to_string(), Value::Array(variants));
            }
        }
        "any" => {}
        // Literal types: KCL surfaces these as `str(apps/v1)`,
        // `int(42)`, `bool(True)` — base type, then the literal in
        // parens. Map to JSON Schema `const`.
        other => {
            if let Some((base, lit)) = parse_literal_type(other) {
                match base {
                    "int" => {
                        out.insert("type".to_string(), json!("integer"));
                        if let Ok(n) = lit.parse::<i64>() {
                            out.insert("const".to_string(), json!(n));
                        }
                    }
                    "float" | "number" => {
                        out.insert("type".to_string(), json!("number"));
                        if let Ok(n) = lit.parse::<f64>() {
                            out.insert("const".to_string(), json!(n));
                        }
                    }
                    "bool" => {
                        out.insert("type".to_string(), json!("boolean"));
                        out.insert("const".to_string(), json!(lit.eq_ignore_ascii_case("true")));
                    }
                    "str" => {
                        out.insert("type".to_string(), json!("string"));
                        out.insert("const".to_string(), json!(lit));
                    }
                    _ => {
                        out.insert("x-akua-kcl-type".to_string(), json!(t.r#type));
                    }
                }
            } else {
                out.insert("x-akua-kcl-type".to_string(), json!(t.r#type));
            }
        }
    }

    if !t.default.is_empty() {
        // KCL stores defaults as their literal-source representation.
        // Try JSON first (numbers / bools / arrays / objects); fall
        // back to the raw string.
        let default =
            serde_json::from_str::<Value>(&t.default).unwrap_or_else(|_| json!(t.default));
        out.insert("default".to_string(), default);
    }

    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(source: &str) -> Value {
        export_input_schema("package.k", source).expect("export")
    }

    #[test]
    fn maps_primitive_types() {
        let schema = export(
            r#"
schema Input:
    name: str
    replicas: int
    weight: float
    enabled: bool

resources = []
"#,
        );
        let props = &schema["properties"];
        assert_eq!(props["name"]["type"], json!("string"));
        assert_eq!(props["replicas"]["type"], json!("integer"));
        assert_eq!(props["weight"]["type"], json!("number"));
        assert_eq!(props["enabled"]["type"], json!("boolean"));
    }

    #[test]
    fn surfaces_required_optional_and_default() {
        let schema = export(
            r#"
schema Input:
    name: str
    replicas: int = 3
    label?: str

resources = []
"#,
        );
        let req = schema["required"].as_array().unwrap();
        let req: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"name"), "required={req:?}");
        assert!(
            !req.contains(&"label"),
            "label is optional, required={req:?}"
        );
        assert_eq!(schema["properties"]["replicas"]["default"], json!(3));
    }

    #[test]
    fn surfaces_docstrings_as_description() {
        let schema = export(
            r#"
schema Input:
    """Public inputs for this package."""

    name: str
    """Application name."""

    replicas: int = 2

resources = []
"#,
        );
        assert_eq!(
            schema["description"],
            json!("Public inputs for this package.")
        );
        assert_eq!(
            schema["properties"]["name"]["description"],
            json!("Application name.")
        );
    }

    #[test]
    fn maps_list_and_dict() {
        let schema = export(
            r#"
schema Input:
    tags: [str]
    labels: {str:str}
    counts: [int]

resources = []
"#,
        );
        assert_eq!(schema["properties"]["tags"]["type"], json!("array"));
        assert_eq!(
            schema["properties"]["tags"]["items"]["type"],
            json!("string")
        );
        assert_eq!(schema["properties"]["labels"]["type"], json!("object"));
        assert_eq!(
            schema["properties"]["labels"]["additionalProperties"]["type"],
            json!("string")
        );
        assert_eq!(
            schema["properties"]["counts"]["items"]["type"],
            json!("integer")
        );
    }

    #[test]
    fn projects_at_ui_decorator_keywords_to_x_ui() {
        let schema = export(
            r#"
schema Input:
    @ui(order=10, group="Identity")
    name: str

    @ui(order=20, widget="slider", min=1, max=20)
    replicas: int = 3

resources = []
"#,
        );
        let name_x_ui = &schema["properties"]["name"]["x-ui"];
        assert_eq!(name_x_ui["order"], json!(10));
        assert_eq!(name_x_ui["group"], json!("Identity"));

        let replicas_x_ui = &schema["properties"]["replicas"]["x-ui"];
        assert_eq!(replicas_x_ui["widget"], json!("slider"));
        assert_eq!(replicas_x_ui["min"], json!(1));
        assert_eq!(replicas_x_ui["max"], json!(20));
    }

    #[test]
    fn openapi_wraps_input_under_components_schemas() {
        let doc = export_input_openapi(
            "package.k",
            r#"
schema Input:
    name: str

resources = []
"#,
        )
        .expect("export openapi");
        assert_eq!(doc["openapi"], json!("3.1.0"));
        assert_eq!(
            doc["components"]["schemas"]["Input"]["properties"]["name"]["type"],
            json!("string")
        );
        // Body should not carry its own $schema — OpenAPI declares it
        // at the document level via `jsonSchemaDialect`.
        assert!(doc["components"]["schemas"]["Input"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn missing_input_schema_surfaces_typed_error() {
        let err = export_input_schema(
            "package.k",
            r#"
schema NotTheInput:
    name: str

resources = []
"#,
        )
        .unwrap_err();
        assert!(matches!(err, ExportError::NoInputSchema { .. }));
    }

    #[test]
    fn maps_union_types_to_one_of() {
        let schema = export(
            r#"
schema Input:
    value: str | int

resources = []
"#,
        );
        let variants = schema["properties"]["value"]["oneOf"]
            .as_array()
            .expect("oneOf array");
        let kinds: Vec<&str> = variants.iter().filter_map(|v| v["type"].as_str()).collect();
        assert!(kinds.contains(&"string"), "kinds={kinds:?}");
        assert!(kinds.contains(&"integer"), "kinds={kinds:?}");
    }

    #[test]
    fn maps_literal_string_int_bool_to_const() {
        let schema = export(
            r#"
schema Input:
    api: "apps/v1"
    answer: 42
    flag: True

resources = []
"#,
        );
        let api = &schema["properties"]["api"];
        assert_eq!(api["type"], json!("string"));
        assert_eq!(api["const"], json!("apps/v1"));

        let answer = &schema["properties"]["answer"];
        assert_eq!(answer["type"], json!("integer"));
        assert_eq!(answer["const"], json!(42));

        let flag = &schema["properties"]["flag"];
        assert_eq!(flag["type"], json!("boolean"));
        assert_eq!(flag["const"], json!(true));
    }

    #[test]
    fn any_type_emits_unconstrained_property() {
        let schema = export(
            r#"
schema Input:
    blob: any

resources = []
"#,
        );
        let blob = &schema["properties"]["blob"];
        // `any` produces an empty object schema (no `type` constraint).
        assert!(blob.get("type").is_none(), "got: {blob}");
    }

    #[test]
    fn nested_schema_property_is_object_with_nested_properties() {
        let schema = export(
            r#"
schema Address:
    city: str
    zip: int

schema Input:
    name: str
    address: Address

resources = []
"#,
        );
        let address = &schema["properties"]["address"];
        assert_eq!(address["type"], json!("object"));
        assert_eq!(
            address["properties"]["city"]["type"],
            json!("string"),
            "address={address}"
        );
        assert_eq!(address["properties"]["zip"]["type"], json!("integer"));
    }

    #[test]
    fn ui_decorator_supports_bool_float_list_kwargs() {
        let schema = export(
            r#"
schema Input:
    @ui(required=True, step=0.5, choices=["a", "b"])
    name: str = "hello"

resources = []
"#,
        );
        let x_ui = &schema["properties"]["name"]["x-ui"];
        assert_eq!(x_ui["required"], json!(true));
        // KCL prints `0.5` as a NumberLit Float.
        assert_eq!(x_ui["step"], json!(0.5));
        assert_eq!(x_ui["choices"], json!(["a", "b"]));
    }

    #[test]
    fn unquote_docstring_strips_supported_quote_forms() {
        assert_eq!(unquote_docstring(r#""""hi""""#), "hi");
        assert_eq!(unquote_docstring("'''hi'''"), "hi");
        assert_eq!(unquote_docstring(r#""hi""#), "hi");
        assert_eq!(unquote_docstring("'hi'"), "hi");
        // Surrounding whitespace is trimmed both before and after the
        // quotes so multi-line schema docstrings collapse cleanly.
        assert_eq!(unquote_docstring("\n  \"\"\"  hi  \"\"\"  \n"), "hi");
        // Unquoted input passes through untouched.
        assert_eq!(unquote_docstring("plain"), "plain");
    }
}
