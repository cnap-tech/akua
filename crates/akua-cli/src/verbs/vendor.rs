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
