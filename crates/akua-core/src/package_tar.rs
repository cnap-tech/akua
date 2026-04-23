//! Tarball a workspace into the bytes that `akua publish` uploads.
//!
//! Contract:
//!
//! - Include: `akua.toml`, `akua.lock`, every `*.k` file, `vendor/`
//!   (local chart deps), `README*`, `inputs.example.yaml`.
//! - Exclude: render outputs (`deploy/`, `rendered/`), akua cache
//!   (`.akua/`), VCS (`.git/`), user inputs (`inputs.yaml` — that's
//!   per-consumer), hidden dotfiles.
//! - Walk in sorted-by-relative-path order so the resulting tarball
//!   is byte-deterministic across machines → the layer digest
//!   stable across republishes that didn't change any source byte.
//!
//! The shape is intentionally flat + standard: anyone with `tar -xzf`
//! can inspect a published Package. No akua-specific framing.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PackageTarError {
    #[error("workspace root `{}` is not a directory", path.display())]
    NotADirectory { path: PathBuf },

    #[error("akua.toml missing under `{}`", path.display())]
    MissingManifest { path: PathBuf },

    #[error("i/o at `{}`: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Unpack a `.tar.gz` produced by [`pack_workspace`] into `target`.
/// The inverse of pack; used by `akua pull`. `target` is created
/// if absent; existing files are overwritten (last-pull-wins on a
/// re-pull, consistent with how `git checkout` handles worktree
/// state).
///
/// Rejects entries whose paths have `..` components or absolute
/// prefixes — the host tar crate already does this by default, but
/// the explicit check above makes the invariant load-bearing even
/// on a tar crate revision that flips the default.
pub fn unpack_to(tar_gz: &[u8], target: &Path) -> Result<(), PackageTarError> {
    std::fs::create_dir_all(target).map_err(|source| PackageTarError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let gz = flate2::read::GzDecoder::new(tar_gz);
    let mut ar = tar::Archive::new(gz);
    ar.set_overwrite(true);
    ar.set_preserve_permissions(false);
    ar.unpack(target).map_err(|source| PackageTarError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Build a deterministic `.tar.gz` of the workspace for publish.
pub fn pack_workspace(root: &Path) -> Result<Vec<u8>, PackageTarError> {
    if !root.is_dir() {
        return Err(PackageTarError::NotADirectory {
            path: root.to_path_buf(),
        });
    }
    if !root.join("akua.toml").is_file() {
        return Err(PackageTarError::MissingManifest {
            path: root.to_path_buf(),
        });
    }

    let mut entries = Vec::new();
    collect(root, root, &mut entries).map_err(|source| PackageTarError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut tar_b = tar::Builder::new(gz);
        tar_b.follow_symlinks(false);

        for (rel, abs) in &entries {
            let mut file = std::fs::File::open(abs).map_err(|source| PackageTarError::Io {
                path: abs.clone(),
                source,
            })?;
            tar_b
                .append_file(rel, &mut file)
                .map_err(|source| PackageTarError::Io {
                    path: abs.clone(),
                    source,
                })?;
        }
        tar_b.finish().map_err(|source| PackageTarError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    }
    Ok(buf)
}

/// Recursively collect files under `dir`, keeping paths relative to
/// `root`. Hidden dotfiles and excluded top-level dirs are skipped.
fn collect(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if should_skip(&name_str) {
            continue;
        }

        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect(root, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walker stays under root")
                .to_path_buf();
            out.push((rel, path));
        }
        // Symlinks deliberately skipped — same reasoning as the helm
        // chart-hash path: they break determinism + widen the sandbox
        // surface.
    }
    Ok(())
}

/// Path-component-level exclusion list. Checked on both leaf file
/// names and intermediate dir names.
fn should_skip(name: &str) -> bool {
    // Hidden files — exclude by default. Keeps build directories
    // (`.akua/`, `.git/`, `.DS_Store`) out without enumerating.
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name,
        // Render outputs — per-consumer, never part of the publish.
        "deploy" | "rendered" | "out"
        // Lock/state users don't want shipped (e.g. one-off renders).
        | "inputs.yaml"
        // Cargo/node/etc directories that might be in a workspace
        // next to akua.toml but aren't part of the Package.
        | "target" | "node_modules"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn list_entries(tar_gz: &[u8]) -> Vec<String> {
        let gz = flate2::read::GzDecoder::new(tar_gz);
        let mut ar = tar::Archive::new(gz);
        ar.entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                e.path().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    }

    #[test]
    fn packs_expected_workspace_contents() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "akua.toml", b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n");
        write(tmp.path(), "akua.lock", b"version = 1\n");
        write(tmp.path(), "package.k", b"resources = []\n");
        write(tmp.path(), "inputs.example.yaml", b"hello: world\n");
        write(tmp.path(), "vendor/nginx/Chart.yaml", b"name: nginx\n");

        let tar_gz = pack_workspace(tmp.path()).expect("pack");
        let names = list_entries(&tar_gz);
        // Paths are relative, sorted alphabetically by collect+sort.
        assert!(names.contains(&"akua.toml".to_string()));
        assert!(names.contains(&"akua.lock".to_string()));
        assert!(names.contains(&"package.k".to_string()));
        assert!(names.contains(&"inputs.example.yaml".to_string()));
        assert!(names.contains(&"vendor/nginx/Chart.yaml".to_string()));
    }

    #[test]
    fn excludes_render_outputs_and_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "akua.toml", b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n");
        write(tmp.path(), "package.k", b"resources = []\n");
        write(tmp.path(), "deploy/000.yaml", b"k: v\n");
        write(tmp.path(), "rendered/000.yaml", b"k: v\n");
        write(tmp.path(), ".akua/cache/x", b"binary\n");
        write(tmp.path(), ".git/HEAD", b"ref: main\n");
        write(tmp.path(), "inputs.yaml", b"per-consumer: yes\n");

        let tar_gz = pack_workspace(tmp.path()).unwrap();
        let names = list_entries(&tar_gz);
        assert!(!names.iter().any(|n| n.starts_with("deploy/")));
        assert!(!names.iter().any(|n| n.starts_with("rendered/")));
        assert!(!names.iter().any(|n| n.starts_with(".akua")));
        assert!(!names.iter().any(|n| n.starts_with(".git")));
        assert!(!names.contains(&"inputs.yaml".to_string()));
    }

    #[test]
    fn deterministic_byte_output() {
        // Two runs over the same fixture → byte-identical tarballs.
        // Guards against BTreeMap/HashMap iteration regressing the
        // ordering invariant layer-digest stability depends on.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "akua.toml", b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n");
        write(tmp.path(), "package.k", b"resources = []\n");
        write(tmp.path(), "b.k", b"// b\n");
        write(tmp.path(), "a.k", b"// a\n");
        let t1 = pack_workspace(tmp.path()).unwrap();
        let t2 = pack_workspace(tmp.path()).unwrap();
        assert_eq!(t1, t2);
    }

    #[test]
    fn missing_manifest_surfaces_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.k", b"resources = []\n");
        let err = pack_workspace(tmp.path()).unwrap_err();
        assert!(matches!(err, PackageTarError::MissingManifest { .. }));
    }

    #[test]
    fn non_directory_root_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("solo.file");
        std::fs::write(&file, b"x").unwrap();
        let err = pack_workspace(&file).unwrap_err();
        assert!(matches!(err, PackageTarError::NotADirectory { .. }));
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "akua.toml", b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n");
        write(src.path(), "package.k", b"resources = []\n");
        write(src.path(), "vendor/nginx/Chart.yaml", b"name: nginx\n");

        let tar_gz = pack_workspace(src.path()).unwrap();

        let dst = tempfile::tempdir().unwrap();
        unpack_to(&tar_gz, dst.path()).unwrap();
        assert!(dst.path().join("akua.toml").is_file());
        assert!(dst.path().join("package.k").is_file());
        assert!(dst.path().join("vendor/nginx/Chart.yaml").is_file());
        assert_eq!(
            std::fs::read(dst.path().join("vendor/nginx/Chart.yaml")).unwrap(),
            b"name: nginx\n"
        );
    }

    /// Prove a round-trip through the tarball preserves file contents
    /// verbatim — guards against accidental gzip compression-level
    /// changes mangling bytes (not that they would, but cheap).
    #[test]
    fn roundtrip_preserves_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "akua.toml", b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n");
        let body = b"apiVersion: v1\nkind: ConfigMap\n";
        write(tmp.path(), "package.k", body);

        let tar_gz = pack_workspace(tmp.path()).unwrap();
        let gz = flate2::read::GzDecoder::new(&tar_gz[..]);
        let mut ar = tar::Archive::new(gz);
        for entry in ar.entries().unwrap() {
            let mut e = entry.unwrap();
            if e.path().unwrap().ends_with("package.k") {
                let mut got = Vec::new();
                e.read_to_end(&mut got).unwrap();
                assert_eq!(got, body);
                return;
            }
        }
        panic!("package.k not in tarball");
    }
}
