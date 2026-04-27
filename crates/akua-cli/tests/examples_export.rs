//! End-to-end check: `akua export` against the canonical
//! `examples/01-hello-webapp/package.k` produces JSON Schema 2020-12
//! that matches the committed golden at
//! `examples/01-hello-webapp/exported/inputs.schema.json`. Catches
//! drift in either direction — schema-emit changes, or example-source
//! drift.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::contract::Context;
use akua_cli::verbs::export::{run, ExportArgs, ExportFormat};
use akua_core::cli_contract::ExitCode;

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/01-hello-webapp")
}

#[test]
fn export_matches_committed_golden() {
    let dir = example_dir();
    let package = dir.join("package.k");
    let golden = dir.join("exported/inputs.schema.json");

    let mut stdout = Vec::new();
    let code = run(
        &Context::human(),
        &ExportArgs {
            package_path: &package,
            format: ExportFormat::JsonSchema,
            out: None,
        },
        &mut stdout,
    )
    .expect("export");
    assert_eq!(code, ExitCode::Success);

    let actual: serde_json::Value =
        serde_json::from_slice(&stdout).expect("export output is valid JSON");
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden).expect("read golden"))
            .expect("golden is valid JSON");

    assert_eq!(
        actual,
        expected,
        "export drift vs {}\n\nactual:\n{}\n",
        golden.display(),
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn export_openapi_wraps_under_components_schemas() {
    let dir = example_dir();
    let package = dir.join("package.k");

    let mut stdout = Vec::new();
    run(
        &Context::human(),
        &ExportArgs {
            package_path: &package,
            format: ExportFormat::Openapi,
            out: None,
        },
        &mut stdout,
    )
    .expect("export");

    let doc: serde_json::Value =
        serde_json::from_slice(&stdout).expect("openapi output is valid JSON");
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["components"]["schemas"]["Input"].is_object());
}
