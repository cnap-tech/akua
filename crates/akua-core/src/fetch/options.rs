//! Per-call + process-global safety limits.
//!
//! Resolution order per call:
//!
//! 1. Per-call [`FetchOptions`] override (threaded via a thread-local
//!    installed by [`fetch_dependencies_with_options`]).
//! 2. Env var (`AKUA_MAX_*`) — process-global default.
//! 3. Hardcoded default below.
//!
//! [`fetch_dependencies_with_options`]: super::fetch_dependencies_with_options

use super::FetchError;

/// Per-call overrides for the fetch safety limits. `None` fields fall
/// back to the env var / default. Useful for multi-tenant hosts that
/// want per-request limits without touching process-global state.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    pub max_download_bytes: Option<u64>,
    pub max_extracted_bytes: Option<u64>,
    pub max_tar_entries: Option<u64>,
    /// Disable the content-addressed cache for this call (equivalent
    /// to setting `AKUA_NO_CACHE=1` but scoped to one call).
    pub disable_cache: bool,
}

thread_local! {
    /// Call-scoped override; only set via [`OptionsGuard::install`] for
    /// the duration of a single `fetch_dependencies_*` invocation.
    static CALL_OPTIONS: std::cell::RefCell<Option<FetchOptions>> =
        const { std::cell::RefCell::new(None) };
}

/// RAII guard that installs `options` onto the current thread's
/// `CALL_OPTIONS` and clears it on drop.
pub(super) struct OptionsGuard {
    previous: Option<FetchOptions>,
}

impl OptionsGuard {
    pub(super) fn install(options: FetchOptions) -> Self {
        let previous = CALL_OPTIONS.with(|cell| cell.replace(Some(options)));
        Self { previous }
    }
}

impl Drop for OptionsGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CALL_OPTIONS.with(|cell| *cell.borrow_mut() = previous);
    }
}

fn call_option<T, F: FnOnce(&FetchOptions) -> Option<T>>(extract: F) -> Option<T> {
    CALL_OPTIONS.with(|cell| cell.borrow().as_ref().and_then(extract))
}

pub(super) fn max_download_bytes() -> u64 {
    call_option(|o| o.max_download_bytes)
        .unwrap_or_else(|| env_bytes("AKUA_MAX_DOWNLOAD_BYTES", 100 * 1024 * 1024))
}

pub(super) fn max_extracted_bytes() -> u64 {
    call_option(|o| o.max_extracted_bytes)
        .unwrap_or_else(|| env_bytes("AKUA_MAX_EXTRACTED_BYTES", 500 * 1024 * 1024))
}

pub(super) fn max_tar_entries() -> u64 {
    call_option(|o| o.max_tar_entries).unwrap_or_else(|| env_bytes("AKUA_MAX_TAR_ENTRIES", 20_000))
}

/// Whether the on-disk cache should be bypassed for the current call.
pub(super) fn cache_disabled() -> bool {
    if CALL_OPTIONS.with(|cell| cell.borrow().as_ref().is_some_and(|o| o.disable_cache)) {
        return true;
    }
    std::env::var_os("AKUA_NO_CACHE").is_some()
}

pub(super) fn env_bytes(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

/// Which download-safety limit tripped. Backs [`FetchError::LimitExceeded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Raw bytes from the wire (HTTP body, OCI layer).
    DownloadBytes,
    /// Uncompressed bytes during tar extraction.
    ExtractedBytes,
    /// Number of entries inside a chart tarball.
    TarEntries,
}

impl LimitKind {
    pub(super) fn env_var(self) -> &'static str {
        match self {
            Self::DownloadBytes => "AKUA_MAX_DOWNLOAD_BYTES",
            Self::ExtractedBytes => "AKUA_MAX_EXTRACTED_BYTES",
            Self::TarEntries => "AKUA_MAX_TAR_ENTRIES",
        }
    }
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DownloadBytes => f.write_str("download size"),
            Self::ExtractedBytes => f.write_str("extracted size"),
            Self::TarEntries => f.write_str("tarball entry count"),
        }
    }
}

/// Build a [`FetchError::LimitExceeded`] with the env-var hint filled in.
pub(super) fn limit_exceeded(kind: LimitKind, limit: u64) -> FetchError {
    FetchError::LimitExceeded {
        kind,
        limit,
        env_var: kind.env_var(),
    }
}
