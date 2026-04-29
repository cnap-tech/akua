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
/// `Some(tempdir)` when `resolved` has entries; `None` otherwise.
/// Saves every caller the "if .is_empty() { None } else { Some(...) }"
/// guard around [`materialize_charts`]. Skips if there are no Helm
/// deps to wrap (KCL deps are materialized separately by the render
/// verb and don't go through the synthetic `charts.*` umbrella).
pub fn materialize_charts_if_any(
    resolved: &crate::chart_resolver::ResolvedCharts,
) -> std::io::Result<Option<tempfile::TempDir>> {
    if resolved.helm_charts().next().is_none() {
        return Ok(None);
    }
    materialize_charts(resolved).map(Some)
}

/// Synthesize an import-only stub directory for Akua-package deps.
/// Each dep with a `package.k` becomes a `<alias>.k` containing only
/// the upstream's `import` and `schema` declarations (extracted via
/// [`crate::pkg_stub::extract_schemas`]). The directory mounts as
/// ExternalPkg `pkgs` so consumers write `import pkgs.<alias>` to
/// reach the typed schemas without firing the upstream's body.
///
/// Returns `None` when no Akua-package deps are present.
pub fn materialize_pkg_stubs_if_any(
    resolved: &crate::chart_resolver::ResolvedCharts,
) -> std::io::Result<Option<tempfile::TempDir>> {
    let aliased: Vec<(&str, &crate::chart_resolver::ResolvedChart)> = resolved
        .kcl_pkgs()
        .filter(|(_, c)| c.is_akua_package())
        .collect();
    if aliased.is_empty() {
        return Ok(None);
    }
    let dir = tempfile::Builder::new().prefix("akua-pkgs-").tempdir()?;
    const PKGS_KCL_MOD: &str =
        "[package]\nname = \"pkgs\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n";
    std::fs::write(dir.path().join("kcl.mod"), PKGS_KCL_MOD)?;
    for (alias, chart) in aliased {
        let source = std::fs::read_to_string(chart.abs_path.join("package.k"))?;
        let stub = crate::pkg_stub::build_stub_module(alias, &source);
        std::fs::write(dir.path().join(format!("{alias}.k")), stub)?;
    }
    Ok(Some(dir))
}

pub fn materialize_charts(
    resolved: &crate::chart_resolver::ResolvedCharts,
) -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new().prefix("akua-charts-").tempdir()?;
    const CHARTS_KCL_MOD: &str =
        "[package]\nname = \"charts\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n";
    std::fs::write(dir.path().join("kcl.mod"), CHARTS_KCL_MOD)?;
    // Only Helm deps get the synthetic wrapper. KCL ecosystem deps
    // are mounted as standalone ExternalPkg entries by the render verb.
    for (name, chart) in resolved.helm_charts() {
        let values_schema = load_values_schema(&chart.abs_path);
        let body = build_chart_module(
            name,
            &crate::values_schema::kcl_string_literal(&chart.abs_path.to_string_lossy()),
            &chart.sha256,
            values_schema.as_deref(),
        );
        std::fs::write(dir.path().join(format!("{name}.k")), body)?;
    }
    Ok(dir)
}

/// Compose the generated `charts/<name>.k` source: the two data
/// constants, the optional typed `Values` schema, plus a `template`
/// lambda that pre-fills `chart = path` so authors write
///
/// ```kcl
/// import charts.nginx
/// resources = nginx.template(nginx.Values { replicaCount = 3 })
/// ```
///
/// instead of the longer `helm.template(helm.Template { chart = nginx.path, ... })`
/// boilerplate. When the chart has no values.schema.json we fall
/// back to a `{str:}` passthrough so the same entry-point works.
fn build_chart_module(
    dep_name: &str,
    path_literal: &str,
    digest: &str,
    values_schema_src: Option<&str>,
) -> String {
    let mut body = format!(
        "# Auto-generated per-render from akua.toml dep `{dep_name}`.\n\
         # slice C: exposes `path`, `sha256`, `Values`, and a\n\
         # pre-filled `template` callable. Regenerated every render.\n\
         \n\
         import akua.helm as _helm\n\
         \n\
         path: str = {path_literal}\n\
         sha256: str = \"{digest}\"\n\
         \n"
    );
    let values_type = if let Some(schema) = values_schema_src {
        body.push_str("# Typed Values schema, generated from values.schema.json.\n");
        body.push_str(schema);
        body.push('\n');
        "Values"
    } else {
        // No schema — accept any dict at the callsite. Keeps the
        // API shape consistent with typed-Values charts.
        "{str:}"
    };
    // Convenience wrapper: pre-fills `chart = path` so the callsite
    // is the values the author actually cares about. Single-argument
    // Options-schema-style call form (matches `akua.helm.template`) —
    // KCL's lambda parser is happiest when the parameter is a named
    // schema rather than a dict-with-defaults.
    let opts_default = if values_type == "Values" {
        "Values {}"
    } else {
        "{}"
    };
    body.push_str(&format!(
        "schema TemplateOpts:\n    values: {values_type} = {opts_default}\n    release: str = \"release\"\n    namespace: str = \"default\"\n\n"
    ));
    body.push_str(
        "template = lambda opts: TemplateOpts = TemplateOpts {} -> [{str:}] {\n    _helm.template(_helm.Template {\n        chart = path\n        values = opts.values\n        release = opts.release\n        namespace = opts.namespace\n    })\n}\n",
    );
    body
}

