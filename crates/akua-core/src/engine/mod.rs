//! Engine plugins — the extension point for "how does a source turn into a
//! chart fragment?"
//!
//! Dispatch is by block presence on the [`Source`]: if `source.helm` is set,
//! the Helm engine runs; if `source.kcl`, the KCL engine; if
//! `source.helmfile`, the helmfile engine. Validation that exactly one block
//! is set happens at manifest-load time.
//!
//! Shipped engines:
//!
//! - [`helm`] — always compiled in. Pass-through for sources that are already
//!   Helm charts (HTTP repo, OCI registry).
//! - [`kcl`] — behind the `engine-kcl` feature. Compiles KCL programs to
//!   rendered YAML and wraps the output as a static Helm chart directory.
//! - [`helmfile`] — behind the `engine-helmfile` feature. Shells to
//!   `helmfile template` and wraps the output similarly.
//!
//! Engines run at **authoring time only** — never at install or deploy time.
//! See `docs/design-notes.md` §4 for the contract.

use std::path::{Path, PathBuf};

use crate::source::{Source, SourceKind};
use crate::umbrella::Dependency;

pub mod helm;

#[cfg(feature = "engine-kcl")]
pub mod kcl;

#[cfg(feature = "engine-helmfile")]
pub mod helmfile;

pub use helm::HelmEngine;

/// What an engine produces for one source during umbrella assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedSource {
    /// Standard Helm dependency — add this entry to the umbrella's
    /// `Chart.yaml` `dependencies:` list.
    Dependency(Dependency),
    /// The engine has materialised a chart directory at this path. The
    /// umbrella references it via `file://<path>`.
    LocalChart(PathBuf),
}

/// Context passed to engines that need to write materialised chart
/// directories to disk. Engines write under `work_dir`.
#[derive(Debug, Clone)]
pub struct PrepareContext<'a> {
    pub work_dir: &'a Path,
}

/// Errors raised by engine preparation.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
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
    #[error("engine `{engine}`: not compiled in (enable the `engine-{engine}` feature)")]
    NotAvailable { engine: &'static str },
}

/// Prepare a source for umbrella inclusion. Dispatches on the source's
/// engine block — parse-time validation has already ensured exactly one
/// block is set.
pub fn prepare(source: &Source, ctx: &PrepareContext<'_>) -> Result<PreparedSource, EngineError> {
    let kind = source
        .kind()
        .expect("manifest validation ensures exactly one engine block");
    match kind {
        SourceKind::Helm => helm::HELM_ENGINE.prepare(source, ctx),
        #[cfg(feature = "engine-kcl")]
        SourceKind::Kcl => kcl::KCL_ENGINE.prepare(source, ctx),
        #[cfg(not(feature = "engine-kcl"))]
        SourceKind::Kcl => Err(EngineError::NotAvailable { engine: "kcl" }),
        #[cfg(feature = "engine-helmfile")]
        SourceKind::Helmfile => helmfile::HELMFILE_ENGINE.prepare(source, ctx),
        #[cfg(not(feature = "engine-helmfile"))]
        SourceKind::Helmfile => Err(EngineError::NotAvailable { engine: "helmfile" }),
    }
}

/// An engine plugin. Each implementation handles one `SourceKind` and reads
/// its own block off the [`Source`].
pub trait Engine: Send + Sync {
    fn name(&self) -> &'static str;
    fn prepare(
        &self,
        source: &Source,
        ctx: &PrepareContext<'_>,
    ) -> Result<PreparedSource, EngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::HelmBlock;

    fn helm_source() -> Source {
        Source {
            name: "app".into(),
            helm: Some(HelmBlock {
                repo: "https://charts.example.com".into(),
                chart: Some("nginx".into()),
                version: "1.0.0".into(),
            }),
            kcl: None,
            helmfile: None,
            values: None,
        }
    }

    #[test]
    fn prepare_dispatches_helm() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = PrepareContext {
            work_dir: tmp.path(),
        };
        let s = helm_source();
        match prepare(&s, &ctx).unwrap() {
            PreparedSource::Dependency(d) => {
                assert_eq!(d.name, "nginx");
            }
            other => panic!("expected Dependency, got {other:?}"),
        }
    }
}
