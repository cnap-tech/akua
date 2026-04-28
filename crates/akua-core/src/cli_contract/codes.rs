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
/// `akua.lock` is out of sync with `akua.toml` — `akua lock --check`
/// found drift. Re-run `akua lock` without `--check` to refresh.
pub const E_LOCK_DRIFT: &str = "E_LOCK_DRIFT";

/// A dep alias referenced by `import <alias>` (or `pkg.render({package =
/// "<alias>"})`) in `package.k` resolves to a kind that's unreachable
/// from KCL. Most common case: an Akua/KCL-module dep was misclassified
/// as a Helm chart by the resolver, or the user declared a Helm chart
/// alias they then tried to `import`. `akua lock` catches this before
/// `akua check` later fails with the opaque `CannotFindModule` from KCL.
pub const E_DEP_KIND_MISMATCH: &str = "E_DEP_KIND_MISMATCH";

// ----- Render --------------------------------------------------------------

pub const E_PACKAGE_MISSING: &str = "E_PACKAGE_MISSING";
pub const E_PACKAGE_PARSE: &str = "E_PACKAGE_PARSE";
pub const E_INPUTS_MISSING: &str = "E_INPUTS_MISSING";
pub const E_INPUTS_PARSE: &str = "E_INPUTS_PARSE";
pub const E_RENDER_KCL: &str = "E_RENDER_KCL";
pub const E_RENDER_YAML: &str = "E_RENDER_YAML";
/// Package called an engine plugin whose WASM backend hasn't shipped
/// yet (docs/roadmap.md tracks the blocked features). Shell-out is
/// not an option — see CLAUDE.md "No shell-out, ever."
pub const E_ENGINE_NOT_AVAILABLE: &str = "E_ENGINE_NOT_AVAILABLE";
/// Package argument to an engine plugin resolved to a path outside
/// the Package directory (traversal / symlink escape).
pub const E_PATH_ESCAPE: &str = "E_PATH_ESCAPE";

/// `pkg.render(opts)` returned a sentinel that the user then patched
/// via sibling fields (e.g. `r | {metadata.labels: {...}} for r in
/// pkg.render(...)`). The expander would wholesale-replace the sentinel
/// with the inner Package's resources — any sibling field silently
/// disappears. Surfaced loud instead. Filter / select operations on
/// the sentinel hit the same case. Full engine-style synchronous
/// `pkg.render` (which would let patches + filters apply naturally)
/// is tracked at #479.
pub const E_PKG_RENDER_PATCH_UNSUPPORTED: &str = "E_PKG_RENDER_PATCH_UNSUPPORTED";
/// `charts.*` dep in `akua.toml` failed to resolve (missing path,
/// not-a-directory, OCI/git Phase-2b gate). See chart_resolver.
pub const E_CHART_RESOLVE: &str = "E_CHART_RESOLVE";
/// `akua render --strict`: a plugin was handed a raw-string chart
/// path instead of a typed `charts.*` import. Surfaces the Package
/// authoring site that needs to migrate.
pub const E_STRICT_UNTYPED_CHART: &str = "E_STRICT_UNTYPED_CHART";
/// Cosign signature failed cryptographic verification, or the payload
/// disagrees with the fetched digest. Attacker-side signal —
/// someone served bytes the configured key didn't approve.
pub const E_COSIGN_VERIFY: &str = "E_COSIGN_VERIFY";
/// A cosign public key was configured but the registry has no
/// `.sig` sidecar (or it's malformed). Publisher-side signal —
/// actionable by the artifact's author, not the consumer.
pub const E_COSIGN_SIG_MISSING: &str = "E_COSIGN_SIG_MISSING";

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

// ----- Inspect -------------------------------------------------------------

pub const E_INSPECT_FAIL: &str = "E_INSPECT_FAIL";

// ----- Diff ----------------------------------------------------------------

pub const E_DIFF_FOUND: &str = "E_DIFF_FOUND";
pub const E_DIFF_NOT_DIR: &str = "E_DIFF_NOT_DIR";

// ----- Add -----------------------------------------------------------------

pub const E_ADD_DEP_EXISTS: &str = "E_ADD_DEP_EXISTS";
pub const E_ADD_INVALID_DEP: &str = "E_ADD_INVALID_DEP";

// ----- Remove --------------------------------------------------------------

pub const E_REMOVE_NOT_FOUND: &str = "E_REMOVE_NOT_FOUND";

// ----- Publish / Pull ------------------------------------------------------

/// `akua publish` failed to upload the artifact. Wraps every registry-
/// side failure (auth rejected, upload PUT non-2xx, manifest malformed).
pub const E_PUBLISH_FAILED: &str = "E_PUBLISH_FAILED";
/// `akua pull` couldn't retrieve / extract the requested artifact.
pub const E_PULL_FAILED: &str = "E_PULL_FAILED";

// Test failures surface via the JSON `status: "fail"` verdict +
// exit code 1 (UserError). No structured stderr error — matches
// the `render` / `verify` pattern where a valid-but-negative
// verdict is an output, not an error.

// ----- General -------------------------------------------------------------

pub const E_IO: &str = "E_IO";
