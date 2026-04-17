//! Umbrella Helm chart assembly.
//!
//! Given a list of [`HelmSource`]s, produces a Helm v2 `Chart.yaml` whose
//! `dependencies` list references each source (with alias matching the
//! values nesting done by [`merge_helm_source_values`]) plus a merged
//! `values.yaml` object.
//!
//! Git sources are **not** representable as Helm dependencies — they're
//! surfaced via [`UmbrellaChart::git_sources`] for the caller to render
//! separately (e.g., clone + helm template against the checked-out path).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{self, PreparedSource, DEFAULT_ENGINE};
use crate::source::HelmSource;
use crate::values::merge_helm_source_values;

/// Assembled umbrella chart ready for Helm render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UmbrellaChart {
    pub chart_yaml: ChartYaml,
    /// Merged values object, nested by alias. Serialized as `values.yaml`.
    pub values: Value,
    /// Git sources that cannot be expressed as Helm dependencies.
    /// The caller renders these separately.
    pub git_sources: Vec<HelmSource>,
}

/// Helm `Chart.yaml` (apiVersion v2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartYaml {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub chart_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,
}

/// A single entry in `Chart.yaml`'s `dependencies:` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("source `{source_id}` specifies unknown engine `{engine}`")]
    UnknownEngine { source_id: String, engine: String },
}

