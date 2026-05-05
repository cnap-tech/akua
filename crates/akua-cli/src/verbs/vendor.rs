//! Shared vendor-pair collection used by `akua publish` + `akua pack`
//! to embed resolved OCI/git deps under `.akua/vendor/<name>/` in the
//! output tarball.
//!
//! Why not in akua-core? The function emits a `stderr` warning on
//! resolver failure — that's CLI-layer, not library-layer behavior.
//! Keeping it here lets core stay quiet + pure.

use std::path::{Path, PathBuf};

use akua_core::AkuaManifest;

/// Resolve non-path deps so their chart content can be vendored into
/// the output tarball. Path deps already live in the workspace tree
/// (typically `vendor/`) and are packed via the workspace walk —
/// don't re-vendor them or they'll appear twice in the tarball.
///
/// A resolver failure here is *loud*: we emit a stderr warning so
/// the publisher doesn't ship an un-vendored artifact by accident.
/// Returns the pairs the resolver *did* produce — a partial-vendor
/// result is still better than nothing when one dep out of many is
/// broken.
pub fn collect_vendor_pairs(workspace: &Path, manifest: &AkuaManifest) -> Vec<(String, PathBuf)> {
    use akua_core::chart_resolver::{self, ResolvedSource, ResolverOptions};
    use akua_core::AkuaLock;

    let expected_digests = AkuaLock::load(workspace)
        .map(|lock| {
            lock.packages
                .into_iter()
                .filter(|p| p.is_oci())
                .map(|p| (p.name, p.digest))
                .collect()
        })
        .unwrap_or_default();
    let opts = ResolverOptions {
        offline: false,
        cache_root: None,
        expected_digests,
        cosign_public_key_pem: None,
        reject_replace: chart_resolver::replace_rejected_from_env(),
    };
    let resolved = match chart_resolver::resolve_with_options(manifest, workspace, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "warning: dep resolution failed, packed artifact will not render offline: {e}"
            );
            return Vec::new();
        }
    };

    let mut pairs = Vec::new();
    for chart in resolved.entries.values() {
        // Path / replace → already in the workspace walk; don't
        // double-vendor.
        let include = matches!(
            chart.source,
            ResolvedSource::Oci { .. } | ResolvedSource::Git { .. }
        );
        if include {
            pairs.push((chart.name.clone(), chart.abs_path.clone()));
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::workspace_with;
    use std::fs;

    const NO_DEPS: &str = r#"[package]
name = "vendor-test"
version = "0.0.1"
edition = "akua.dev/v1alpha1"
"#;

    #[test]
    fn empty_manifest_yields_empty_vendor_pairs() {
        let ws = workspace_with(NO_DEPS);
        let manifest = AkuaManifest::load(ws.path()).unwrap();
        let pairs = collect_vendor_pairs(ws.path(), &manifest);
        assert!(pairs.is_empty(), "no deps → no vendor pairs");
    }

    #[test]
    fn path_dep_is_excluded_from_vendor_pairs() {
        // Path deps live inside the workspace tree and are picked up
        // by the workspace walk in `akua publish` / `akua pack`.
        // Re-vendoring would duplicate them in the output tarball.
        let ws = workspace_with(&format!(
            "{NO_DEPS}\n[dependencies]\nlocal = {{ path = \"./local-chart\" }}\n"
        ));
        let chart_dir = ws.path().join("local-chart");
        fs::create_dir(&chart_dir).unwrap();
        fs::create_dir(chart_dir.join("templates")).unwrap();
        fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: local\nversion: 0.0.1\n",
        )
        .unwrap();
        fs::write(chart_dir.join("templates/cm.yaml"), "kind: ConfigMap\n").unwrap();

        let manifest = AkuaManifest::load(ws.path()).unwrap();
        let pairs = collect_vendor_pairs(ws.path(), &manifest);
        assert!(
            pairs.is_empty(),
            "path dep must NOT appear in vendor pairs, got: {pairs:?}"
        );
    }

    #[test]
    fn resolver_failure_returns_empty_vec_after_warning() {
        // Path that doesn't exist → resolver fails. The function emits
        // a stderr warning and returns Vec::new() so packing can
        // proceed with whatever it has, rather than aborting.
        let ws = workspace_with(&format!(
            "{NO_DEPS}\n[dependencies]\nbroken = {{ path = \"./does-not-exist\" }}\n"
        ));
        let manifest = AkuaManifest::load(ws.path()).unwrap();
        let pairs = collect_vendor_pairs(ws.path(), &manifest);
        assert!(
            pairs.is_empty(),
            "resolver-failure path returns empty Vec, got: {pairs:?}"
        );
    }
}
