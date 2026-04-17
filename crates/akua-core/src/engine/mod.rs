//! Engine plugins — the extension point for "how does a source turn into a
//! chart fragment?"
//!
//! An [`Engine`] is invoked once per source during umbrella assembly. Its job
//! is to produce the [`Dependency`] entry the umbrella's `Chart.yaml` will
//! reference, or materialise a chart directory on disk (for early-binding
//! engines like KCL and helmfile) and return a `file://` reference.
//!
//! Shipped engines:
//!
//! - [`helm`] — always compiled in. Pass-through for sources that are already
//!   Helm charts (HTTP repo, OCI registry, Git path).
//! - [`kcl`] — behind the `engine-kcl` feature. Shells to the `kcl` CLI,
//!   captures rendered YAML, wraps it as a static Helm chart directory.
//! - [`helmfile`] — behind the `engine-helmfile` feature. Shells to
//!   `helmfile template`, wraps output similarly.
//!
//! Engines run at **authoring time only** — never at install or deploy time.
//! See `docs/design-notes.md` §4 for the contract in full.
//!
//! ```text
//! HelmSource  ──Engine::prepare──►  PreparedSource
//!                                   ├─ ::Dependency (helm-shaped)
//!                                   ├─ ::Git        (clone + render separately)
//!                                   └─ ::LocalChart (engine materialised a chart dir)
//! ```

use std::path::{Path, PathBuf};

use crate::source::HelmSource;
use crate::umbrella::Dependency;

pub mod helm;

#[cfg(feature = "engine-kcl")]
pub mod kcl;

#[cfg(feature = "engine-helmfile")]
pub mod helmfile;

pub use helm::HelmEngine;

/// The engine name used when `engine:` is omitted from a source.
pub const DEFAULT_ENGINE: &str = "helm";

/// What an engine produces for one source during umbrella assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedSource {
    /// Standard Helm dependency — add this entry to the umbrella's
    /// `Chart.yaml` `dependencies:` list.
    Dependency(Dependency),
    /// Git-shaped source — caller clones + renders separately and merges
    /// the output manifests alongside the Helm render.
    Git,
    /// The engine has materialised a chart directory at this path. The
    /// umbrella dep references it via `file://<path>`.
    LocalChart(PathBuf),
}

/// Context passed to engines that need to write materialised chart
/// directories to disk.
///
/// Engines write under `work_dir` — one sub-directory per source, typically
/// named after the source id. The umbrella then references those directories
/// via `file://` in `Chart.yaml` dependencies.
#[derive(Debug, Clone)]
pub struct PrepareContext<'a> {
    pub work_dir: &'a Path,
}

/// Errors raised by engine preparation.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine `{engine}` requires field `{field}` on source `{source_id}`")]
    MissingField {
        engine: &'static str,
        source_id: String,
        field: &'static str,
    },
    #[error("engine `{engine}`: running `{cmd}`: {source}")]
    Spawn {
        engine: &'static str,
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("engine `{engine}`: `{cmd}` exited with status {status}:\n{stderr}")]
    CliFailed {
        engine: &'static str,
        cmd: String,
        status: i32,
        stderr: String,
    },
    #[error("engine `{engine}`: writing to {path}: {source}")]
    Write {
        engine: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// An engine plugin. Implementations decide how their source type contributes
/// to the umbrella chart.
pub trait Engine: Send + Sync {
    /// Stable identifier matched against `engine:` in `package.yaml`.
    fn name(&self) -> &'static str;

    /// Produce the umbrella entry for this source. Engines that need to
    /// write intermediate artifacts use `ctx.work_dir`.
    fn prepare(
        &self,
        source: &HelmSource,
        ctx: &PrepareContext<'_>,
    ) -> Result<PreparedSource, EngineError>;
}

/// Resolve an engine by name. Returns `None` for unknown engines — callers
/// should surface that as a package-level error so users get a clear message.
pub fn resolve(name: &str) -> Option<&'static dyn Engine> {
    match name {
        "helm" => Some(&helm::HELM_ENGINE),
        #[cfg(feature = "engine-kcl")]
        "kcl" => Some(&kcl::KCL_ENGINE),
        #[cfg(feature = "engine-helmfile")]
        "helmfile" => Some(&helmfile::HELMFILE_ENGINE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_returns_helm_engine_by_name() {
        let e = resolve("helm").unwrap();
        assert_eq!(e.name(), "helm");
    }

    #[test]
    fn resolve_returns_none_for_unknown_engine() {
        // Even with both features compiled in, unknown engines return None.
        assert!(resolve("nonexistent").is_none());
    }

    #[cfg(not(feature = "engine-kcl"))]
    #[test]
    fn resolve_kcl_is_none_without_feature() {
        assert!(resolve("kcl").is_none());
    }

    #[cfg(feature = "engine-kcl")]
    #[test]
    fn resolve_kcl_is_some_with_feature() {
        assert!(resolve("kcl").is_some());
    }
}