/// Build an umbrella chart from a set of sources. Each source's `engine` field
/// selects the [`Engine`] impl that prepares its umbrella entry; unknown
/// engines surface as [`BuildError::UnknownEngine`].
pub fn build_umbrella_chart(
    name: &str,
    version: &str,
    sources: &[HelmSource],
) -> Result<UmbrellaChart, BuildError> {
    let mut dependencies = Vec::new();
    let mut git_sources = Vec::new();

    for source in sources {
        let engine_name = source.engine.as_deref().unwrap_or(DEFAULT_ENGINE);
        let engine = engine::resolve(engine_name).ok_or_else(|| BuildError::UnknownEngine {
            source_id: source.id.clone().unwrap_or_else(|| "<unnamed>".to_string()),
            engine: engine_name.to_string(),
        })?;
        match engine.prepare(source) {
            PreparedSource::Dependency(dep) => dependencies.push(dep),
            PreparedSource::Git => git_sources.push(source.clone()),
            PreparedSource::LocalChart(path) => dependencies.push(Dependency {
                name: source.id.clone().unwrap_or_else(|| "local".to_string()),
                version: source.chart.target_revision.clone(),
                repository: format!("file://{}", path.display()),
                alias: crate::source::get_source_alias(source),
                condition: None,
            }),
        }
    }

    let values = merge_helm_source_values(sources);

    Ok(UmbrellaChart {
        chart_yaml: ChartYaml {
            api_version: "v2".to_string(),
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            chart_type: Some("application".to_string()),
            dependencies,
        },
        values,
        git_sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ChartRef;
    use serde_json::json;

    fn build_umbrella_chart_unwrapped(
        name: &str,
        version: &str,
        sources: &[HelmSource],
    ) -> UmbrellaChart {
        build_umbrella_chart(name, version, sources).expect("known engines only")
    }

    fn helm(id: &str, chart: &str, version: &str) -> HelmSource {
        HelmSource {
            id: Some(id.to_string()),
            engine: None,
            chart: ChartRef {
                repo_url: "https://charts.example.com".to_string(),
                chart: Some(chart.to_string()),
                target_revision: version.to_string(),
                path: None,
            },
            values: Some(json!({"replicaCount": 1})),
        }
    }

    fn oci(id: &str, url: &str, version: &str) -> HelmSource {
        HelmSource {
            id: Some(id.to_string()),
            engine: None,
            chart: ChartRef {
                repo_url: url.to_string(),
                chart: None,
                target_revision: version.to_string(),
                path: None,
            },
            values: None,
        }
    }

    fn git(id: &str, url: &str, path: &str) -> HelmSource {
        HelmSource {
            id: Some(id.to_string()),
            engine: None,
            chart: ChartRef {
                repo_url: url.to_string(),
                chart: None,
                target_revision: "main".to_string(),
                path: Some(path.to_string()),
            },
            values: Some(json!({"global": true})),
        }
    }

    #[test]
    fn empty_sources_produce_empty_deps() {
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[]);
        assert_eq!(u.chart_yaml.name, "pkg");
        assert_eq!(u.chart_yaml.api_version, "v2");
        assert!(u.chart_yaml.dependencies.is_empty());
        assert_eq!(u.values, json!({}));
        assert!(u.git_sources.is_empty());
    }

    #[test]
    fn helm_http_source_becomes_dep_with_alias() {
        let s = helm("id_a", "redis", "7.0.0");
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[s]);
        assert_eq!(u.chart_yaml.dependencies.len(), 1);
        let d = &u.chart_yaml.dependencies[0];
        assert_eq!(d.name, "redis");
        assert_eq!(d.version, "7.0.0");
        assert_eq!(d.repository, "https://charts.example.com");
        let alias = d.alias.as_deref().unwrap();
        assert!(alias.starts_with("redis-"));

        // Values nested under the SAME alias.
        assert!(u.values.as_object().unwrap().contains_key(alias));
    }

    #[test]
    fn oci_source_splits_url() {
        let s = oci("id_o", "oci://ghcr.io/cnap-tech/charts/cilium", "1.15.0");
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[s]);
        let d = &u.chart_yaml.dependencies[0];
        assert_eq!(d.name, "cilium");
        assert_eq!(d.repository, "oci://ghcr.io/cnap-tech/charts");
        assert_eq!(d.version, "1.15.0");
    }

    #[test]
    fn git_source_is_split_out() {
        let g = git("g1", "https://github.com/org/repo", "charts/app");
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[g]);
        assert!(u.chart_yaml.dependencies.is_empty());
        assert_eq!(u.git_sources.len(), 1);
        // Git values land at root.
        assert_eq!(u.values, json!({"global": true}));
    }

    #[test]
    fn mixed_sources_separate_correctly() {
        let sources = vec![
            helm("a", "redis", "7.0.0"),
            oci("b", "oci://ghcr.io/org/postgres", "15.0.0"),
            git("c", "https://github.com/org/repo", "charts/app"),
        ];
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &sources);
        assert_eq!(u.chart_yaml.dependencies.len(), 2);
        assert_eq!(u.git_sources.len(), 1);

        let names: Vec<&str> = u
            .chart_yaml
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(names.contains(&"redis"));
        assert!(names.contains(&"postgres"));
    }

    #[test]
    fn two_same_chart_different_ids_get_distinct_aliases() {
        let sources = vec![helm("a", "redis", "7.0.0"), helm("b", "redis", "7.0.0")];
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &sources);
        assert_eq!(u.chart_yaml.dependencies.len(), 2);
        let alias_a = u.chart_yaml.dependencies[0].alias.as_deref().unwrap();
        let alias_b = u.chart_yaml.dependencies[1].alias.as_deref().unwrap();
        assert_ne!(alias_a, alias_b);
        assert!(alias_a.starts_with("redis-"));
        assert!(alias_b.starts_with("redis-"));
    }

    #[test]
    fn chart_yaml_serializes_to_helm_compatible_shape() {
        let s = helm("id", "redis", "7.0.0");
        let u = build_umbrella_chart_unwrapped("my-pkg", "0.1.0", &[s]);
        let yaml = serde_yaml::to_string(&u.chart_yaml).unwrap();
        assert!(yaml.contains("apiVersion: v2"));
        assert!(yaml.contains("name: my-pkg"));
        assert!(yaml.contains("type: application"));
        assert!(yaml.contains("dependencies:"));
        assert!(yaml.contains("- name: redis"));
        assert!(yaml.contains("alias: redis-"));
    }

    #[test]
    fn no_id_means_no_alias_field() {
        let mut s = helm("_", "redis", "7.0.0");
        s.id = None;
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[s]);
        assert!(u.chart_yaml.dependencies[0].alias.is_none());
    }
}
