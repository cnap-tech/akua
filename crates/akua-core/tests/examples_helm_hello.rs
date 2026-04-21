//! End-to-end integration test: render `examples/00-helm-hello/` via
//! the `engine-helm-shell` engine and verify the ConfigMap the chart
//! template produces.
//!
//! Ignored by default — requires `helm` on PATH. Run with:
//!
//!     cargo test -p akua-core --features engine-helm-shell \
//!         --test examples_helm_hello -- --ignored

#![cfg(all(feature = "engine-kcl", feature = "engine-helm-shell"))]

use std::path::{Path, PathBuf};

use akua_core::PackageK;

fn example_dir() -> PathBuf {
    // crates/akua-core → workspace root → examples/00-helm-hello
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/00-helm-hello")
}

#[test]
#[ignore = "requires `helm` on PATH; run with --ignored"]
fn renders_minimal_helm_package_end_to_end() {
    // No cwd juggling: `PackageK::render` pushes the package onto
    // the plugin render stack, so `helm.template("./chart")` resolves
    // against the package's own directory.
    let dir = example_dir();
    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let inputs = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("inputs.example.yaml")).expect("read inputs"),
    )
    .expect("parse inputs");

    let rendered = package.render(&inputs).expect("render");
    assert_eq!(rendered.resources.len(), 1, "chart has one template");

    let cm = &rendered.resources[0];
    assert_eq!(cm["kind"], serde_yaml::Value::String("ConfigMap".into()));
    assert_eq!(
        cm["metadata"]["name"],
        serde_yaml::Value::String("hello-greeting".into())
    );
    assert_eq!(
        cm["data"]["greeting"],
        serde_yaml::Value::String("hello from the example".into())
    );
    assert_eq!(
        cm["data"]["replicas"],
        serde_yaml::Value::String("3".into())
    );
}
