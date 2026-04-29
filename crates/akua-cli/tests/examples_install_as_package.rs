//! End-to-end render of `examples/11-install-as-package/` — verifies
//! the install-as-Package shape: `pkg.render` returns a real list, the
//! tenant overlay applies via list comprehension, the PDB is filtered
//! out, and the install-meta ConfigMap is appended. Pure-KCL.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/11-install-as-package")
}

#[test]
fn renders_install_as_package_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    let resolved = chart_resolver::resolve(&manifest, &dir).expect("resolve charts");

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
            if msg.contains("worker module wasn't compiled") {
                eprintln!("skipping: {msg}");
                return;
            }
            panic!("render failed: {e}");
        }
    };

    assert_eq!(
        rendered.resources.len(),
        3,
        "Deployment + Service (PDB filtered) + extras ConfigMap = 3"
    );

    // Confirm PDB was actually filtered — the upstream emits it.
    assert!(
        !rendered
            .resources
            .iter()
            .any(|r| r["kind"].as_str() == Some("PodDisruptionBudget")),
        "PodDisruptionBudget should have been filtered out"
    );

    // Every resource should carry the tenant overlay label.
    for r in &rendered.resources {
        assert_eq!(
            r["metadata"]["labels"]["install.cnap.tech/tenant"]
                .as_str()
                .unwrap_or(""),
            "acme",
            "tenant overlay missing on {}/{}",
            r["kind"].as_str().unwrap_or("?"),
            r["metadata"]["name"].as_str().unwrap_or("?")
        );
    }

    // Byte-equal goldens.
    let goldens = [
        "rendered/000-deployment-webapp.yaml",
        "rendered/001-service-webapp.yaml",
        "rendered/002-configmap-webapp-install-meta.yaml",
    ];
    for (i, rel) in goldens.iter().enumerate() {
        let golden = serde_yaml::from_slice::<serde_yaml::Value>(
            &std::fs::read(dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse {rel}: {e}"));
        assert_eq!(
            rendered.resources[i], golden,
            "resource {i} drifted from {rel}"
        );
    }
}
