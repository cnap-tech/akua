//! # akua-core
//!
//! Library surface for the `akua` CLI. Typed primitives for:
//!
//! - [`cli_contract`] — universal flags, exit codes, structured errors,
//!   agent-context detection.
//! - [`lock_file`] / [`mod_file`] — `akua.lock` + `akua.toml` parsers.
//! - [`package_k`] — `Package.k` loader (KCL-only subset).
//! - [`package_render`] — writes a rendered Package to disk as raw YAML.
//!
//! Spec: [`docs/package-format.md`](../../../docs/package-format.md),
//! [`docs/lockfile-format.md`](../../../docs/lockfile-format.md),
//! [`docs/cli-contract.md`](../../../docs/cli-contract.md).

#![allow(dead_code)]

pub mod cli_contract;
#[cfg(feature = "engine-kcl")]
pub mod dir_diff;
pub(crate) mod hex;
pub mod lock_file;
pub mod mod_file;
#[cfg(feature = "engine-kcl")]
pub mod package_k;
#[cfg(feature = "engine-kcl")]
pub mod package_render;

pub use cli_contract::{AgentContext, AgentSource, ExitCode, StructuredError};
pub use lock_file::{
    AkuaLock, LockError, LockLoadError, LockedPackage, Replaced, CURRENT_VERSION as LOCK_VERSION,
};
pub use mod_file::{
    AkuaManifest, Dependency as ManifestDependency, DependencySource, ManifestError,
    ManifestLoadError, PackageSection, Replace, WorkspaceSection,
};
#[cfg(feature = "engine-kcl")]
pub use dir_diff::{diff as dir_diff, DirDiff, DirDiffError, FileChange};
#[cfg(feature = "engine-kcl")]
pub use package_k::{
    format_kcl, lint_kcl, LintIssue, OutputSpec, PackageK, PackageKError, RenderedPackage,
};
#[cfg(feature = "engine-kcl")]
pub use package_render::{
    render_outputs, OutputSummary, RenderError as PackageRenderError, RenderSummary,
    FORMAT_RAW_MANIFESTS,
};
