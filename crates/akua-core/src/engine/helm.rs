//! Helm engine — pass-through. The source is already a Helm chart
//! (HTTP repo, OCI registry, or Git path); no materialisation needed.

use super::{Engine, EngineError, PrepareContext, PreparedSource, DEFAULT_ENGINE};
use crate::source::{get_source_alias, is_oci, parse_oci_url, HelmSource};
use crate::umbrella::Dependency;

#[derive(Debug, Clone, Default)]
pub struct HelmEngine;

impl Engine for HelmEngine {
    fn name(&self) -> &'static str {
        DEFAULT_ENGINE
    }

    fn prepare(
        &self,
        source: &HelmSource,
        _ctx: &PrepareContext<'_>,
    ) -> Result<PreparedSource, EngineError> {
        let alias = get_source_alias(source);

        if is_oci(&source.chart.repo_url) {
            if let Some(parsed) = parse_oci_url(&source.chart.repo_url) {
                return Ok(PreparedSource::Dependency(Dependency {
                    name: parsed.chart_name,
                    version: source.chart.target_revision.clone(),
                    repository: parsed.repository,
                    alias,
                    condition: None,
                }));
            }
            return Ok(PreparedSource::Git);
        }

        Ok(
            match source.chart.chart.as_deref().filter(|s| !s.is_empty()) {
                Some(chart_name) => PreparedSource::Dependency(Dependency {
                    name: chart_name.to_string(),
                    version: source.chart.target_revision.clone(),
                    repository: source.chart.repo_url.clone(),
                    alias,
                    condition: None,
                }),
                None => PreparedSource::Git,
            },
        )
    }
}

pub(crate) static HELM_ENGINE: HelmEngine = HelmEngine;

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

    fn ctx() -> PrepareContext<'static> {
        PrepareContext {
            work_dir: std::path::Path::new("/tmp"),
        }
    }

    #[test]
    fn http_source() {
        let e = HelmEngine;
        let s = helm_src("redis", "https://charts.example.com");
        let dep = expect_dep(e.prepare(&s, &ctx()).unwrap());
        assert_eq!(dep.name, "redis");
        assert_eq!(dep.repository, "https://charts.example.com");
    }

    #[test]
    fn oci_source() {
        let e = HelmEngine;
        let mut s = helm_src("", "oci://ghcr.io/org/postgres");
        s.chart.chart = None;
        let dep = expect_dep(e.prepare(&s, &ctx()).unwrap());
        assert_eq!(dep.name, "postgres");
        assert_eq!(dep.repository, "oci://ghcr.io/org");
    }

    #[test]
    fn git_like_source() {
        let e = HelmEngine;
        let mut s = helm_src("", "https://github.com/org/repo");
        s.chart.chart = None;
        s.chart.path = Some("charts/app".to_string());
        assert_eq!(e.prepare(&s, &ctx()).unwrap(), PreparedSource::Git);
    }
}
