//! Emit the single JSON Schema bundle for the whole akua CLI contract.
//!
//! Lives in `akua-cli` (not `akua-core`) because the bundle needs to
//! register types from both crates — akua-cli depends on akua-core,
//! so this is the only place that sees the full surface.
//!
//! ts-rs (feature = "ts-export") emits per-type `.ts` files because
//! TS imports are per-file. schemars emits ONE document with every
//! type in `$defs` — the standard JSON Schema bundle shape, matching
//! how `schemas/v1/akua.json` will ship in the signed release artifact.
//!
//! Trigger: `cargo test -p akua-cli --features schema-export --test export_sdk_bundle`.

#![cfg(feature = "schema-export")]

use std::path::Path;

use akua_cli::verbs::version::VersionOutput;
use akua_core::cli_contract::StructuredError;
use akua_core::cli_contract::error::Level;
use schemars::generate::SchemaSettings;

#[test]
fn export_json_schema_bundle() {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    // Register every top-level type that appears in `--json` output.
    // Nested types (e.g. `StructuredError.level: Level`) are discovered
    // during walk and land in `$defs` automatically — listing them here
    // is only for top-level discoverability by agents.
    generator.subschema_for::<StructuredError>();
    generator.subschema_for::<Level>();
    generator.subschema_for::<VersionOutput>();

    let defs = generator.take_definitions(true);
    let bundle = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://akua.dev/schemas/v1/akua.json",
        "title": "akua CLI contract",
        "description": "Machine-readable shape of every type akua emits under `--json`. \
                        Bind any validator (ajv, Zod, pydantic, gojsonschema, ...) against this.",
        "$defs": defs,
    });

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk-schemas/akua.json");
    std::fs::create_dir_all(out.parent().expect("has parent")).expect("create sdk-schemas");
    let json = serde_json::to_string_pretty(&bundle).expect("bundle -> json");
    std::fs::write(&out, json).expect("write bundle");
}
