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
//!
//! ## Invariant: no CLI coupling
//!
//! This crate is a pure library. It does not depend on `clap`, does not
//! call `std::process::exit`, does not read `std::env::args`, and has
//! no `[[bin]]` target. The only process-spawn usage is [`helm`] shelling
//! out to the `helm` binary — a library capability, not a CLI coupling.
//!
//! The invariant exists so future non-CLI consumers (Node-API binding,
//! Python `py03` binding, in-process `@akua/sdk` without spawn, an HTTP
//! server) can depend on this crate without inheriting CLI assumptions.
//! Don't introduce `clap`, argv parsing, or process::exit here. Those
//! live in the `akua-cli` crate.

#![allow(dead_code)]

/// Apply the SDK-export derives to a contract type. Avoids hand-repeating
/// three `#[cfg_attr]` attributes per struct/enum (and the `export_to`
/// path, where a typo silently drops the type from the bundle).
///
/// Usage:
/// ```ignore
/// akua_core::contract_type! {
///     #[derive(Debug, Serialize, Deserialize)]
///     pub struct Foo { ... }
/// }
/// ```
///
/// Workspace-internal by intent: `akua-cli` already consumes it for
/// verbs that define their own response types (e.g. `VersionOutput`).
/// External consumers shouldn't need it — they're not writing to
/// `sdk-types/` or the bundle. `#[macro_export]` is the only mechanism
/// that makes the macro reachable across crates in the same workspace,
/// so it's public by mechanism; `#[doc(hidden)]` keeps it out of the
/// rendered API docs.
#[macro_export]
#[doc(hidden)]
macro_rules! contract_type {
    ($item:item) => {
        #[cfg_attr(feature = "ts-export", derive(::ts_rs::TS))]
        #[cfg_attr(
            feature = "ts-export",
            ts(export, export_to = "../../../packages/sdk/src/types/")
        )]
        #[cfg_attr(feature = "schema-export", derive(::schemars::JsonSchema))]
        $item
    };
}

pub mod cli_contract;
#[cfg(feature = "engine-kcl")]
pub mod dir_diff;
#[cfg(feature = "engine-helm-shell")]
pub mod helm;
pub(crate) mod hex;
#[cfg(feature = "engine-kcl")]
pub mod kcl_plugin;
pub mod lock_file;
pub mod mod_file;
#[cfg(feature = "engine-kcl")]
pub mod package_k;
#[cfg(feature = "engine-kcl")]
pub mod package_render;
#[cfg(feature = "engine-kcl")]
pub mod pkg_render;

pub use cli_contract::{AgentContext, AgentSource, ExitCode, StructuredError};
#[cfg(feature = "engine-kcl")]
pub use dir_diff::{diff as dir_diff, DirDiff, DirDiffError, FileChange};
pub use lock_file::{
    AkuaLock, LockError, LockLoadError, LockedPackage, Replaced, CURRENT_VERSION as LOCK_VERSION,
};
pub use mod_file::{
    AkuaManifest, DependencySource, ManifestError, ManifestLoadError, PackageSection, Replace,
    WorkspaceSection,
};
#[cfg(feature = "engine-kcl")]
pub use package_k::{
    format_kcl, lint_kcl, list_options_kcl, LintIssue, OptionInfo, OutputSpec, PackageK,
    PackageKError, RenderedPackage,
};
#[cfg(feature = "engine-kcl")]
pub use package_render::{
    render_outputs, OutputSummary, RenderError as PackageRenderError, RenderSummary,
    FORMAT_RAW_MANIFESTS,
};
