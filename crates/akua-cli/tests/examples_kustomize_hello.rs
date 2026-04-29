//! End-to-end render of `examples/09-kustomize-hello/` — exercises the
//! `kustomize.build` plugin callable through the wasmtime sandbox.
//! No inputs (the kustomization tree is fully declarative).
//!
//! Skips cleanly if `kustomize-engine.wasm` or the render-worker
//! module haven't been built yet.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/09-kustomize-hello")
}

#[test]
fn renders_kustomize_hello_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve charts");
    // 09 has no chart deps — kustomize tree is local.
    assert!(resolved.entries.is_empty());

    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let inputs = serde_yaml::Value::Null;

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
            if msg.contains("kustomize-engine.wasm not built")
                || msg.contains("worker module wasn't compiled")
            {
                eprintln!("skipping: {msg}");
                return;
            }
            panic!("render failed: {e}");
        }
    };

    assert_eq!(
        rendered.resources.len(),
        1,
        "overlay emits one prod-prefixed ConfigMap"
    );

    let golden = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("rendered/000-configmap-prod-hello.yaml")).expect("read golden"),
    )
    .expect("parse golden");
    assert_eq!(
        rendered.resources[0], golden,
        "rendered ConfigMap drifted from rendered/000-configmap-prod-hello.yaml"
    );
}
