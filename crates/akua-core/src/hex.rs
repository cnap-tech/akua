//! Lower-hex encoding + sha256-digest validation, shared by the fetch
//! pipeline (`fetch/streaming.rs`, `fetch/cache.rs`) and the render
//! pipeline (`package_render.rs`). Kept in one module so the two don't
//! drift on digest formatting.

/// Hex-encode an arbitrary byte slice.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
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
pub(crate) fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}
