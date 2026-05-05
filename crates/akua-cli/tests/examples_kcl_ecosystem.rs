//! End-to-end render of `examples/10-kcl-ecosystem/` against the live
//! `oci://ghcr.io/kcl-lang/k8s` registry. Proves the KCL OCI fetch +
//! ExternalPkg wiring by composing a typed Deployment against the
//! upstream schema bundle.
//!
//! First run pulls the layer from ghcr.io into `$XDG_CACHE_HOME/akua/oci/`;
//! subsequent runs (including subsequent CI runs that share the cache)
//! cache-hit and skip the network. The committed `akua.lock` digest
//! pins the exact blob the test resolves, so a drifted upstream tag
//! fails fast instead of silently picking up new bytes.
//!
//! Skips cleanly if the render-worker module hasn't been built yet.

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::{Path, PathBuf};

use akua_cli::verbs::render::render_in_worker;
use akua_core::chart_resolver::ResolverOptions;
use akua_core::lock_file::{AkuaLock, LockedPackage};
use akua_core::{chart_resolver, AkuaManifest, PackageK};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/10-kcl-ecosystem")
}

#[test]
// Live network pull + wasmtime epoch budget: cold runs trip
// `wasm trap: interrupt` inside the kcl loader if ghcr.io / cargo cache
// are slow. Skipped from `cargo test --workspace` so flakes don't
// shadow real failures; run explicitly via `cargo test -- --include-ignored`
// or `task release:validate` (which already passes that flag).
#[ignore = "online: pulls oci://ghcr.io/kcl-lang/k8s; sensitive to wasmtime epoch budget"]
fn renders_kcl_ecosystem_dep_against_golden() {
    let dir = example_dir();

    let manifest = AkuaManifest::load(&dir).expect("load akua.toml");
    assert!(
        manifest.dependencies.contains_key("k8s"),
        "example 10 must declare the k8s dep"
    );

    // Mirror the production `akua render` flow: load akua.lock for
    // digest pinning, run the resolver online so first-time pulls
    // populate `~/.cache/akua/oci/sha256/<digest>/` from ghcr.io.
    let lock = AkuaLock::load(&dir).expect("load akua.lock");
    let expected_digests = lock
        .packages
        .into_iter()
        .filter(LockedPackage::is_oci)
        .map(|p| (p.name, p.digest))
        .collect();
    let opts = ResolverOptions {
        offline: false,
        cache_root: None,
        expected_digests,
        cosign_public_key_pem: None,
        reject_replace: false,
    };
    let resolved =
        chart_resolver::resolve_with_options(&manifest, &dir, &opts).expect("resolve k8s pkg");
    let k8s = resolved.entries.get("k8s").expect("k8s entry");
    assert_eq!(
        k8s.kind,
        chart_resolver::PackageKind::KclModule,
        "kpm-published artifact must be detected as KclModule"
    );
    assert!(k8s.abs_path.is_absolute());
    // Marker file confirms the resolver pointed us at the package root.
    assert!(k8s.abs_path.join("kcl.mod").is_file());

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
