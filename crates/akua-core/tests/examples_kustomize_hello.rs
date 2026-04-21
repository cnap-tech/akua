//! End-to-end integration test: render `examples/09-kustomize-hello/`
//! via the embedded `kustomize-engine-wasm` and verify the overlay's
//! namePrefix + labels land on the base ConfigMap.

#![cfg(all(feature = "engine-kcl", feature = "engine-kustomize"))]

use std::path::{Path, PathBuf};

use akua_core::PackageK;

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/09-kustomize-hello")
}

#[test]
fn renders_minimal_kustomize_package_end_to_end() {
    let dir = example_dir();
    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let rendered = match package.render(&serde_yaml::Value::Null) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("kustomize-engine.wasm not built") {
                eprintln!("skipping: {msg}");
                return;
            }
            panic!("render failed: {e}");
        }
    };

    assert_eq!(rendered.resources.len(), 1, "base has one resource");
    let cm = &rendered.resources[0];
    assert_eq!(cm["kind"], serde_yaml::Value::String("ConfigMap".into()));
    assert_eq!(
        cm["metadata"]["name"],
        serde_yaml::Value::String("prod-hello".into()),
        "overlay namePrefix applied"
    );
    assert_eq!(
        cm["metadata"]["labels"]["env"],
        serde_yaml::Value::String("prod".into()),
        "overlay commonLabel applied"
    );
    assert_eq!(
        cm["data"]["greeting"],
        serde_yaml::Value::String("hello from kustomize base".into())
    );
}
