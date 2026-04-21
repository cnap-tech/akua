//! Bundled `akua.*` KCL stdlib — thin wrappers over `kcl_plugin.*`
//! so authoring code imports `akua.helm` / `akua.pkg` instead of
//! reaching into KCL's raw plugin namespace.
//!
//! The `.k` sources live under `crates/akua-core/stdlib/akua/` and
//! are embedded via `include_str!`. On first render this module
//! materializes them to a per-process tempdir and hands the path to
//! [`ExecProgramArgs.external_pkgs`] so `import akua.*` resolves
//! there.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const HELM_K: &str = include_str!("../stdlib/akua/helm.k");
const KUSTOMIZE_K: &str = include_str!("../stdlib/akua/kustomize.k");
const PKG_K: &str = include_str!("../stdlib/akua/pkg.k");

/// Minimal `kcl.mod` — KCL's loader requires the external pkg root
/// to be a real KCL package (see `kcl/crates/api/.../testdata`), so
/// we ship one. `name` matches the pkg_name we register in
/// `external_pkgs`, which is how `import akua.helm` resolves.
const KCL_MOD: &str = "[package]\nname = \"akua\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n";

/// Root directory that maps to `akua` in KCL's `external_pkgs`.
/// `external_pkgs: [{ pkg_name: "akua", pkg_path: stdlib_root() }]`
/// makes `import akua.helm` resolve to `<root>/helm.k`.
///
/// Materialized once per process on first call; subsequent calls
/// return the cached path. The tempdir sticks around for process
/// lifetime — on macOS/Linux it lands under `$TMPDIR` and is
/// reaped naturally.
pub fn stdlib_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "akua-stdlib-{}-{}",
            std::process::id(),
            // Wall-clock nanos tag: cross-process uniqueness when a
            // prior run crashed before $TMPDIR was reaped and left a
            // stale dir under the same pid.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).expect("mkdir akua stdlib tempdir");
        std::fs::write(dir.join("kcl.mod"), KCL_MOD).expect("write akua/kcl.mod");
        std::fs::write(dir.join("helm.k"), HELM_K).expect("write akua/helm.k");
        std::fs::write(dir.join("kustomize.k"), KUSTOMIZE_K).expect("write akua/kustomize.k");
        std::fs::write(dir.join("pkg.k"), PKG_K).expect("write akua/pkg.k");
        dir
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdlib_root_materializes_helm_and_pkg() {
        let root = stdlib_root();
        assert!(root.is_dir(), "root must be a directory: {}", root.display());
        assert!(root.join("kcl.mod").is_file());
        assert!(root.join("helm.k").is_file());
        assert!(root.join("kustomize.k").is_file());
        assert!(root.join("pkg.k").is_file());

        let helm = std::fs::read_to_string(root.join("helm.k")).unwrap();
        assert!(helm.contains("kcl_plugin.helm"));
        assert!(helm.contains("template"));

        let kustomize = std::fs::read_to_string(root.join("kustomize.k")).unwrap();
        assert!(kustomize.contains("kcl_plugin.kustomize"));
        assert!(kustomize.contains("build"));

        let pkg = std::fs::read_to_string(root.join("pkg.k")).unwrap();
        assert!(pkg.contains("kcl_plugin.pkg"));
        assert!(pkg.contains("render"));
    }

    #[test]
    fn stdlib_root_is_stable_across_calls() {
        let a = stdlib_root();
        let b = stdlib_root();
        assert_eq!(a, b);
    }
}
