//! Stable error code constants per [cli-contract §1.2](../../../../docs/cli-contract.md#12-structured-errors-on-stderr).
//!
//! Codes are part of the stability contract — agents branch on them
//! and docs link to them. Collecting them here prevents typo drift
//! across verbs and makes the inventory greppable.
//!
//! Naming: `SHOUTY_SNAKE_CASE` prefixed with `E_`.

// ----- Lockfile / manifest -------------------------------------------------

pub const E_MANIFEST_MISSING: &str = "E_MANIFEST_MISSING";
pub const E_MANIFEST_PARSE: &str = "E_MANIFEST_PARSE";
pub const E_LOCK_MISSING: &str = "E_LOCK_MISSING";
pub const E_LOCK_PARSE: &str = "E_LOCK_PARSE";

// ----- General -------------------------------------------------------------

pub const E_IO: &str = "E_IO";
