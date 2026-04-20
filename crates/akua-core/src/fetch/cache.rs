//! Content-addressed on-disk chart cache.
//!
//! Layout under `$XDG_CACHE_HOME/akua/v1/` (or `$HOME/.cache/akua/v1/`):
//!
//! ```text
//! refs/<sha256(key)>       # contents: hex sha256 of the blob
//! blobs/<sha256(blob)>.tgz # the cached tarball bytes
//! ```
//!
//! Two-tier: `refs/` keys a lookup by `(repo, name, version)` → content
//! digest; `blobs/` stores tarballs content-addressed so identical
//! tarballs dedupe across names. Writes are atomic via `tempfile::persist`.
//! All failures are non-fatal — the caller falls back to live download.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

use crate::hex::{hex_encode, is_valid_sha256_hex};
use crate::umbrella::Dependency;

pub(super) fn key_for_dep(dep: &Dependency) -> String {
    format!("{}|{}|{}", dep.repository, dep.name, dep.version)
}

pub(super) fn get(key: &str) -> Option<Vec<u8>> {
    let root = root()?;
    let key_hash = sha256_hex(key.as_bytes());
    let ref_path = root.join("refs").join(&key_hash);
    let blob_digest = std::fs::read_to_string(&ref_path).ok()?;
    let blob_digest = blob_digest.trim();
    if !is_valid_sha256_hex(blob_digest) {
        return None;
    }
    let blob_path = root.join("blobs").join(format!("{blob_digest}.tgz"));
    let bytes = std::fs::read(&blob_path).ok()?;
    // Integrity check: a corrupted blob is worse than a cache miss.
    if sha256_hex(&bytes) != blob_digest {
        return None;
    }
    Some(bytes)
}

pub(super) fn put(key: &str, bytes: &[u8]) -> std::io::Result<()> {
    let Some(root) = root() else {
        return Ok(());
    };
    let blob_digest = sha256_hex(bytes);
    let (blobs_dir, refs_dir) = ensure_dirs(&root)?;
    let blob_path = blobs_dir.join(format!("{blob_digest}.tgz"));
    if !blob_path.exists() {
        let mut tmp = tempfile::NamedTempFile::new_in(&blobs_dir)?;
        tmp.write_all(bytes)?;
        tmp.flush()?;
        tmp.persist(&blob_path).map_err(|e| e.error)?;
    }
    write_ref(&refs_dir, key, &blob_digest)
}

/// Cache-put from an already-written tempfile. The caller has
/// streamed bytes to the tempfile and knows the sha256 digest;
/// we atomic-rename the file into `blobs/<digest>.tgz` (zero
/// extra reads). Preferred over [`put`] for chart-sized payloads.
pub(super) fn put_file(
    key: &str,
    digest: &str,
    temp: tempfile::NamedTempFile,
) -> std::io::Result<()> {
    let Some(root) = root() else {
        return Ok(());
    };
    let (blobs_dir, refs_dir) = ensure_dirs(&root)?;
    let blob_path = blobs_dir.join(format!("{digest}.tgz"));
    if blob_path.exists() {
        // Someone else already raced us — their file is
        // content-addressed so identical bytes either way. Drop
        // the tempfile and reuse theirs.
        drop(temp);
    } else {
        temp.persist(&blob_path).map_err(|e| e.error)?;
    }
    write_ref(&refs_dir, key, digest)?;
    // Best-effort LRU trim. A failing evict shouldn't break the
    // write that just landed.
    let _ = evict_if_over_cap(&blobs_dir, &refs_dir);
    Ok(())
}

/// Approximate LRU eviction by mtime. Reads `AKUA_MAX_CACHE_BYTES`
/// (default 5 GB). Removes oldest `blobs/*.tgz` first; orphan refs
/// get cleaned up on next read (they just miss).
fn evict_if_over_cap(
    blobs_dir: &std::path::Path,
    refs_dir: &std::path::Path,
) -> std::io::Result<()> {
    let cap = max_cache_bytes();
    if cap == 0 {
        return Ok(());
    }
    let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let mut total: u64 = 0;
    for e in std::fs::read_dir(blobs_dir)? {
        let e = e?;
        let meta = e.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        total += meta.len();
        entries.push((e.path(), mtime, meta.len()));
    }
    if total <= cap {
        return Ok(());
    }
    // Oldest first.
    entries.sort_by_key(|(_, mtime, _)| *mtime);
    for (path, _, size) in entries {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    // Sweep orphan ref files (refs pointing at no-longer-existing blobs).
    for e in std::fs::read_dir(refs_dir)?.flatten() {
        if let Ok(contents) = std::fs::read_to_string(e.path()) {
            let digest = contents.trim();
            let blob = blobs_dir.join(format!("{digest}.tgz"));
            if !blob.exists() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

fn max_cache_bytes() -> u64 {
    // Default 5 GB. Set to 0 to disable eviction entirely.
    super::options::env_bytes("AKUA_MAX_CACHE_BYTES", 5 * 1024 * 1024 * 1024)
}

/// Look up the cached blob path for `key` without reading its
/// bytes. Integrity check happens at read time by the consumer
/// (streaming unpack). Caller must re-verify if paranoid.
pub(super) fn get_path(key: &str) -> Option<PathBuf> {
    let root = root()?;
    let ref_path = root.join("refs").join(sha256_hex(key.as_bytes()));
    let blob_digest = std::fs::read_to_string(&ref_path).ok()?;
    let blob_digest = blob_digest.trim();
    if !is_valid_sha256_hex(blob_digest) {
        return None;
    }
    let blob_path = root.join("blobs").join(format!("{blob_digest}.tgz"));
    blob_path.exists().then_some(blob_path)
}

fn ensure_dirs(root: &std::path::Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let blobs = root.join("blobs");
    let refs = root.join("refs");
    std::fs::create_dir_all(&blobs)?;
    std::fs::create_dir_all(&refs)?;
    Ok((blobs, refs))
}

fn write_ref(refs_dir: &std::path::Path, key: &str, digest: &str) -> std::io::Result<()> {
    let key_hash = sha256_hex(key.as_bytes());
    let ref_path = refs_dir.join(&key_hash);
    let mut tmp = tempfile::NamedTempFile::new_in(refs_dir)?;
    tmp.write_all(digest.as_bytes())?;
    tmp.flush()?;
    tmp.persist(&ref_path).map_err(|e| e.error)?;
    Ok(())
}

/// Resolve the cache root, honouring `XDG_CACHE_HOME` and `HOME`.
/// Returns `None` on systems where neither is set (e.g. some CI
/// sandboxes) — caller falls back to no-cache behaviour.
fn root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AKUA_CACHE_DIR") {
        return Some(PathBuf::from(dir).join("v1"));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("akua").join("v1"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}
