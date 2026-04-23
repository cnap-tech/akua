//! Inventory + housekeeping for the content-addressed caches that
//! `oci_fetcher` + `git_fetcher` populate.
//!
//! Layouts on disk:
//!
//! - `$XDG_CACHE_HOME/akua/oci/sha256/<hex>/<chart>/` — one per
//!   unique OCI blob digest. Populated on first `akua add` /
//!   `akua render` against the OCI ref, reused on subsequent calls.
//! - `$XDG_CACHE_HOME/akua/git/repos/<sanitized-url>.git/` — bare
//!   clones, one per remote.
//! - `$XDG_CACHE_HOME/akua/git/checkouts/<commit-sha>/` — worktree
//!   per commit.
//!
//! Ops on ephemeral CI runners want: how big is it? What's in it?
//! Can I reclaim disk? This module answers those three questions
//! without cycling through the fetcher abstractions.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Single cache entry. `size_bytes` is the recursive tree size;
/// computing it walks the entry once per `list()` call — cheap
/// enough for typical cache sizes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CacheEntry {
    /// `"oci-blob"`, `"git-repo"`, or `"git-checkout"`.
    pub kind: &'static str,
    /// Content-addressed identifier:
    /// - OCI blob → `"sha256:<hex>"`
    /// - Git repo → sanitized URL directory name (stable across
    ///   invocations)
    /// - Git checkout → `"git:<40-hex>"`
    pub id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Aggregate summary of a cache listing. Stable JSON shape —
/// callers + agents pin these fields.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CacheInventory {
    pub oci_root: PathBuf,
    pub git_root: PathBuf,
    pub entries: Vec<CacheEntry>,
    pub total_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("i/o at `{}`: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// List every cache entry under the default roots. Missing roots
/// → `entries: []`, `total_bytes: 0`. Stable across empty caches
/// so automation can unconditionally parse.
pub fn list() -> Result<CacheInventory, CacheError> {
    let oci_root = default_cache_root("oci");
    let git_root = default_cache_root("git");
    list_at(&oci_root, &git_root)
}

/// Same as [`list`] but with explicit roots — used by tests.
pub fn list_at(oci_root: &Path, git_root: &Path) -> Result<CacheInventory, CacheError> {
    let mut entries = Vec::new();
    collect_oci(oci_root, &mut entries)?;
    collect_git(git_root, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let total_bytes = entries.iter().map(|e| e.size_bytes).sum();
    Ok(CacheInventory {
        oci_root: oci_root.to_path_buf(),
        git_root: git_root.to_path_buf(),
        entries,
        total_bytes,
    })
}

/// What `clear()` did. Count of removed entries + freed bytes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CacheClearReport {
    pub removed: usize,
    pub freed_bytes: u64,
}

/// Wipe caches. `ClearScope` picks OCI, git, or both. Safe on
/// absent roots — removes nothing and reports zeros.
pub fn clear(scope: ClearScope) -> Result<CacheClearReport, CacheError> {
    let oci_root = default_cache_root("oci");
    let git_root = default_cache_root("git");
    clear_at(scope, &oci_root, &git_root)
}

/// Same as [`clear`] but with explicit roots — used by tests.
pub fn clear_at(
    scope: ClearScope,
    oci_root: &Path,
    git_root: &Path,
) -> Result<CacheClearReport, CacheError> {
    let mut report = CacheClearReport {
        removed: 0,
        freed_bytes: 0,
    };
    if scope.includes_oci() {
        let mut entries = Vec::new();
        collect_oci(oci_root, &mut entries)?;
        reap(oci_root, &entries, &mut report)?;
    }
    if scope.includes_git() {
        let mut entries = Vec::new();
        collect_git(git_root, &mut entries)?;
        reap(git_root, &entries, &mut report)?;
    }
    Ok(report)
}

/// Sum the entry sizes into the report, then `rm -rf` the root.
/// No-op when the root is absent. Avoids a second recursive walk by
/// reusing the inventory we already did.
fn reap(
    root: &Path,
    entries: &[CacheEntry],
    report: &mut CacheClearReport,
) -> Result<(), CacheError> {
    if !root.exists() {
        return Ok(());
    }
    for e in entries {
        report.removed += 1;
        report.freed_bytes += e.size_bytes;
    }
    std::fs::remove_dir_all(root).map_err(|source| CacheError::Io {
        path: root.to_path_buf(),
        source,
    })
}

/// Which cache branches to reap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearScope {
    Both,
    OciOnly,
    GitOnly,
}

impl ClearScope {
    fn includes_oci(self) -> bool {
        matches!(self, ClearScope::Both | ClearScope::OciOnly)
    }
    fn includes_git(self) -> bool {
        matches!(self, ClearScope::Both | ClearScope::GitOnly)
    }

    /// Stable JSON label. Pinned by consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            ClearScope::Both => "both",
            ClearScope::OciOnly => "oci",
            ClearScope::GitOnly => "git",
        }
    }
}

/// Default cache root. `$XDG_CACHE_HOME/akua/<subdir>` →
/// `$HOME/.cache/akua/<subdir>` → `./.akua/cache/<subdir>`.
/// Matches the fallback chain in `chart_resolver`.
pub fn default_cache_root(subdir: &str) -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("akua").join(subdir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".cache/akua").join(subdir);
        }
    }
    PathBuf::from(".akua/cache").join(subdir)
}

