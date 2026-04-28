//! Recursive two-directory structural diff.
//!
//! Walks two roots, pairs files by relative path, hashes contents for
//! the overlap. Used by `akua diff` to compare two rendered outputs
//! (e.g. deploy/ before and after a Package edit).
//!
//! Non-file entries (symlinks, sockets, fifos) are reported in
//! [`DirDiff::skipped`] rather than hashed — any change in
//! their *file type* is treated as a change, but symlink target
//! changes aren't inspected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;

crate::contract_type! {
/// Structural diff between two directory trees. All paths are
/// relative to the respective root; sorted alphabetically for
/// deterministic output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirDiff {
    /// Files present in the `after` root but not the `before` root.
    pub added: Vec<PathBuf>,

    /// Files present in the `before` root but not the `after` root.
    pub removed: Vec<PathBuf>,

    /// Files present in both but with differing sha256 content.
    pub changed: Vec<FileChange>,

    /// Files present in both with identical sha256 content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unchanged: Vec<PathBuf>,

    /// Entries that aren't regular files (symlinks, fifos, sockets…).
    /// Surfaced so the caller knows the diff isn't exhaustive, without
    /// the diff itself trying to semantically compare those kinds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<PathBuf>,
}
}

crate::contract_type! {
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: PathBuf,

    /// `sha256:<hex>` of the `before` file's contents.
    pub before: String,

    /// `sha256:<hex>` of the `after` file's contents.
    pub after: String,
}
}

impl DirDiff {
    /// `true` when before and after trees hold the same set of files
    /// with matching contents (ignoring `skipped` entries).
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DirDiffError {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("`{path}` is not a directory")]
    NotDir { path: PathBuf },
}

/// Compare two directory trees. Walks both in full; the hashing is
/// O(sum of file sizes in the intersection).
pub fn diff(before: &Path, after: &Path) -> Result<DirDiff, DirDiffError> {
    let before_files = collect(before)?;
    let after_files = collect(after)?;

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();
    let mut skipped: Vec<PathBuf> = before_files
        .skipped
        .iter()
        .chain(after_files.skipped.iter())
        .cloned()
        .collect();
    skipped.sort();
    skipped.dedup();

    let mut all_keys: Vec<&PathBuf> = before_files
        .files
        .keys()
        .chain(after_files.files.keys())
        .collect();
    all_keys.sort();
    all_keys.dedup();

    for rel in all_keys {
        match (before_files.files.get(rel), after_files.files.get(rel)) {
            (Some(b), Some(a)) => {
                let before_hash = hash_file(&before.join(rel), b.size)?;
                let after_hash = hash_file(&after.join(rel), a.size)?;
                if before_hash == after_hash {
                    unchanged.push(rel.clone());
                } else {
                    changed.push(FileChange {
                        path: rel.clone(),
                        before: format!("sha256:{before_hash}"),
                        after: format!("sha256:{after_hash}"),
                    });
                }
            }
            (Some(_), None) => removed.push(rel.clone()),
            (None, Some(_)) => added.push(rel.clone()),
            (None, None) => unreachable!("key came from either map"),
        }
    }

    Ok(DirDiff {
        added,
        removed,
        changed,
        unchanged,
        skipped,
    })
}

/// Diff two `{relative_path → sha256-hex}` maps. Pure function — no
/// filesystem access; the caller (typically `@akua-dev/sdk`'s `diff`
/// verb or a WASM host that lacks `std::fs`) pre-walks the trees and
/// hashes each file.
///
/// Hex values MUST be bare lowercase without the `sha256:` prefix —
/// the prefix is added when wrapping a change into [`FileChange`].
/// Keys are relative paths; they participate in the sort + dedup
/// exactly the way [`diff`] treats them.
pub fn diff_manifests(
    before: &std::collections::BTreeMap<PathBuf, String>,
    after: &std::collections::BTreeMap<PathBuf, String>,
) -> DirDiff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();

    let mut all_keys: Vec<&PathBuf> = before.keys().chain(after.keys()).collect();
    all_keys.sort();
    all_keys.dedup();

    for rel in all_keys {
        match (before.get(rel), after.get(rel)) {
            (Some(b), Some(a)) => {
                if b == a {
                    unchanged.push(rel.clone());
                } else {
                    changed.push(FileChange {
                        path: rel.clone(),
                        before: format!("sha256:{b}"),
                        after: format!("sha256:{a}"),
                    });
                }
            }
            (Some(_), None) => removed.push(rel.clone()),
            (None, Some(_)) => added.push(rel.clone()),
            (None, None) => unreachable!("key came from either map"),
        }
    }

    DirDiff {
        added,
        removed,
        changed,
        unchanged,
        skipped: Vec::new(),
    }
}

struct FileMeta {
    size: u64,
}

struct CollectResult {
    files: BTreeMap<PathBuf, FileMeta>,
    skipped: Vec<PathBuf>,
}

/// Walk `root` recursively, collecting regular files by relative path.
/// Follows no symlinks — they're reported in `skipped`.
fn collect(root: &Path) -> Result<CollectResult, DirDiffError> {
    let metadata = std::fs::symlink_metadata(root).map_err(|source| DirDiffError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(DirDiffError::NotDir {
            path: root.to_path_buf(),
        });
    }

    let mut files = BTreeMap::new();
    let mut skipped = Vec::new();
    walk(root, root, &mut files, &mut skipped)?;
    Ok(CollectResult { files, skipped })
}

