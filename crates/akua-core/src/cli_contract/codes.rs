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

// ----- Render --------------------------------------------------------------

pub const E_PACKAGE_MISSING: &str = "E_PACKAGE_MISSING";
pub const E_PACKAGE_PARSE: &str = "E_PACKAGE_PARSE";
pub const E_INPUTS_MISSING: &str = "E_INPUTS_MISSING";
pub const E_INPUTS_PARSE: &str = "E_INPUTS_PARSE";
pub const E_RENDER_KCL: &str = "E_RENDER_KCL";
pub const E_RENDER_OUTPUT_NOT_FOUND: &str = "E_RENDER_OUTPUT_NOT_FOUND";
pub const E_RENDER_OUTPUT_AMBIGUOUS: &str = "E_RENDER_OUTPUT_AMBIGUOUS";
pub const E_RENDER_UNSUPPORTED_KIND: &str = "E_RENDER_UNSUPPORTED_KIND";
pub const E_RENDER_YAML: &str = "E_RENDER_YAML";

// ----- Init ----------------------------------------------------------------

pub const E_INIT_EXISTS: &str = "E_INIT_EXISTS";
pub const E_INIT_EMPTY_NAME: &str = "E_INIT_EMPTY_NAME";

// ----- Fmt -----------------------------------------------------------------

pub const E_FMT_CHANGED: &str = "E_FMT_CHANGED";
pub const E_FMT_KCL: &str = "E_FMT_KCL";

// ----- Lint ----------------------------------------------------------------

pub const E_LINT_FAIL: &str = "E_LINT_FAIL";

// ----- Check ---------------------------------------------------------------

pub const E_CHECK_FAIL: &str = "E_CHECK_FAIL";

// ----- Diff ----------------------------------------------------------------

pub const E_DIFF_FOUND: &str = "E_DIFF_FOUND";
pub const E_DIFF_NOT_DIR: &str = "E_DIFF_NOT_DIR";

// ----- Add -----------------------------------------------------------------

pub const E_ADD_DEP_EXISTS: &str = "E_ADD_DEP_EXISTS";
pub const E_ADD_INVALID_DEP: &str = "E_ADD_INVALID_DEP";

// ----- General -------------------------------------------------------------

pub const E_IO: &str = "E_IO";