/// Read `values.schema.json` from the chart root, convert it to KCL,
/// and return the source. `None` when the file is absent. A
/// malformed schema surfaces on stderr (see CLAUDE.md — structured
/// errors, not prose) then falls back to `None`, so the render still
/// proceeds with an untyped `{str:}` values shape instead of
/// mysteriously crashing.
fn load_values_schema(chart_dir: &std::path::Path) -> Option<String> {
    let schema_path = chart_dir.join("values.schema.json");
    let bytes = std::fs::read(&schema_path).ok()?;
    match crate::values_schema::generate_from_bytes(&bytes) {
        Ok(gen) if !gen.source.is_empty() => Some(gen.source),
        Ok(_) => None, // non-object root; generator returns empty
        Err(e) => {
            eprintln!(
                "akua: skipping malformed values.schema.json at {}: {e}",
                schema_path.display()
            );
            None
        }
    }
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
        assert!(
            root.is_dir(),
            "root must be a directory: {}",
            root.display()
        );
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
                kind: crate::chart_resolver::PackageKind::HelmChart,
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
        // Slice-C addition: `template` lambda pre-filled with `chart = path`.
        assert!(nginx_k.contains("template = lambda"), "module: {nginx_k}");
        assert!(
            nginx_k.contains("schema TemplateOpts:"),
            "module: {nginx_k}"
        );
        // No values.schema.json → values type is the passthrough dict.
        assert!(nginx_k.contains("values: {str:}"), "module: {nginx_k}");
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
                kind: crate::chart_resolver::PackageKind::HelmChart,
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
    fn materialize_charts_embeds_values_schema_when_present() {
        use crate::chart_resolver::{ResolvedChart, ResolvedCharts};
        use std::collections::BTreeMap;

        // Build a real chart dir with a values.schema.json so the
        // stdlib generator picks it up end-to-end.
        let chart_root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(chart_root.path().join("templates")).unwrap();
        std::fs::write(
            chart_root.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(
            chart_root.path().join("values.schema.json"),
            r#"{
                "type": "object",
                "properties": {
                    "replicaCount": { "type": "integer", "default": 1 },
                    "image": {
                        "type": "object",
                        "properties": {
                            "repository": { "type": "string" }
                        },
                        "required": ["repository"]
                    }
                }
            }"#,
        )
        .unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "nginx".to_string(),
            ResolvedChart {
                name: "nginx".to_string(),
                abs_path: chart_root.path().canonicalize().unwrap(),
                sha256: "sha256:deadbeef".to_string(),
                kind: crate::chart_resolver::PackageKind::HelmChart,
                source: crate::chart_resolver::ResolvedSource::Path {
                    declared: "./vendor/nginx".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        let tmp = materialize_charts(&resolved).unwrap();
        let body = std::fs::read_to_string(tmp.path().join("nginx.k")).unwrap();
        // Existing fields still present.
        assert!(body.contains("path: str ="), "{}", body);
        // New: typed Values schema inlined.
        assert!(body.contains("schema Values:"), "{}", body);
        assert!(body.contains("replicaCount: int = 1"), "{}", body);
        assert!(body.contains("schema ValuesImage:"), "{}", body);
        // template() lambda typed on the generated Values schema
        // via the TemplateOpts wrapper.
        assert!(body.contains("schema TemplateOpts:"), "{}", body);
        assert!(body.contains("values: Values"), "{}", body);
        assert!(body.contains("Values {}"), "{}", body);
    }

    #[test]
    fn materialize_charts_skips_missing_values_schema() {
        use crate::chart_resolver::{ResolvedChart, ResolvedCharts};
        use std::collections::BTreeMap;

        // Chart dir without values.schema.json. Materialization still
        // produces path + sha256 and *no* schema block.
        let chart_root = tempfile::tempdir().unwrap();
        std::fs::write(
            chart_root.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "nginx".to_string(),
            ResolvedChart {
                name: "nginx".to_string(),
                abs_path: chart_root.path().canonicalize().unwrap(),
                sha256: "sha256:abc".to_string(),
                kind: crate::chart_resolver::PackageKind::HelmChart,
                source: crate::chart_resolver::ResolvedSource::Path {
                    declared: "./vendor/nginx".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        let tmp = materialize_charts(&resolved).unwrap();
        let body = std::fs::read_to_string(tmp.path().join("nginx.k")).unwrap();
        assert!(body.contains("path: str ="), "{}", body);
        assert!(
            !body.contains("schema Values"),
            "schema unexpectedly emitted: {}",
            body
        );
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
        use crate::values_schema::kcl_string_literal;
        assert_eq!(kcl_string_literal("plain"), r#""plain""#);
        assert_eq!(kcl_string_literal(r#"a"b"#), r#""a\"b""#);
        assert_eq!(kcl_string_literal(r"a\b"), r#""a\\b""#);
        assert_eq!(kcl_string_literal("line1\nline2"), r#""line1\nline2""#);
    }

    #[test]
    fn materialize_pkg_stubs_emits_alias_dot_k_with_render_lambda() {
        use crate::chart_resolver::{PackageKind, ResolvedChart, ResolvedCharts, ResolvedSource};
        use std::collections::BTreeMap;

        let upstream = tempfile::tempdir().unwrap();
        std::fs::write(
            upstream.path().join("akua.toml"),
            "[package]\nname = \"upstream\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();
        std::fs::write(
            upstream.path().join("package.k"),
            "schema Input:\n    appName: str\n\nresources = []\n",
        )
        .unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "upstream".to_string(),
            ResolvedChart {
                name: "upstream".to_string(),
                abs_path: upstream.path().to_path_buf(),
                sha256: "sha256:abc".to_string(),
                kind: PackageKind::KclModule,
                source: ResolvedSource::Path {
                    declared: "./upstream".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        let stub_dir = materialize_pkg_stubs_if_any(&resolved)
            .expect("materialize")
            .expect("Some(_)");
        let upstream_k = std::fs::read_to_string(stub_dir.path().join("upstream.k")).unwrap();
        assert!(upstream_k.contains("schema Input:"), "stub: {upstream_k}");
        assert!(
            upstream_k.contains("render = lambda inputs: Input"),
            "stub: {upstream_k}"
        );
        assert!(
            upstream_k.contains("package = \"upstream\""),
            "stub: {upstream_k}"
        );
        assert!(stub_dir.path().join("kcl.mod").is_file());
    }

    #[test]
    fn materialize_pkg_stubs_skips_plain_kcl_modules() {
        // KclModule deps that lack a `package.k` (i.e. raw KCL ecosystem
        // packages) must NOT appear in the pkgs umbrella — they go via
        // /kcl-pkgs/<alias> directly.
        use crate::chart_resolver::{PackageKind, ResolvedChart, ResolvedCharts, ResolvedSource};
        use std::collections::BTreeMap;

        let plain = tempfile::tempdir().unwrap();
        std::fs::write(
            plain.path().join("kcl.mod"),
            "[package]\nname = \"plain\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "plain".to_string(),
            ResolvedChart {
                name: "plain".to_string(),
                abs_path: plain.path().to_path_buf(),
                sha256: "sha256:abc".to_string(),
                kind: PackageKind::KclModule,
                source: ResolvedSource::Path {
                    declared: "./plain".to_string(),
                },
            },
        );
        let resolved = ResolvedCharts { entries };

        assert!(materialize_pkg_stubs_if_any(&resolved).unwrap().is_none());
    }
}
