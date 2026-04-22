//! Resolve `charts.*` dependencies declared in `akua.toml` into typed
//! [`ResolvedChart`] values.
//!
//! A `Package.k` that writes
//!
//! ```kcl
//! import charts.nginx
//! ```
//!
//! is asking akua's loader to materialize a per-render KCL package named
//! `charts` whose `nginx.k` points at the on-disk chart directory the
//! `nginx` dep in `akua.toml` resolves to. That path + a content-addressed
//! digest of the chart tree is exactly what this module computes.
//!
//! Phase 2a (this crate's shipping scope) resolves **local-path deps
//! only** — OCI pull + digest verify + git checkout land in Phase 2b.
//! OCI and git deps here return [`ChartResolveError::UnsupportedSource`]
//! pointing at the roadmap.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::mod_file::{AkuaManifest, DependencySource};

/// A single resolved chart dep — a materialized on-disk directory plus
/// a content-addressed digest of the tree (filenames + contents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChart {
    /// Local alias in `akua.toml` / the `import charts.<name>` stem.
    pub name: String,

    /// Canonicalized absolute path on disk. Safe to hand to
    /// `helm-engine-wasm::render_dir` directly.
    pub abs_path: PathBuf,

    /// `sha256:<hex>` of the chart tree. Stable across machines when
    /// file contents + names are identical.
    pub sha256: String,
}

/// The resolver's output. Canonical order (alphabetical by dep name)
/// so downstream users — `akua.lock` writers, `charts/` KCL module
/// generators — get deterministic iteration for free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedCharts {
    pub entries: BTreeMap<String, ResolvedChart>,
}

