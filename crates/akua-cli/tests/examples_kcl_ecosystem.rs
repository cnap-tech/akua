//! End-to-end render of `examples/10-kcl-ecosystem/` — proves the
//! KCL OCI fetch + ExternalPkg wiring by composing a typed Deployment
//! against the upstream `kcl-lang/k8s` schema bundle.
//!
//! The example's akua.toml lists `oci://ghcr.io/kcl-lang/k8s@1.31.2`
//! with a `replace = { path = "./vendor/k8s" }` override so this test
//! runs offline. Drop the replace + run `akua render` to exercise the
//! live OCI path.
//!
//! Skips cleanly if the render-worker module hasn't been built yet.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/10-kcl-ecosystem")
}

#[test]
fn renders_kcl_ecosystem_dep_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    assert!(
        manifest.dependencies.contains_key("k8s"),
        "example 10 must declare the k8s dep"
    );

    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve k8s pkg");
    let k8s = resolved.entries.get("k8s").expect("k8s entry");
    assert_eq!(
        k8s.kind,
        chart_resolver::PackageKind::KclModule,
        "vendor tree has kcl.mod → resolver must label it KclModule"
    );
    assert!(k8s.abs_path.is_absolute());
    assert!(k8s.abs_path.ends_with("vendor/k8s"));
    // Marker file confirms the resolver pointed us at the package root,
    // not the parent vendor/ dir.
    assert!(k8s.abs_path.join("kcl.mod").is_file());

    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let inputs = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("inputs.example.yaml")).expect("read inputs.example.yaml"),
    )
    .expect("parse inputs");

    let rendered = match render_in_worker(&package, &inputs, &resolved, false) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("worker module wasn't compiled") {
                eprintln!("skipping: {msg}");
                return;
            }
            panic!("render failed: {e}");
        }
    };

    assert_eq!(
        rendered.resources.len(),
        1,
        "package emits exactly one Deployment"
    );

    let golden = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("rendered/000-deployment-hello.yaml")).expect("read golden"),
    )
    .expect("parse golden");
    assert_eq!(
        rendered.resources[0], golden,
        "rendered Deployment drifted from rendered/000-deployment-hello.yaml"
    );
}
