//! End-to-end render of `examples/00-helm-hello/` — the simplest Helm
//! integration, using a raw-string `./chart` path (the pre-Phase-2a
//! form) rather than a typed `charts.*` import. Diffs the live render
//! against `examples/00-helm-hello/rendered/` golden output.
//!
//! Skips cleanly if `helm-engine.wasm` or the render-worker module
//! haven't been built yet.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/00-helm-hello")
}

#[test]
fn renders_helm_hello_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve charts");
    // 00 uses a raw-string chart path; no typed deps to resolve.
    assert!(resolved.entries.is_empty());

    let package = PackageK::load(&dir.join("package.k")).expect("load package.k");
    let inputs = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("inputs.example.yaml")).expect("read inputs.example.yaml"),
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

    assert_eq!(rendered.resources.len(), 1, "chart emits one ConfigMap");

    let golden = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("rendered/000-configmap-hello-greeting.yaml"))
            .expect("read golden"),
    )
    .expect("parse golden");
    assert_eq!(
        rendered.resources[0], golden,
        "rendered ConfigMap drifted from rendered/000-configmap-hello-greeting.yaml"
    );
}
