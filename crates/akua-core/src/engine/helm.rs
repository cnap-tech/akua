//! Helm engine — pass-through. The source is already a Helm chart living
//! in an HTTP repo or OCI registry; we just translate the block's fields
//! into an umbrella-chart [`Dependency`] entry.

use super::{Engine, EngineError, PrepareContext, PreparedSource};
use crate::source::{get_source_alias, parse_oci_url, HelmBlock, Source};
use crate::umbrella::Dependency;

pub const ENGINE_NAME: &str = "helm";

#[derive(Debug, Clone, Default)]
pub struct HelmEngine;

impl Engine for HelmEngine {
    fn name(&self) -> &'static str {
        ENGINE_NAME
    }

    fn prepare(
        &self,
        source: &Source,
        _ctx: &PrepareContext<'_>,
    ) -> Result<PreparedSource, EngineError> {
        let block = source.helm.as_ref().expect("dispatch ensures helm block");
        let alias = get_source_alias(source);
        Ok(PreparedSource::Dependency(build_dependency(block, alias)))
    }
}

fn build_dependency(block: &HelmBlock, alias: Option<String>) -> Dependency {
    // Two supported shapes:
    //   A) chart embedded in `repo` (OCI only, `oci://host/path/chart` with
    //      no explicit `chart:` field) — split the last segment off.
    //   B) chart passed explicitly (HTTP repos always, OCI when the chart
    //      lives under a multi-chart path like `oci://ghcr.io/org/charts`).
    let explicit_chart = block.chart.as_deref().filter(|s| !s.is_empty());
    if let Some(chart) = explicit_chart {
        return Dependency {
            name: chart.to_string(),
            version: block.version.clone(),
            repository: block.repo.trim_end_matches('/').to_string(),
            alias,
            condition: None,
        };
    }
    if let Some(parsed) = parse_oci_url(&block.repo) {
        return Dependency {
            name: parsed.chart_name,
            version: block.version.clone(),
            repository: parsed.repository,
            alias,
            condition: None,
        };
    }
    // HTTP repo with no chart name → last resort, use the repo as-is. The
    // fetcher will surface a clear error if the chart can't be located.
    Dependency {
        name: String::new(),
        version: block.version.clone(),
        repository: block.repo.trim_end_matches('/').to_string(),
        alias,
        condition: None,
    }
}

pub(crate) static HELM_ENGINE: HelmEngine = HelmEngine;

#[cfg(test)]
mod tests {
    use super::*;

    fn helm_src(chart: Option<&str>, repo: &str) -> Source {
        Source {
            name: "id".to_string(),
            helm: Some(HelmBlock {
                repo: repo.to_string(),
                chart: chart.map(String::from),
                version: "1.0.0".to_string(),
            }),
            kcl: None,
            helmfile: None,
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
        let s = helm_src(Some("redis"), "https://charts.example.com");
        let dep = expect_dep(HelmEngine.prepare(&s, &ctx()).unwrap());
        assert_eq!(dep.name, "redis");
        assert_eq!(dep.repository, "https://charts.example.com");
    }

    #[test]
    fn oci_source_chart_embedded_in_url() {
        let s = helm_src(None, "oci://ghcr.io/org/postgres");
        let dep = expect_dep(HelmEngine.prepare(&s, &ctx()).unwrap());
        assert_eq!(dep.name, "postgres");
        assert_eq!(dep.repository, "oci://ghcr.io/org");
    }

    #[test]
    fn oci_source_with_explicit_chart() {
        let s = helm_src(Some("podinfo"), "oci://ghcr.io/stefanprodan/charts");
        let dep = expect_dep(HelmEngine.prepare(&s, &ctx()).unwrap());
        assert_eq!(dep.name, "podinfo");
        assert_eq!(dep.repository, "oci://ghcr.io/stefanprodan/charts");
    }
}
