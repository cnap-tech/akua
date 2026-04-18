//! Shared HTTP response streaming helpers.
//!
//! Both the HTTP Helm and OCI pull paths need to drain a
//! `reqwest::Response` safely — capped by [`AKUA_MAX_DOWNLOAD_BYTES`]
//! against a spoofed `Content-Length`, avoiding unbounded
//! preallocation, and optionally computing a sha256 on the fly.
//!
//! [`AKUA_MAX_DOWNLOAD_BYTES`]: super::options

use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::hex::hex_encode;
use super::options::{limit_exceeded, LimitKind};
use super::FetchError;

/// Stream a response body into memory, aborting once `limit` bytes
/// have been read. Used for small bodies (repo `index.yaml`); for
/// chart tarballs, prefer [`stream_response_to_file`] which never
/// holds the full payload in memory.
pub(super) async fn download_with_limit(
    mut resp: reqwest::Response,
    limit: u64,
) -> Result<Vec<u8>, FetchError> {
    if resp.content_length().is_some_and(|d| d > limit) {
        return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
    }
    // Cap the upfront allocation so a server advertising a spoofed
    // Content-Length can't force a 100 MB reservation before any bytes
    // arrive. 4 MB is generous for an index.yaml and grows naturally
    // via `extend_from_slice`.
    const MAX_INITIAL_ALLOC: usize = 4 * 1024 * 1024;
    let initial = resp
        .content_length()
        .map(|n| n.min(limit).min(MAX_INITIAL_ALLOC as u64) as usize)
        .unwrap_or(4096);
    let mut buf: Vec<u8> = Vec::with_capacity(initial);
    while let Some(chunk) = resp.chunk().await? {
        if (buf.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stream a response body to an on-disk tempfile, writing chunks as
/// they arrive and computing the sha256 on the fly. Peak memory is one
/// chunk (~16 KB typical for reqwest). Returns the tempfile handle plus
/// the hex-encoded digest so the caller can both unpack from disk and
/// promote the same file into the content-addressed cache without
/// reading its bytes a second time.
pub(super) async fn stream_response_to_file(
    mut resp: reqwest::Response,
    limit: u64,
    scratch_dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), FetchError> {
    if resp.content_length().is_some_and(|d| d > limit) {
        return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
    }
    std::fs::create_dir_all(scratch_dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(scratch_dir)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        if written.saturating_add(chunk.len() as u64) > limit {
            return Err(limit_exceeded(LimitKind::DownloadBytes, limit));
        }
        temp.write_all(&chunk)?;
        hasher.update(&chunk);
        written += chunk.len() as u64;
    }
    temp.flush()?;
    let digest = hex_encode(&hasher.finalize());
    Ok((temp, digest))
}
