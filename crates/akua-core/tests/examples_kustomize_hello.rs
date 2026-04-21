//! End-to-end integration test: render `examples/09-kustomize-hello/`
//! via the `engine-kustomize-shell` engine and verify the overlay's
//! namePrefix + commonLabels land on the base ConfigMap.
//!
//! Ignored by default — requires `kustomize` on PATH. Run with:
//!
//!     cargo test -p akua-core --features engine-kustomize-shell \
//!         --test examples_kustomize_hello -- --ignored

#![cfg(all(feature = "engine-kcl", feature = "engine-kustomize-shell"))]

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
#[ignore = "requires `kustomize` on PATH; run with --ignored"]
fn renders_minimal_kustomize_package_end_to_end() {
    let dir = example_dir();
    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let rendered = package
        .render(&serde_yaml::Value::Null)
        .expect("render");

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
