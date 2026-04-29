//! End-to-end integration test: render `examples/01-hello-webapp/` —
//! a Package that imports a typed `charts.nginx` dep resolved from
//! `akua.toml` (Phase 2a local-path form) and feeds the resolved
//! chart path into `helm.template`.
//!
//! Runs through the wasmtime render sandbox (Phase 4), not the
//! old in-process `PackageK::render_with_charts`. Requires
//! `crates/helm-engine-wasm/assets/helm-engine.wasm`; skips cleanly
//! when the WASM artifact hasn't been built yet.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/01-hello-webapp")
}

#[test]
fn renders_hello_webapp_with_resolved_chart_dep() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    assert!(
        manifest.dependencies.contains_key("nginx"),
        "example 01 must declare nginx dep"
    );
    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve charts");
    let nginx = resolved.entries.get("nginx").expect("nginx resolved");
    assert!(nginx.abs_path.is_absolute());
    assert!(nginx.abs_path.ends_with("vendor/nginx"));
    assert!(nginx.sha256.starts_with("sha256:"));

    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let inputs = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("inputs.yaml")).expect("read inputs"),
    )
    .expect("parse inputs");

    let rendered = match render_in_worker(
        &package,
        &inputs,
        &resolved,
        false,
        akua_core::kcl_plugin::BudgetSnapshot::default(),
    ) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("helm-engine.wasm not built")
                || msg.contains("worker module wasn't compiled")
            {
                eprintln!("skipping: {msg}");
                return;
            }
            panic!("render failed: {e}");
        }
    };

    // The vendored chart emits a Deployment + Service. Look up each by
    // kind instead of index so template-order changes don't break the
    // test.
    let by_kind = |kind: &str| {
        rendered
            .resources
            .iter()
            .find(|r| r["kind"].as_str() == Some(kind))
            .unwrap_or_else(|| panic!("no {kind} in rendered output: {rendered:?}"))
    };

    let deploy = by_kind("Deployment");
    assert_eq!(
        deploy["metadata"]["name"],
        serde_yaml::Value::String("hello".into()),
        "fullnameOverride wires to input.name"
    );
    assert_eq!(
        deploy["spec"]["replicas"],
        serde_yaml::Value::Number(3.into()),
        "replicas input flows to chart values"
    );

    let svc = by_kind("Service");
    assert_eq!(
        svc["metadata"]["name"],
        serde_yaml::Value::String("hello".into())
    );
    assert_eq!(
        svc["spec"]["type"],
        serde_yaml::Value::String("ClusterIP".into())
    );
}
