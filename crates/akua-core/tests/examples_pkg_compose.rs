//! Integration test: render `examples/08-pkg-compose/` and verify
//! `pkg.render` sentinel expansion produces the expected resources.
//! Pure-KCL — no engine feature needed.

#![cfg(feature = "engine-kcl")]

use std::path::{Path, PathBuf};

use akua_core::PackageK;

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/08-pkg-compose")
}

#[test]
fn pkg_compose_renders_two_configmaps_from_shared_inner_package() {
    let dir = example_dir();
    let package = PackageK::load(&dir.join("package.k")).expect("load outer");
    let inputs = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("inputs.example.yaml")).expect("read inputs"),
    )
    .expect("parse inputs");

    let rendered = package.render(&inputs).expect("render");

    assert_eq!(
        rendered.resources.len(),
        2,
        "two pkg.render sentinels should expand to two ConfigMaps"
    );

    let names: Vec<String> = rendered
        .resources
        .iter()
        .filter_map(|r| {
            r["metadata"]["name"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect();
    assert!(
        names.contains(&"frontend".to_string()),
        "expected frontend ConfigMap: {names:?}"
    );
    assert!(
        names.contains(&"backend".to_string()),
        "expected backend ConfigMap: {names:?}"
    );

    // Verify per-component data plumbed through the sentinel round-trip.
    let frontend = rendered
        .resources
        .iter()
        .find(|r| r["metadata"]["name"] == serde_yaml::Value::String("frontend".into()))
        .expect("frontend");
    assert_eq!(
        frontend["data"]["PUBLIC_API_URL"],
        serde_yaml::Value::String("https://api.example.com".into())
    );

    let backend = rendered
        .resources
        .iter()
        .find(|r| r["metadata"]["name"] == serde_yaml::Value::String("backend".into()))
        .expect("backend");
    assert_eq!(
        backend["data"]["LOG_LEVEL"],
        serde_yaml::Value::String("info".into())
    );
}
