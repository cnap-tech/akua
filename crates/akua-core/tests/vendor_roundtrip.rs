//! Phase 7 C round-trip: pack a workspace with a vendored OCI dep,
//! unpack it into a fresh dir, run the resolver in offline mode, and
//! confirm the dep resolved from `.akua/vendor/` without touching
//! the network.
//!
//! Guards the offline-after-pull contract. A future refactor that
//! breaks any link in the chain (publish-side vendor embed,
//! pack_workspace tarball shape, unpack_to extraction, resolver
//! vendor-first lookup) trips this test.

#![cfg(all(feature = "oci-fetch", feature = "engine-kcl"))]

use std::path::Path;

use akua_core::{
    chart_resolver::{self, ResolvedSource, ResolverOptions},
    package_tar, AkuaManifest,
};

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn minimal_chart(root: &Path) {
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::write(
        root.join("Chart.yaml"),
        b"apiVersion: v2\nname: nginx\nversion: 0.1.0\n",
    )
    .unwrap();
    std::fs::write(
        root.join("templates/cm.yaml"),
        b"apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo\n",
    )
    .unwrap();
}

#[test]
fn publish_then_pull_then_render_offline() {
    // Source workspace: declares nginx as an OCI dep (never
    // actually pulled — the "cache" below is a hand-built chart
    // we hand to `pack_workspace_with_vendored_deps`).
    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "akua.toml",
        br#"
[package]
name    = "round-trip"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
nginx = { oci = "oci://ghcr.io/acme/nginx", version = "1.2.3" }
"#,
    );
    write(src.path(), "package.k", b"resources = []\n");

    // Simulate a prior `akua add`: the chart lives in a cache dir
    // outside the workspace, as it would under
    // `$XDG_CACHE_HOME/akua/oci/sha256/<hex>/nginx/`.
    let cache = tempfile::tempdir().unwrap();
    let chart_dir = cache.path().join("nginx");
    minimal_chart(&chart_dir);

    // Publish: pack the workspace + vendor the cached chart.
    let tar_gz = package_tar::pack_workspace_with_vendored_deps(
        src.path(),
        &[("nginx".to_string(), chart_dir.clone())],
    )
    .expect("pack with vendored dep");

    // Pull: unpack the tarball into a fresh workspace, as
    // `akua pull` would.
    let pulled = tempfile::tempdir().unwrap();
    package_tar::unpack_to(&tar_gz, pulled.path()).expect("unpack");

    // Vendor dir is present post-extract.
    assert!(
        pulled.path().join(".akua/vendor/nginx/Chart.yaml").is_file(),
        "vendor dir missing after unpack"
    );

    // Render: resolve in offline mode. Without vendor-first the
    // OCI dep would fail (`offline mode needs a lockfile-pinned
    // digest`); with vendor-first it succeeds.
    let manifest = AkuaManifest::load(pulled.path()).expect("load");
    let opts = ResolverOptions {
        offline: true,
        cache_root: None,
        expected_digests: Default::default(),
        cosign_public_key_pem: None,
    };
    let resolved = chart_resolver::resolve_with_options(&manifest, pulled.path(), &opts)
        .expect("offline resolve from vendor");

    let nginx = resolved.entries.get("nginx").expect("nginx entry");
    assert!(
        nginx.abs_path.ends_with(".akua/vendor/nginx"),
        "abs_path: {:?}",
        nginx.abs_path
    );
    match &nginx.source {
        ResolvedSource::Oci {
            oci,
            version,
            blob_digest,
        } => {
            assert_eq!(oci, "oci://ghcr.io/acme/nginx");
            assert_eq!(version, "1.2.3");
            assert!(blob_digest.starts_with("sha256:"));
        }
        other => panic!("expected ResolvedSource::Oci, got {other:?}"),
    }
    // Digest from the vendored tree matches what the resolver
    // recomputed — same contract as a path dep.
    assert!(nginx.sha256.starts_with("sha256:"));
}