fn walk(
    root: &Path,
    cursor: &Path,
    files: &mut BTreeMap<PathBuf, FileMeta>,
    skipped: &mut Vec<PathBuf>,
) -> Result<(), DirDiffError> {
    let entries = std::fs::read_dir(cursor).map_err(|source| DirDiffError::Io {
        path: cursor.to_path_buf(),
        source,
    })?;
    let mut children: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    children.sort();

    for path in children {
        let meta = std::fs::symlink_metadata(&path).map_err(|source| DirDiffError::Io {
            path: path.clone(),
            source,
        })?;
        if meta.is_dir() {
            walk(root, &path, files, skipped)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walk stays under root")
                .to_path_buf();
            files.insert(rel, FileMeta { size: meta.len() });
        } else {
            // Symlinks, sockets, fifos — record and move on.
            let rel = path
                .strip_prefix(root)
                .expect("walk stays under root")
                .to_path_buf();
            skipped.push(rel);
        }
    }
    Ok(())
}

fn hash_file(path: &Path, _hint_size: u64) -> Result<String, DirDiffError> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path).map_err(|source| DirDiffError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    std::io::copy(&mut file, &mut hasher).map_err(|source| DirDiffError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex_encode(&hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn diff_manifests_classifies_changes() {
        let mut before = BTreeMap::new();
        before.insert(PathBuf::from("a.yaml"), "aaaaaa".to_string());
        before.insert(PathBuf::from("b.yaml"), "bbbbbb".to_string());
        before.insert(PathBuf::from("gone.yaml"), "gone".to_string());
        let mut after = BTreeMap::new();
        after.insert(PathBuf::from("a.yaml"), "aaaaaa".to_string());
        after.insert(PathBuf::from("b.yaml"), "bbbbb2".to_string());
        after.insert(PathBuf::from("new.yaml"), "newnew".to_string());

        let d = diff_manifests(&before, &after);
        assert_eq!(d.unchanged, vec![PathBuf::from("a.yaml")]);
        assert_eq!(d.added, vec![PathBuf::from("new.yaml")]);
        assert_eq!(d.removed, vec![PathBuf::from("gone.yaml")]);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].path, PathBuf::from("b.yaml"));
        assert_eq!(d.changed[0].before, "sha256:bbbbbb");
        assert_eq!(d.changed[0].after, "sha256:bbbbb2");
        assert!(!d.is_clean());
    }

    #[test]
    fn diff_manifests_empty_maps_are_clean() {
        let empty: BTreeMap<PathBuf, String> = BTreeMap::new();
        let d = diff_manifests(&empty, &empty);
        assert!(d.is_clean());
        assert!(d.added.is_empty() && d.removed.is_empty() && d.changed.is_empty());
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn identical_trees_produce_clean_diff() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "a.yaml", "content-a");
        write(before.path(), "nested/b.yaml", "content-b");
        write(after.path(), "a.yaml", "content-a");
        write(after.path(), "nested/b.yaml", "content-b");

        let d = diff(before.path(), after.path()).unwrap();
        assert!(d.is_clean());
        assert_eq!(d.unchanged.len(), 2);
    }

    #[test]
    fn detects_added_files() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "a.yaml", "x");
        write(after.path(), "a.yaml", "x");
        write(after.path(), "new.yaml", "fresh");

        let d = diff(before.path(), after.path()).unwrap();
        assert_eq!(d.added, vec![PathBuf::from("new.yaml")]);
        assert!(d.removed.is_empty());
        assert!(d.changed.is_empty());
    }

    #[test]
    fn detects_removed_files() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "gone.yaml", "x");
        write(before.path(), "stay.yaml", "y");
        write(after.path(), "stay.yaml", "y");

        let d = diff(before.path(), after.path()).unwrap();
        assert_eq!(d.removed, vec![PathBuf::from("gone.yaml")]);
        assert_eq!(d.unchanged, vec![PathBuf::from("stay.yaml")]);
    }

    #[test]
    fn detects_changed_contents_via_hash() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "x.yaml", "before");
        write(after.path(), "x.yaml", "after");

        let d = diff(before.path(), after.path()).unwrap();
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].path, PathBuf::from("x.yaml"));
        assert!(d.changed[0].before.starts_with("sha256:"));
        assert_ne!(d.changed[0].before, d.changed[0].after);
    }

    #[test]
    fn nested_paths_preserve_structure() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "deep/a/b/c.yaml", "x");
        write(after.path(), "deep/a/b/c.yaml", "y");

        let d = diff(before.path(), after.path()).unwrap();
        assert_eq!(d.changed[0].path, PathBuf::from("deep/a/b/c.yaml"));
    }

    #[test]
    fn empty_dirs_produce_empty_diff() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        let d = diff(before.path(), after.path()).unwrap();
        assert!(d.is_clean());
        assert!(d.unchanged.is_empty());
    }

    #[test]
    fn output_ordering_is_deterministic() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "z.yaml", "x");
        write(before.path(), "a.yaml", "x");
        write(before.path(), "m.yaml", "x");
        write(after.path(), "z.yaml", "y");
        write(after.path(), "a.yaml", "y");
        write(after.path(), "m.yaml", "y");

        let d = diff(before.path(), after.path()).unwrap();
        let paths: Vec<_> = d.changed.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("a.yaml"),
                PathBuf::from("m.yaml"),
                PathBuf::from("z.yaml"),
            ]
        );
    }

    #[test]
    fn not_a_directory_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file.txt");
        fs::write(&file, "hi").unwrap();
        let err = diff(&file, tmp.path()).unwrap_err();
        assert!(matches!(err, DirDiffError::NotDir { .. }));
    }

    #[test]
    fn missing_directory_surfaces_io_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("no-such");
        let err = diff(&missing, tmp.path()).unwrap_err();
        assert!(matches!(err, DirDiffError::Io { .. }));
    }
}
