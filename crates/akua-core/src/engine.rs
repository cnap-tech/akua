//! Engine plugins — the extension point for "how does a source turn into a
//! chart fragment?"
//!
//! An [`Engine`] is invoked once per source during umbrella assembly. Its job
//! is to produce the [`Dependency`] entry the umbrella's `Chart.yaml` will
//! reference, and optionally a schema fragment that participates in the
//! merged install schema.
//!
//! Today the only shipped impl is [`HelmEngine`] — the source is already a
//! Helm chart (HTTP repo, OCI registry, or Git path) and nothing needs to be
//! done beyond extracting chart metadata. Future engines (KCL, kustomize,
//! helmfile) will materialise their source into a local chart dir and
//! return a file-referencing dependency.
//!
//! Engines run at **authoring time only** — never at install or deploy time.
//! See `docs/design-notes.md` §4 for the contract in full.
//!
//! ```text
//! HelmSource  ──Engine::prepare──►  PreparedSource
//!                                   ├─ ::Dependency (helm-shaped)
//!                                   ├─ ::Git        (clone + render)
//!                                   └─ ::LocalChart (future: KCL/kustomize
//!                                                    materialise a dir)
//! ```

use crate::source::HelmSource;
use crate::umbrella::Dependency;

/// The engine name used when `engine:` is omitted from a source or package
/// manifest. Matches the reserved name on [`HelmEngine`].
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
    /// Reserved for early-binding engines (KCL/kustomize) once they land:
    /// the engine has materialised a chart directory on disk. The umbrella
    /// dep should reference it via `file://<path>`.
    #[allow(dead_code)]
    LocalChart(std::path::PathBuf),
}

/// An engine plugin. Implementations decide how their source type contributes
/// to the umbrella chart.
pub trait Engine: Send + Sync {
    /// Stable identifier matched against `engine:` in `package.yaml`.
    fn name(&self) -> &'static str;

    /// Produce the umbrella entry for this source.
    fn prepare(&self, source: &HelmSource) -> PreparedSource;
}

/// The built-in Helm engine — pass-through for sources that are already
/// Helm charts (HTTP repo, OCI registry).
#[derive(Debug, Clone, Default)]
pub struct HelmEngine;

impl Engine for HelmEngine {
    fn name(&self) -> &'static str {
        DEFAULT_ENGINE
    }

    fn prepare(&self, source: &HelmSource) -> PreparedSource {
        use crate::source::{get_source_alias, is_oci, parse_oci_url};

        let alias = get_source_alias(source);

        if is_oci(&source.chart.repo_url) {
            if let Some(parsed) = parse_oci_url(&source.chart.repo_url) {
                return PreparedSource::Dependency(Dependency {
                    name: parsed.chart_name,
                    version: source.chart.target_revision.clone(),
                    repository: parsed.repository,
                    alias,
                    condition: None,
                });
            }
            return PreparedSource::Git;
        }

        match source.chart.chart.as_deref().filter(|s| !s.is_empty()) {
            Some(chart_name) => PreparedSource::Dependency(Dependency {
                name: chart_name.to_string(),
                version: source.chart.target_revision.clone(),
                repository: source.chart.repo_url.clone(),
                alias,
                condition: None,
            }),
            None => PreparedSource::Git,
        }
    }
}

/// Static helm engine instance. Avoids per-resolve allocation.
static HELM_ENGINE: HelmEngine = HelmEngine;

/// Resolve an engine by name. Returns `None` for unknown engines — callers
/// should surface that as a package-level error so users get a clear message.
pub fn resolve(name: &str) -> Option<&'static dyn Engine> {
    match name {
        "helm" => Some(&HELM_ENGINE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ChartRef;

    fn helm_src(chart: &str, url: &str) -> HelmSource {
        HelmSource {
            id: Some("id".to_string()),
            engine: None,
            chart: ChartRef {
                repo_url: url.to_string(),
                chart: Some(chart.to_string()),
                target_revision: "1.0.0".to_string(),
                path: None,
            },
            values: None,
        }
    }

    fn expect_dep(p: PreparedSource) -> Dependency {
        match p {
            PreparedSource::Dependency(d) => d,
            other => panic!("expected Dependency, got {other:?}"),
        }
    }

    #[test]
    fn helm_engine_prepares_http_source() {
        let e = HelmEngine;
        let s = helm_src("redis", "https://charts.example.com");
        let dep = expect_dep(e.prepare(&s));
        assert_eq!(dep.name, "redis");
        assert_eq!(dep.repository, "https://charts.example.com");
    }

    #[test]
    fn helm_engine_prepares_oci_source() {
        let e = HelmEngine;
        let mut s = helm_src("", "oci://ghcr.io/org/postgres");
        s.chart.chart = None;
        let dep = expect_dep(e.prepare(&s));
        assert_eq!(dep.name, "postgres");
        assert_eq!(dep.repository, "oci://ghcr.io/org");
    }

    #[test]
    fn helm_engine_classifies_git_like_source() {
        let e = HelmEngine;
        let mut s = helm_src("", "https://github.com/org/repo");
        s.chart.chart = None;
        s.chart.path = Some("charts/app".to_string());
        assert_eq!(e.prepare(&s), PreparedSource::Git);
    }

    #[test]
    fn resolve_returns_helm_engine_by_name() {
        let e = resolve("helm").unwrap();
        assert_eq!(e.name(), "helm");
    }

    #[test]
    fn resolve_returns_none_for_unknown_engine() {
        assert!(resolve("kcl").is_none());
        assert!(resolve("helmfile").is_none());
    }
}