// --- OCI inventory --------------------------------------------------------

/// OCI layout: `<root>/sha256/<hex>/<chart-dir-name>/`. One entry
/// per `<hex>`; the chart-dir-name is part of the path but not the
/// identifier.
fn collect_oci(root: &Path, out: &mut Vec<CacheEntry>) -> Result<(), CacheError> {
    let sha_root = root.join("sha256");
    let Ok(rd) = std::fs::read_dir(&sha_root) else {
        return Ok(());
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(hex) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let size_bytes = dir_size(&path).map_err(|source| CacheError::Io {
            path: path.clone(),
            source,
        })?;
        out.push(CacheEntry {
            kind: "oci-blob",
            id: format!("sha256:{hex}"),
            path,
            size_bytes,
        });
    }
    Ok(())
}

// --- Git inventory --------------------------------------------------------

/// Git cache has two children: `repos/<sanitized-url>.git/` and
/// `checkouts/<commit-sha>/`. Both are surfaced so `akua cache list`
/// gives an accurate disk picture.
fn collect_git(root: &Path, out: &mut Vec<CacheEntry>) -> Result<(), CacheError> {
    collect_git_subdir(&root.join("repos"), "git-repo", None, out)?;
    collect_git_subdir(&root.join("checkouts"), "git-checkout", Some("git:"), out)?;
    Ok(())
}

fn collect_git_subdir(
    dir: &Path,
    kind: &'static str,
    id_prefix: Option<&str>,
    out: &mut Vec<CacheEntry>,
) -> Result<(), CacheError> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let size_bytes = dir_size(&path).map_err(|source| CacheError::Io {
            path: path.clone(),
            source,
        })?;
        let id = match id_prefix {
            Some(prefix) => format!("{prefix}{name}"),
            None => name.to_string(),
        };
        out.push(CacheEntry {
            kind,
            id,
            path,
            size_bytes,
        });
    }
    Ok(())
}

// --- Size walk ------------------------------------------------------------

/// Recursive sum of file sizes under `dir`. Symlinks skipped (same
/// reasoning as the other walkers — they break determinism +
/// inflate the size count with off-tree targets).
fn dir_size(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    walk_for_size(dir, &mut total)?;
    Ok(total)
}

fn walk_for_size(dir: &Path, total: &mut u64) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_for_size(&entry.path(), total)?;
        } else if ft.is_file() {
            *total += entry.metadata()?.len();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn empty_roots_yield_empty_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        let inv = list_at(&oci, &git).unwrap();
        assert!(inv.entries.is_empty());
        assert_eq!(inv.total_bytes, 0);
    }

    #[test]
    fn lists_oci_entries_with_size() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        write(&oci.join("sha256/abc123/nginx/Chart.yaml"), b"apiVersion: v2\n");
        write(&oci.join("sha256/abc123/nginx/values.yaml"), b"foo: bar\n");
        write(&oci.join("sha256/def456/other/file"), b"x");

        let inv = list_at(&oci, &git).unwrap();
        assert_eq!(inv.entries.len(), 2);
        let abc = inv.entries.iter().find(|e| e.id == "sha256:abc123").unwrap();
        assert_eq!(abc.kind, "oci-blob");
        assert!(abc.size_bytes > 0);
        assert_eq!(
            inv.total_bytes,
            inv.entries.iter().map(|e| e.size_bytes).sum::<u64>()
        );
    }

    #[test]
    fn lists_git_repos_and_checkouts_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        write(&git.join("repos/github.com_foo_bar.git/HEAD"), b"ref\n");
        write(&git.join("checkouts/deadbeef/Chart.yaml"), b"x\n");

        let inv = list_at(&oci, &git).unwrap();
        assert!(inv.entries.iter().any(|e| e.kind == "git-repo"));
        assert!(inv
            .entries
            .iter()
            .any(|e| e.kind == "git-checkout" && e.id == "git:deadbeef"));
    }

    #[test]
    fn clear_both_removes_everything_and_reports_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        write(&oci.join("sha256/abc/chart/x"), b"123456");
        write(&git.join("checkouts/abc/x"), b"789");

        let report = clear_at(ClearScope::Both, &oci, &git).unwrap();
        assert_eq!(report.removed, 2);
        assert!(report.freed_bytes > 0);
        assert!(!oci.exists());
        assert!(!git.exists());
    }

    #[test]
    fn clear_oci_only_leaves_git_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        write(&oci.join("sha256/abc/chart/x"), b"a");
        write(&git.join("checkouts/abc/x"), b"b");

        clear_at(ClearScope::OciOnly, &oci, &git).unwrap();
        assert!(!oci.exists());
        assert!(git.exists());
    }

    #[test]
    fn clear_missing_root_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let oci = tmp.path().join("oci");
        let git = tmp.path().join("git");
        let report = clear_at(ClearScope::Both, &oci, &git).unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(report.freed_bytes, 0);
    }

    #[test]
    fn default_cache_root_honours_xdg_home_then_home() {
        // Can't override env deterministically in parallel tests, so
        // just verify the path shape rather than concrete values —
        // the subdir is always the last component.
        let root = default_cache_root("oci");
        assert!(root.ends_with("oci"));
    }
}
