//! Tar+gzip unpack with safety caps.
//!
//! Enforces three classes of limit:
//!
//! - **Gzip bomb** — the inner [`LimitedReader`] caps decompressed
//!   bytes at `AKUA_MAX_EXTRACTED_BYTES`.
//! - **Entry explosion** — the walk counts entries against
//!   `AKUA_MAX_TAR_ENTRIES` and returns early.
//! - **Path escape** — [`validate_tar_entry_path`] rejects absolute /
//!   `..` / Windows-root paths; non-regular-file / non-directory tar
//!   types are rejected outright (symlinks, hard links, device files
//!   all error out with `UnsafeEntryPath`).

use std::path::{Path, PathBuf};

use super::options::{limit_exceeded, max_extracted_bytes, max_tar_entries, LimitKind};
use super::FetchError;

/// Unpack a `tar+gzip` chart tarball into `charts_dir/target_name/`.
///
/// Helm chart tarballs wrap the chart under a single top-level directory
/// named `<chart-name>/` (which may or may not match `target_name` —
/// e.g., `nginx-18.1.0.tgz` wraps `nginx/`). We extract the archive and
/// rename the top-level dir to `target_name` (so aliased deps land at
/// their alias).
pub(super) fn unpack_chart_tgz<R: std::io::Read>(
    tgz: R,
    charts_dir: &Path,
    target_name: &str,
) -> Result<(), FetchError> {
    let tmp = tempfile::tempdir_in(charts_dir)?;
    // Cap the gunzip stream so a gzip bomb can't fill the disk.
    // `tar::Archive::unpack` reads until EOF, which for a bomb might be
    // terabytes of zeros. The `LimitedReader` surfaces EOF early, at
    // which point `tar` returns a clean error.
    let gz = flate2::read::GzDecoder::new(tgz);
    let extract_limit = max_extracted_bytes();
    let entry_limit = max_tar_entries();
    let capped = LimitedReader::new(gz, extract_limit);
    let mut archive = tar::Archive::new(capped);

    // Walk entries manually so we can (a) enforce the entry-count cap,
    // (b) reject path-traversal attempts, (c) reject symlinks/hard-links
    // (which otherwise let a malicious chart write outside the target
    // via `entry.unpack()` copying the header linkname verbatim).
    let mut total_entries: u64 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        total_entries += 1;
        if total_entries > entry_limit {
            return Err(limit_exceeded(LimitKind::TarEntries, entry_limit));
        }
        let path = entry.path()?.into_owned();
        validate_tar_entry_path(&path)?;
        // Reject symlinks, hard links, and special files. Real Helm
        // charts ship regular files + directories only; anything else
        // is either a packaging mistake or an extraction escape
        // (CVE class: tar-slip via symlink → target file read).
        let kind = entry.header().entry_type();
        if !matches!(
            kind,
            tar::EntryType::Regular | tar::EntryType::Directory | tar::EntryType::XGlobalHeader
        ) {
            return Err(FetchError::UnsafeEntryPath {
                path: format!("{} (disallowed entry type: {:?})", path.display(), kind),
            });
        }
        // Skip the PAX global header record itself (it's metadata, not a file).
        if matches!(kind, tar::EntryType::XGlobalHeader) {
            continue;
        }
        let dest_path = tmp.path().join(&path);
        // Per-entry `unpack` doesn't create intermediate dirs, unlike the
        // one-shot `archive.unpack()`. Do it ourselves.
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&dest_path).map_err(|e| {
            if error_chain_has_extract_signal(&e) {
                limit_exceeded(LimitKind::ExtractedBytes, extract_limit)
            } else {
                FetchError::Unpack(e)
            }
        })?;
    }

    // The archive contains one top-level dir — move it to our target name.
    let mut top: Option<PathBuf> = None;
    for entry in std::fs::read_dir(tmp.path())? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if top.is_some() {
                return Err(FetchError::MalformedChart(
                    "tarball has >1 top-level directory".to_string(),
                ));
            }
            top = Some(entry.path());
        }
    }
    let top = top.ok_or_else(|| {
        FetchError::MalformedChart("tarball has no top-level directory".to_string())
    })?;
    let dest = charts_dir.join(target_name);
    std::fs::rename(&top, &dest)?;
    Ok(())
}

/// Reject tar entries that would write outside the target dir: absolute
/// paths, `..` components, Windows prefix/root. `entry.unpack()` doesn't
/// filter these on its own, so the caller must.
pub(super) fn validate_tar_entry_path(path: &Path) -> Result<(), FetchError> {
    use std::path::Component;
    path.components().try_for_each(|c| match c {
        Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
            Err(FetchError::UnsafeEntryPath {
                path: path.display().to_string(),
            })
        }
        _ => Ok(()),
    })
}

/// Caps the gunzip stream so a decompression bomb fails early instead
/// of filling the disk.
pub(super) struct LimitedReader<R> {
    inner: R,
    consumed: u64,
    limit: u64,
}

impl<R: std::io::Read> LimitedReader<R> {
    pub(super) fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            consumed: 0,
            limit,
        }
    }
}

/// Marker substring in the `io::Error` that `LimitedReader` emits. The
/// extract path recognises "chart too big" vs "chart broken" by
/// scanning the error chain for this string.
///
/// Why string-match rather than a typed downcast: `io::Error::source()`
/// delegates to the *inner* error's `source()`, so a boxed typed
/// payload is skipped by the chain walker — the payload is never
/// directly visible as a node in the chain. The substring is process-
/// internal; `FetchError::LimitExceeded` is what the caller actually
/// sees.
const EXTRACT_LIMIT_SIGNAL: &str = "akua extract limit";

fn error_chain_has_extract_signal(err: &std::io::Error) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if e.to_string().contains(EXTRACT_LIMIT_SIGNAL) {
            return true;
        }
        current = e.source();
    }
    false
}

impl<R: std::io::Read> std::io::Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.consumed >= self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{EXTRACT_LIMIT_SIGNAL} ({} bytes) exceeded", self.limit),
            ));
        }
        let remaining = self.limit - self.consumed;
        let to_read = std::cmp::min(buf.len() as u64, remaining) as usize;
        let n = self.inner.read(&mut buf[..to_read])?;
        self.consumed += n as u64;
        Ok(n)
    }
}
