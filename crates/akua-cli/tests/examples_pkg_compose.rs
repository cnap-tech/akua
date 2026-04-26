//! End-to-end render of `examples/08-pkg-compose/` — verifies that
//! `pkg.render` sentinel expansion produces the expected per-component
//! ConfigMaps when the same inner Package is composed twice with
//! different inputs. Pure-KCL, no engine plugins.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/08-pkg-compose")
}

#[test]
fn renders_pkg_compose_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve charts");
    assert!(resolved.entries.is_empty());

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
        2,
        "two pkg.render sentinels expand to two ConfigMaps"
    );

    // Resource order depends on KCL evaluation; look up by metadata.name.
    let by_name = |name: &str| {
        rendered
            .resources
            .iter()
            .find(|r| r["metadata"]["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("no ConfigMap named {name} in: {rendered:?}"))
    };

    let golden_frontend = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("rendered/000-configmap-frontend.yaml")).expect("read golden FE"),
    )
    .expect("parse golden FE");
    let golden_backend = serde_yaml::from_slice::<serde_yaml::Value>(
        &std::fs::read(dir.join("rendered/001-configmap-backend.yaml")).expect("read golden BE"),
    )
    .expect("parse golden BE");

    assert_eq!(
        *by_name("frontend"),
        golden_frontend,
        "frontend ConfigMap drifted from rendered/000-configmap-frontend.yaml"
    );
    assert_eq!(
        *by_name("backend"),
        golden_backend,
        "backend ConfigMap drifted from rendered/001-configmap-backend.yaml"
    );
}