impl ResolvedCharts {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedChart> {
        self.entries.get(name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChartResolveError {
    #[error("chart `{name}`: path-dep target `{}` does not exist", path.display())]
    NotFound { name: String, path: PathBuf },

    #[error("chart `{name}`: path-dep target `{}` is not a directory", path.display())]
    NotADirectory { name: String, path: PathBuf },

    #[error("chart `{name}`: i/o at `{}`: {source}", path.display())]
    Io {
        name: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// OCI / git deps point at Phase 2b. Deliberately distinguishable
    /// from a user mistake so the CLI can surface the roadmap link.
    #[error("chart `{name}`: {kind} source not yet supported — waiting on Phase 2b (OCI pull + digest verify). See docs/roadmap.md.")]
    UnsupportedSource { name: String, kind: &'static str },

    /// `replace.path` overrides on oci/git deps are Phase 2b as well —
    /// path-only deps don't use replace at all (rejected by manifest
    /// validation upstream).
    #[error("chart `{name}`: replace override not yet supported — Phase 2b")]
    ReplaceUnsupported { name: String },
}

/// Resolve every dep in `manifest` against `workspace_root` and return
/// a [`ResolvedCharts`] suitable for threading into
/// `package_k::render_with_charts`.
///
/// `workspace_root` is the directory `akua.toml` sits in. Relative
/// path deps resolve against it.
pub fn resolve(
    manifest: &AkuaManifest,
    workspace_root: &Path,
) -> Result<ResolvedCharts, ChartResolveError> {
    let mut entries = BTreeMap::new();
    for (name, dep) in &manifest.dependencies {
        if dep.replace.is_some() {
            return Err(ChartResolveError::ReplaceUnsupported { name: name.clone() });
        }
        // source() is `Some` for any manifest that passed validation.
        let source = dep
            .source()
            .expect("manifest validated before resolver entry");
        match source {
            DependencySource::Path => {
                let requested = dep.path.as_deref().expect("path source has path set");
                let resolved = resolve_path(name, requested, workspace_root)?;
                entries.insert(name.clone(), resolved);
            }
            DependencySource::Oci => {
                return Err(ChartResolveError::UnsupportedSource {
                    name: name.clone(),
                    kind: "oci://",
                });
            }
            DependencySource::Git => {
                return Err(ChartResolveError::UnsupportedSource {
                    name: name.clone(),
                    kind: "git",
                });
            }
        }
    }
    Ok(ResolvedCharts { entries })
}

fn resolve_path(
    name: &str,
    requested: &str,
    workspace_root: &Path,
) -> Result<ResolvedChart, ChartResolveError> {
    let rel = PathBuf::from(requested);
    let joined = if rel.is_absolute() {
        rel
    } else {
        workspace_root.join(rel)
    };

    let canon = match joined.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ChartResolveError::NotFound {
                name: name.to_string(),
                path: joined,
            });
        }
        Err(e) => {
            return Err(ChartResolveError::Io {
                name: name.to_string(),
                path: joined,
                source: e,
            });
        }
    };

    let meta = canon.metadata().map_err(|e| ChartResolveError::Io {
        name: name.to_string(),
        path: canon.clone(),
        source: e,
    })?;
    if !meta.is_dir() {
        return Err(ChartResolveError::NotADirectory {
            name: name.to_string(),
            path: canon,
        });
    }

    let sha256 = hash_dir(&canon).map_err(|e| ChartResolveError::Io {
        name: name.to_string(),
        path: canon.clone(),
        source: e,
    })?;

    Ok(ResolvedChart {
        name: name.to_string(),
        abs_path: canon,
        sha256,
    })
}

/// Content-hash a directory tree. Walks files in sorted-by-relative-path
/// order so the digest is stable across filesystems (ext4 returns
/// `readdir` order; APFS returns arbitrary order). For each file the
/// hasher absorbs `<rel_path>\0<bytes>\n` — the NUL separator rules out
/// a "file A ends where file B's name begins" collision.
///
/// Symlinks are skipped entirely: their target lives outside the chart
/// dir, which breaks both determinism and the sandbox assumption that
/// the tarball we hand the engine has no escape hatches. An actual
/// chart needing a symlink is already broken on Windows hosts.
fn hash_dir(root: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, abs) in files {
        let bytes = std::fs::read(&abs)?;
        // Use `to_string_lossy` for cross-platform parity: Windows uses
        // UTF-16 OsStr internally; Unix is bytes. A chart with
        // non-UTF8-path filenames is broken regardless — collapsing
        // here doesn't create realistic collisions.
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(&bytes);
        hasher.update(b"\n");
    }
    Ok(format!("sha256:{}", hex_encode(&hasher.finalize())))
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let path = entry.path();
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walker stays under root")
                .to_path_buf();
            out.push((rel, path));
        }
        // Symlinks deliberately skipped — see `hash_dir` rationale.
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest(body: &str) -> AkuaManifest {
        let src = format!(
            r#"
[package]
name    = "test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
{body}
"#
        );
        AkuaManifest::parse(&src).expect("manifest parse")
    }

    /// Write a minimal chart tree (Chart.yaml + templates/cm.yaml) at
    /// `root`. Returns `root` for fluency in the callsite.
    fn write_minimal_chart(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join("templates")).unwrap();
        std::fs::write(
            root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(
            root.join("templates/cm.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo\n",
        )
        .unwrap();
        root.to_path_buf()
    }

    #[test]
    fn resolves_local_path_dep() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);

