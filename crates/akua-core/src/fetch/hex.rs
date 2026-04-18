//! Hex encoding helpers shared across the fetch pipeline.
//!
//! Both the streaming download (`streaming.rs`) and the content-
//! addressed cache (`cache.rs`) encode sha256 digests the same way;
//! both sanity-check incoming digest strings the same way. Kept here
//! so the two modules don't drift out of sync.

/// Hex-encode an arbitrary byte slice. Used to stringify sha256
/// digests for on-disk filenames and error messages.
pub(super) fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Validate that `s` is a bare hex-encoded sha256 digest (64 lowercase
/// hex chars). Used on values read from the cache's `refs/` entries
/// and any attacker-controllable digest field before we turn it into a
/// filesystem path.
pub(super) fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}
