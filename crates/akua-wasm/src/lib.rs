//! # akua-wasm
//!
//! WASM bindings for `akua-core`. Built with `wasm-pack build
//! crates/akua-wasm --target bundler` to produce a `.wasm` module + TypeScript
//! definitions consumable from a browser.
//!
//! Exposes the pure-algorithm surface: schema field extraction, input
//! transforms, umbrella chart assembly, value merging. Does *not* include
//! Helm render (shells to a binary — not WASM-safe).

use std::collections::HashMap;

use akua_core::{
    apply_install_transforms as core_apply_install_transforms,
    build_umbrella_chart as core_build_umbrella_chart,
    extract_install_fields as core_extract_install_fields, hash_to_suffix as core_hash_to_suffix,
    merge_source_values as core_merge_source_values,
    merge_values_schemas as core_merge_values_schemas, schema::SourceWithSchema,
    validate_values_schema as core_validate_values_schema, ExtractedInstallField, Source,
};
use wasm_bindgen::prelude::*;

#[cfg(feature = "console_error_panic_hook")]
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsValue> {
    // Default serializer emits Map<K,V> as `Map` and serde_json::Map too. JS
    // consumers expect plain objects.
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    v.serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

fn from_js<T: serde::de::DeserializeOwned>(v: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(v).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Deterministic short alias suffix (djb2 + base36). Used for chart aliases.
#[wasm_bindgen(js_name = hashToSuffix)]
pub fn hash_to_suffix(input: &str, length: usize) -> String {
    core_hash_to_suffix(input, length)
}

/// Extract `x-user-input` / `x-install` fields from a JSON Schema.
#[wasm_bindgen(js_name = extractInstallFields)]
pub fn extract_install_fields(schema: JsValue) -> Result<JsValue, JsValue> {
    let schema: serde_json::Value = from_js(schema)?;
    let fields = core_extract_install_fields(&schema);
    to_js(&fields)
}

/// Apply schema transforms (slugify, template substitution) to user inputs.
///
/// `fields` is the output of `extractInstallFields`; `inputs` is an object
/// mapping dot-paths to string values. Returns resolved values nested by path.
#[wasm_bindgen(js_name = applyInstallTransforms)]
pub fn apply_install_transforms(fields: JsValue, inputs: JsValue) -> Result<JsValue, JsValue> {
    let fields: Vec<ExtractedInstallField> = from_js(fields)?;
    let inputs: HashMap<String, String> = from_js(inputs)?;
    let resolved = core_apply_install_transforms(&fields, &inputs)
        .map_err(|e| JsValue::from_str(&format!("apply_install_transforms: {e}")))?;
    to_js(&resolved)
}

/// Validate a values.schema.json structurally. Returns the error message,
/// or `null` if the schema is valid.
#[wasm_bindgen(js_name = validateValuesSchema)]
pub fn validate_values_schema(schema: JsValue) -> Result<Option<String>, JsValue> {
    let schema: serde_json::Value = from_js(schema)?;
    Ok(core_validate_values_schema(&schema))
}

/// Merge values from multiple sources into a single object, nested by alias.
#[wasm_bindgen(js_name = mergeSourceValues)]
pub fn merge_source_values(sources: JsValue) -> Result<JsValue, JsValue> {
    let sources: Vec<Source> = from_js(sources)?;
    let merged = core_merge_source_values(&sources);
    to_js(&merged)
}

/// Merge JSON Schemas from multiple sources into one umbrella schema.
///
/// Input: array of `{ source, schema? }`. Output: a single
/// `type: object` schema where each source's schema nests under its
/// deterministic alias (same alias the values use). Sources without a
/// schema are skipped. Used by the install wizard to show one combined
/// form for a multi-source package.
#[wasm_bindgen(js_name = mergeValuesSchemas)]
pub fn merge_values_schemas(sources: JsValue) -> Result<JsValue, JsValue> {
    let sources: Vec<SourceWithSchema> = from_js(sources)?;
    let merged = core_merge_values_schemas(&sources);
    to_js(&merged)
}

/// Build an umbrella Helm chart from a set of sources. Returns
/// `{ chartYaml, values }`.
#[wasm_bindgen(js_name = buildUmbrellaChart)]
pub fn build_umbrella_chart(
    name: &str,
    version: &str,
    sources: JsValue,
) -> Result<JsValue, JsValue> {
    let sources: Vec<Source> = from_js(sources)?;
    let umbrella = core_build_umbrella_chart(name, version, &sources)
        .map_err(|e| JsValue::from_str(&format!("buildUmbrellaChart: {e}")))?;
    to_js(&umbrella)
}
