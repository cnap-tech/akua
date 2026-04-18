//! Shared sha256 digest verification for pull paths.
//!
//! Both the OCI layer (manifest-advertised) and HTTP Helm index.yaml
//! (`digest:` field) publish a sha256 for the chart tarball. We verify
//! after streaming — a registry that swaps bytes can't ride through
//! undetected.

use super::FetchError;

/// Compare an advertised sha256 against a locally-computed one.
///
/// - Accepts both bare hex and `sha256:<hex>` forms for `advertised`.
/// - Case-insensitive — some registries emit upper-hex, our local
///   computation is lower-hex.
/// - `actual` must be bare lower-hex (what [`super::hex::hex_encode`]
///   returns). Returns [`FetchError::DigestMismatch`] with the
///   original `advertised` preserved for operator debugging; the
///   caller supplies `url` (redacted for error logs).
pub(super) fn verify(url: &str, advertised: &str, actual: &str) -> Result<(), FetchError> {
    let normalised = advertised
        .strip_prefix("sha256:")
        .unwrap_or(advertised)
        .to_ascii_lowercase();
    if normalised == actual.to_ascii_lowercase() {
        Ok(())
    } else {
        Err(FetchError::DigestMismatch {
            url: url.to_string(),
            expected: advertised.to_string(),
            actual: actual.to_string(),
        })
    }
}
