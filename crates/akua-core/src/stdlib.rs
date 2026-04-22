//! Bundled `akua.*` KCL stdlib — thin wrappers over `kcl_plugin.*`
//! so authoring code imports `akua.helm` / `akua.pkg` instead of
//! reaching into KCL's raw plugin namespace.
//!
//! The `.k` sources live under `crates/akua-core/stdlib/akua/` and
//! are embedded via `include_str!`. On first render this module
//! materializes them to a per-process tempdir and hands the path to
//! [`ExecProgramArgs.external_pkgs`] so `import akua.*` resolves
//! there.
//!
//! Per-render addition: when the caller's `akua.toml` declares
//! `charts.*` deps, [`materialize_charts`] writes a `charts` KCL
//! package next to the static stdlib, one `.k` file per dep pointing
//! at the resolved path + digest. Callers in `package_k` hand that
//! tempdir to KCL alongside the static `akua` external_pkg.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const CTX_K: &str = include_str!("../stdlib/akua/ctx.k");
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
/// Materialize a `charts` KCL package containing one `.k` file per
/// dep in `resolved`, plus a minimal `kcl.mod` so KCL's loader accepts
/// the directory. Returns the [`tempfile::TempDir`] so the caller
/// can register it via `ExternalPkg` and drop it after
/// `exec_program` returns.
///
/// Each generated `<name>.k` exposes two constants:
///
/// ```text
/// path   = "<abs_path>"
/// sha256 = "sha256:<hex>"
/// ```
///
/// Packages wire them up as
///
/// ```kcl
/// import charts.nginx
/// helm.template(helm.Template { chart = nginx.path, ... })
/// ```
///
/// Richer typed `Chart` / `Values` schemas (and the `helm.Template.chart: str | Chart`
/// union that consumes them) ship in Phase 2b.
pub fn materialize_charts(
    resolved: &crate::chart_resolver::ResolvedCharts,
) -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new().prefix("akua-charts-").tempdir()?;
    const CHARTS_KCL_MOD: &str =
        "[package]\nname = \"charts\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n";
    std::fs::write(dir.path().join("kcl.mod"), CHARTS_KCL_MOD)?;
    for (name, chart) in &resolved.entries {
        // Two fields, both typed as `str`, so callers can do
        //   helm.template({ chart = charts.nginx.path, ... })
        // without pulling in schemas. KCL string literals take care of
        // any path escaping — we only ever emit `"..."` around the
        // debug-formatted PathBuf display.
        let body = format!(
            "# Auto-generated per-render from akua.toml dep `{name}`.\n\
             # Points at a resolved chart directory on disk.\n\
             \n\
             path: str = {path_literal}\n\
             sha256: str = \"{digest}\"\n",
            path_literal = kcl_str_literal(&chart.abs_path.to_string_lossy()),
            digest = chart.sha256,
        );
        std::fs::write(dir.path().join(format!("{name}.k")), body)?;
    }
    Ok(dir)
}

/// Quote a string as a KCL double-quoted literal, escaping `\` and `"`.
/// Chart paths may contain spaces on macOS (`/Users/someone with spaces/`)
/// so `format!("\"{s}\"")` isn't safe — backslashes in a path would
/// otherwise be interpreted as KCL escape sequences.
fn kcl_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

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
        std::fs::write(dir.join("ctx.k"), CTX_K).expect("write akua/ctx.k");
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
    use std::path::PathBuf;

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

    #[test]
    fn materialize_charts_writes_expected_files() {
        use crate::chart_resolver::{ResolvedChart, ResolvedCharts};
        use std::collections::BTreeMap;

        let mut entries = BTreeMap::new();
        entries.insert(
            "nginx".to_string(),
            ResolvedChart {
                name: "nginx".to_string(),
                abs_path: PathBuf::from("/tmp/charts/nginx"),
                sha256: "sha256:abc123".to_string(),
                source: crate::chart_resolver::ResolvedSource::Path {
                    declared: "./charts/nginx".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        let tmp = materialize_charts(&resolved).expect("materialize");
        let root = tmp.path();
        assert!(root.join("kcl.mod").is_file());
        let kcl_mod = std::fs::read_to_string(root.join("kcl.mod")).unwrap();
        assert!(kcl_mod.contains("name = \"charts\""));

        let nginx_k = std::fs::read_to_string(root.join("nginx.k")).unwrap();
        assert!(nginx_k.contains("path: str = \"/tmp/charts/nginx\""));
        assert!(nginx_k.contains("sha256: str = \"sha256:abc123\""));
    }

    #[test]
    fn materialize_charts_escapes_backslash_in_path() {
        use crate::chart_resolver::{ResolvedChart, ResolvedCharts};
        use std::collections::BTreeMap;

        let mut entries = BTreeMap::new();
        entries.insert(
            "win".to_string(),
            ResolvedChart {
                name: "win".to_string(),
                // Quoted `"` and `\` both exercised — KCL would
                // otherwise mis-parse.
                abs_path: PathBuf::from(r#"C:\charts\w"ei"rd"#),
                sha256: "sha256:f00".to_string(),
                source: crate::chart_resolver::ResolvedSource::Path {
                    declared: r#"C:\charts\w"ei"rd"#.to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };
        let tmp = materialize_charts(&resolved).unwrap();
        let body = std::fs::read_to_string(tmp.path().join("win.k")).unwrap();
        assert!(body.contains(r#"path: str = "C:\\charts\\w\"ei\"rd""#));
    }

    #[test]
    fn materialize_charts_empty_resolves_with_only_kcl_mod() {
        use crate::chart_resolver::ResolvedCharts;
        let resolved = ResolvedCharts::default();
        let tmp = materialize_charts(&resolved).unwrap();
        assert!(tmp.path().join("kcl.mod").is_file());
        // No .k files when no deps.
        let k_files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("k"))
            .collect();
        assert_eq!(k_files.len(), 0);
    }

    #[test]
    fn kcl_str_literal_escapes_special_chars() {
        assert_eq!(kcl_str_literal("plain"), r#""plain""#);
        assert_eq!(kcl_str_literal(r#"a"b"#), r#""a\"b""#);
        assert_eq!(kcl_str_literal(r"a\b"), r#""a\\b""#);
        assert_eq!(kcl_str_literal("line1\nline2"), r#""line1\nline2""#);
    }
}