        let resolved = resolve(&manifest, ws.path()).expect("resolve");
        assert_eq!(resolved.len(), 1);
        let nginx = resolved.get("nginx").expect("nginx entry");
        assert_eq!(nginx.name, "nginx");
        assert!(nginx.abs_path.ends_with("charts/nginx"));
        assert!(nginx.abs_path.is_absolute());
        assert!(
            nginx.sha256.starts_with("sha256:"),
            "digest shape: {}",
            nginx.sha256
        );
        assert_eq!(nginx.sha256.len(), "sha256:".len() + 64);
    }

    #[test]
    fn digest_is_stable_across_calls() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);

        let a = resolve(&manifest, ws.path()).unwrap();
        let b = resolve(&manifest, ws.path()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn digest_changes_when_chart_contents_change() {
        let ws = tempfile::tempdir().unwrap();
        let chart = ws.path().join("charts/nginx");
        write_minimal_chart(&chart);
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);
        let before = resolve(&manifest, ws.path()).unwrap();

        // Mutate one template.
        std::fs::write(
            chart.join("templates/cm.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo2\n",
        )
        .unwrap();

        let after = resolve(&manifest, ws.path()).unwrap();
        assert_ne!(before.get("nginx").unwrap().sha256, after.get("nginx").unwrap().sha256);
    }

    #[test]
    fn digest_stable_across_file_creation_order() {
        // Create two charts with the same final contents but different
        // creation sequences — digest should match. Guards against
        // `readdir`-order flakiness on unsorted hashing.
        let ws_a = tempfile::tempdir().unwrap();
        let ws_b = tempfile::tempdir().unwrap();
        let chart_a = ws_a.path().join("c");
        let chart_b = ws_b.path().join("c");
        std::fs::create_dir_all(chart_a.join("templates")).unwrap();
        std::fs::create_dir_all(chart_b.join("templates")).unwrap();

        // A: Chart.yaml first, then cm.yaml
        std::fs::write(chart_a.join("Chart.yaml"), "v: 1\n").unwrap();
        std::fs::write(chart_a.join("templates/cm.yaml"), "body\n").unwrap();
        // B: cm.yaml first, then Chart.yaml
        std::fs::write(chart_b.join("templates/cm.yaml"), "body\n").unwrap();
        std::fs::write(chart_b.join("Chart.yaml"), "v: 1\n").unwrap();

        let mani_a = minimal_manifest(r#"x = { path = "./c" }"#);
        let mani_b = minimal_manifest(r#"x = { path = "./c" }"#);
        let a = resolve(&mani_a, ws_a.path()).unwrap();
        let b = resolve(&mani_b, ws_b.path()).unwrap();
        assert_eq!(a.get("x").unwrap().sha256, b.get("x").unwrap().sha256);
    }

    #[test]
    fn multiple_deps_alphabetical() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/zulu"));
        write_minimal_chart(&ws.path().join("charts/alpha"));
        let manifest = minimal_manifest(
            r#"
zulu  = { path = "./charts/zulu" }
alpha = { path = "./charts/alpha" }
"#,
        );
        let resolved = resolve(&manifest, ws.path()).unwrap();
        let names: Vec<&str> = resolved.entries.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["alpha", "zulu"], "BTreeMap iteration is sorted");
    }

    #[test]
    fn missing_path_dep_produces_typed_error() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(r#"ghost = { path = "./nope" }"#);
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::NotFound { ref name, .. } if name == "ghost"),
            "got: {err:?}"
        );
    }

    #[test]
    fn path_dep_pointing_at_a_file_is_rejected() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("not-a-chart.txt"), "hi").unwrap();
        let manifest = minimal_manifest(r#"bad = { path = "./not-a-chart.txt" }"#);
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::NotADirectory { ref name, .. } if name == "bad"),
            "got: {err:?}"
        );
    }

    #[test]
    fn oci_dep_surfaces_phase_2b_error() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://ghcr.io/foo/nginx", version = "1.0.0" }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::UnsupportedSource { ref name, kind: "oci://" } if name == "nginx"),
            "got: {err:?}"
        );
        // Error message must point a user at the roadmap.
        assert!(err.to_string().contains("Phase 2b"));
    }

    #[test]
    fn git_dep_surfaces_phase_2b_error() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(
            r#"libs = { git = "https://github.com/foo/bar", tag = "v1.0.0" }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::UnsupportedSource { ref name, kind: "git" } if name == "libs"),
            "got: {err:?}"
        );
    }

    #[test]
    fn replace_directive_rejected_until_phase_2b() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx-fork"));
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://r/n", version = "1.0.0", replace = { path = "./charts/nginx-fork" } }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::ReplaceUnsupported { ref name } if name == "nginx"),
            "got: {err:?}"
        );
    }

    #[test]
    fn empty_deps_returns_empty() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest("");
        let resolved = resolve(&manifest, ws.path()).unwrap();
        assert!(resolved.is_empty());
    }
}
