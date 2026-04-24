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
/// The `tar` crate strips `..` components and rejects absolute paths
/// by default (since ~0.4.30), so a crafted archive can't write
/// outside `target`. We rely on that invariant rather than a
/// pre-extraction pass — a tar-crate regression would be flagged by
/// the crate's own test suite before it reached us.
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
/// Convenience wrapper that doesn't vendor any deps — use
/// [`pack_workspace_with_vendored_deps`] when the publisher wants
/// the pulled artifact to render offline.
pub fn pack_workspace(root: &Path) -> Result<Vec<u8>, PackageTarError> {
    pack_workspace_with_vendored_deps(root, &[])
}

/// Like [`pack_workspace`] but also embeds each entry in
/// `vendored` under `.akua/vendor/<name>/` in the output tarball.
/// Used by `akua publish` to include OCI / git dep chart trees
/// alongside the Package source so `akua pull` lands a workspace
/// that renders without network access.
///
/// `vendored` is a list of `(dep_name, chart_dir)` pairs. The
/// content of each `chart_dir` is copied in recursively — same
/// walk + skip rules as `pack_workspace` itself.
///
/// Path deps already live in the workspace tree (usually under
/// `vendor/`) and are packed via the normal walk; don't include
/// them here or they'll double-vendor.
pub fn pack_workspace_with_vendored_deps(
    root: &Path,
    vendored: &[(String, PathBuf)],
) -> Result<Vec<u8>, PackageTarError> {
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

    let entries = crate::walk::collect_files(root, |name| !should_skip_file(name))
        .map_err(|source| PackageTarError::Io {
            path: root.to_path_buf(),
            source,
        })?;

    // Collect vendored-dep files upfront so the tarball is packed in
    // one sorted stream. Each (dep_name, chart_dir) contributes a
    // sub-tree at `.akua/vendor/<dep_name>/`. Skip rules match the
    // workspace walk — no `target/` / `node_modules/` / hidden dirs
    // leak into published artifacts even if a chart cache contains
    // them.
    let mut vendor_entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (name, chart_dir) in vendored {
        let pairs = crate::walk::collect_files(chart_dir, |_| true).map_err(|source| {
            PackageTarError::Io {
                path: chart_dir.clone(),
                source,
            }
        })?;
        for (rel, abs) in pairs {
            let tar_path = PathBuf::from(".akua/vendor").join(name).join(rel);
            vendor_entries.push((tar_path, abs));
        }
    }
    vendor_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut tar_b = tar::Builder::new(gz);
        tar_b.follow_symlinks(false);

        for (rel, abs) in entries.iter().chain(vendor_entries.iter()) {
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

/// Compute the `sha256:<hex>` digest of a packed tarball. Same
/// shape as the OCI layer digest the registry would assign — so a
/// locally-packed artifact and its published counterpart carry
/// matching identifiers. Pin this in downstream automation.
pub fn layer_digest(tar_gz: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", crate::hex::hex_encode(&Sha256::digest(tar_gz)))
}

/// Summary of a packed tarball's contents — read in-memory without
/// unpacking to disk. Used by `akua inspect --tarball` for operator
/// triage ("what's in this thing?") of artifacts that came over
/// air-gap transfer.
///
/// `package_name` / `version` / `edition` mirror `[package]` in
/// `akua.toml`. They're `None` when the tarball doesn't contain a
/// manifest at its root (malformed, but don't hard-fail — surface
/// what we *can* read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarballInspection {
    pub layer_digest: String,
    pub compressed_size_bytes: u64,
    pub uncompressed_size_bytes: u64,
    pub file_count: usize,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub package_edition: Option<String>,
    /// Sorted names under `.akua/vendor/`. Empty when the tarball
    /// was packed with `--no-vendor` or carries no OCI/git deps.
    pub vendored_deps: Vec<String>,
}

/// Inspect a packed tarball without unpacking to disk. Reads the
/// archive twice: once to sum sizes + list entries, once to extract
/// `akua.toml`. Both passes stream over the same bytes; no
/// filesystem i/o beyond decompression buffers.
pub fn inspect(tar_gz: &[u8]) -> Result<TarballInspection, PackageTarError> {
    use std::collections::BTreeSet;
    use std::io::Read;

    let mut file_count: usize = 0;
    let mut uncompressed: u64 = 0;
    let mut vendored: BTreeSet<String> = BTreeSet::new();
    let mut manifest_text: Option<String> = None;

    let gz = flate2::read::GzDecoder::new(tar_gz);
    let mut ar = tar::Archive::new(gz);
    let entries = ar.entries().map_err(|source| PackageTarError::Io {
        path: PathBuf::from("<tarball>"),
        source,
    })?;

    for entry in entries {
        let mut e = entry.map_err(|source| PackageTarError::Io {
            path: PathBuf::from("<tarball>"),
            source,
        })?;
        if !e.header().entry_type().is_file() {
            continue;
        }
        file_count += 1;
        uncompressed += e.size();
        let path = e.path().map(|p| p.into_owned()).unwrap_or_default();

        // Top-level .akua/vendor/<name>/... — capture just <name>.
        if let Ok(stripped) = path.strip_prefix(".akua/vendor") {
            if let Some(name) = stripped.components().next() {
                if let Some(s) = name.as_os_str().to_str() {
                    vendored.insert(s.to_string());
                }
            }
        }

        // Root-level akua.toml → read into memory for parsing below.
        if path == std::path::Path::new("akua.toml") {
            let mut buf = String::new();
            e.read_to_string(&mut buf)
                .map_err(|source| PackageTarError::Io {
                    path: path.clone(),
                    source,
                })?;
            manifest_text = Some(buf);
        }
    }

    let (package_name, package_version, package_edition) = match manifest_text
        .as_deref()
        .and_then(|txt| crate::mod_file::AkuaManifest::parse(txt).ok())
    {
        Some(m) => (
            Some(m.package.name),
            Some(m.package.version),
            Some(m.package.edition),
        ),
        None => (None, None, None),
    };

    Ok(TarballInspection {
        layer_digest: layer_digest(tar_gz),
        compressed_size_bytes: tar_gz.len() as u64,
        uncompressed_size_bytes: uncompressed,
        file_count,
        package_name,
        package_version,
        package_edition,
        vendored_deps: vendored.into_iter().collect(),
    })
}

/// Per-file exclusions unique to publish. Directory-level skips
/// (`deploy`, `target`, hidden dirs) are handled by the shared
/// [`crate::walk`] module.
fn should_skip_file(name: &str) -> bool {
    // Hidden files — publish-time we want `.gitignore` / `.DS_Store`
    // out even when they land in an otherwise-kept dir.
    if name.starts_with('.') {
        return true;
    }
    // Per-consumer / one-off state never part of a publish.
    matches!(name, "inputs.yaml")
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
    fn pack_embeds_vendored_deps_under_akua_vendor() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "akua.toml",
            b"[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"akua.dev/v1alpha1\"\n",
        );
        write(tmp.path(), "package.k", b"resources = []\n");

        // Vendored chart lives outside the workspace — simulates
        // the `$XDG_CACHE_HOME/akua/oci/sha256/<hex>/podinfo/`
        // layout `akua publish` would pass in.
        let cache = tempfile::tempdir().unwrap();
        let chart = cache.path().join("nginx");
        std::fs::create_dir_all(chart.join("templates")).unwrap();
        std::fs::write(chart.join("Chart.yaml"), b"apiVersion: v2\nname: nginx\n").unwrap();
        std::fs::write(chart.join("templates/cm.yaml"), b"apiVersion: v1\n").unwrap();

        let vendored = vec![("nginx".to_string(), chart.clone())];
        let tar_gz = pack_workspace_with_vendored_deps(tmp.path(), &vendored).unwrap();
        let names = list_entries(&tar_gz);

        assert!(names.contains(&".akua/vendor/nginx/Chart.yaml".to_string()), "names: {names:?}");
        assert!(
            names.contains(&".akua/vendor/nginx/templates/cm.yaml".to_string()),
            "names: {names:?}"
        );
        // Workspace files still present.
        assert!(names.contains(&"akua.toml".to_string()));
        assert!(names.contains(&"package.k".to_string()));
    }

    #[test]
    fn pack_with_no_vendored_deps_matches_plain_pack() {
        // `pack_workspace(root)` and `pack_workspace_with_vendored_deps(root, &[])`
        // must produce byte-identical output — the vendor API is
        // strictly additive.
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "akua.toml",
            b"[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"akua.dev/v1alpha1\"\n",
        );
        write(tmp.path(), "package.k", b"resources = []\n");
        let a = pack_workspace(tmp.path()).unwrap();
        let b = pack_workspace_with_vendored_deps(tmp.path(), &[]).unwrap();
        assert_eq!(a, b);
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

    #[test]
    fn inspect_reads_akua_toml_fields_and_counts_entries() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "akua.toml",
            b"[package]\nname = \"demo\"\nversion = \"1.2.3\"\nedition = \"akua.dev/v1alpha1\"\n",
        );
        write(tmp.path(), "package.k", b"resources = []\n");
        write(tmp.path(), "inputs.example.yaml", b"hello: world\n");

        let tar_gz = pack_workspace(tmp.path()).unwrap();
        let got = inspect(&tar_gz).unwrap();

        assert_eq!(got.package_name.as_deref(), Some("demo"));
        assert_eq!(got.package_version.as_deref(), Some("1.2.3"));
        assert_eq!(got.package_edition.as_deref(), Some("akua.dev/v1alpha1"));
        assert_eq!(got.file_count, 3);
        assert!(got.uncompressed_size_bytes > 0);
        assert_eq!(got.compressed_size_bytes, tar_gz.len() as u64);
        assert!(got.layer_digest.starts_with("sha256:"));
        assert!(got.vendored_deps.is_empty());
    }

    #[test]
    fn inspect_surfaces_vendored_deps_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "akua.toml",
            b"[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        );
        write(tmp.path(), "package.k", b"resources = []\n");

        // Build two vendored chart dirs outside the workspace and
        // pack them in via pack_workspace_with_vendored_deps.
        let chart_root = tempfile::tempdir().unwrap();
        let redis_dir = chart_root.path().join("redis");
        let nginx_dir = chart_root.path().join("nginx");
        std::fs::create_dir_all(&redis_dir).unwrap();
        std::fs::create_dir_all(&nginx_dir).unwrap();
        std::fs::write(redis_dir.join("Chart.yaml"), b"name: redis\n").unwrap();
        std::fs::write(nginx_dir.join("Chart.yaml"), b"name: nginx\n").unwrap();

        let tar_gz = pack_workspace_with_vendored_deps(
            tmp.path(),
            &[
                ("redis".to_string(), redis_dir),
                ("nginx".to_string(), nginx_dir),
            ],
        )
        .unwrap();

        let got = inspect(&tar_gz).unwrap();
        assert_eq!(got.vendored_deps, vec!["nginx".to_string(), "redis".to_string()]);
    }

    #[test]
    fn inspect_tolerates_missing_manifest() {
        // Build an ad-hoc tarball by hand so pack_workspace's
        // MissingManifest check doesn't short-circuit us — inspect
        // must degrade gracefully on malformed bytes from arbitrary
        // sources.
        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut b = tar::Builder::new(gz);
            let bytes = b"resources = []\n";
            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(bytes.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            b.append_data(&mut hdr, "package.k", &bytes[..]).unwrap();
            b.finish().unwrap();
        }

        let got = inspect(&buf).unwrap();
        assert_eq!(got.package_name, None);
        assert_eq!(got.package_version, None);
        assert_eq!(got.file_count, 1);
    }
}
