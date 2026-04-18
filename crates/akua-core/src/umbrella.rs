//! Umbrella Helm chart assembly.
//!
//! Given a list of [`Source`]s, produces a Helm v2 `Chart.yaml` whose
//! `dependencies` list references each source (with alias matching the
//! values nesting done by [`merge_source_values`]) plus a merged
//! `values.yaml` object.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::path::Path;

use crate::engine::{self, EngineError, PrepareContext, PreparedSource};
use crate::source::Source;
use crate::values::merge_source_values;

/// Assembled umbrella chart ready for Helm render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UmbrellaChart {
    pub chart_yaml: ChartYaml,
    /// Merged values object, nested by alias. Serialized as `values.yaml`.
    pub values: Value,
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
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// Build an umbrella chart from a set of sources. Engines that need to
/// write materialised chart directories (KCL, helmfile) receive a
/// placeholder path — prefer [`build_umbrella_chart_in`] when any
/// non-helm source is present.
pub fn build_umbrella_chart(
    name: &str,
    version: &str,
    sources: &[Source],
) -> Result<UmbrellaChart, BuildError> {
    build_umbrella_chart_in(name, version, sources, Path::new("."))
}

/// Like [`build_umbrella_chart`] but accepts a `work_dir` where engines that
/// materialise local charts (KCL, helmfile) write their output.
pub fn build_umbrella_chart_in(
    name: &str,
    version: &str,
    sources: &[Source],
    work_dir: &Path,
) -> Result<UmbrellaChart, BuildError> {
    let mut dependencies = Vec::new();
    let ctx = PrepareContext { work_dir };

    for source in sources {
        match engine::prepare(source, &ctx)? {
            PreparedSource::Dependency(dep) => dependencies.push(dep),
            PreparedSource::LocalChart(path) => dependencies.push(Dependency {
                name: source.name.clone(),
                version: source_version(source),
                repository: format!("file://{}", path.display()),
                alias: crate::source::get_source_alias(source),
                condition: None,
            }),
        }
    }

    let values = merge_source_values(sources);

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
    })
}

/// Extract the version string from whatever engine block is set.
fn source_version(source: &Source) -> String {
    if let Some(h) = &source.helm {
        h.version.clone()
    } else if let Some(k) = &source.kcl {
        k.version.clone()
    } else if let Some(hf) = &source.helmfile {
        hf.version.clone()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::HelmBlock;
    use serde_json::json;

    fn build_umbrella_chart_unwrapped(
        name: &str,
        version: &str,
        sources: &[Source],
    ) -> UmbrellaChart {
        build_umbrella_chart(name, version, sources).expect("known engines only")
    }

    fn helm(name: &str, chart: &str, version: &str) -> Source {
        Source {
            name: name.to_string(),
            helm: Some(HelmBlock {
                repo: "https://charts.example.com".to_string(),
                chart: Some(chart.to_string()),
                version: version.to_string(),
            }),
            kcl: None,
            helmfile: None,
            values: Some(json!({"replicaCount": 1})),
        }
    }

    fn oci(name: &str, url: &str, version: &str) -> Source {
        Source {
            name: name.to_string(),
            helm: Some(HelmBlock {
                repo: url.to_string(),
                chart: None,
                version: version.to_string(),
            }),
            kcl: None,
            helmfile: None,
            values: None,
        }
    }

    #[test]
    fn empty_sources_produce_empty_deps() {
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &[]);
        assert_eq!(u.chart_yaml.name, "pkg");
        assert_eq!(u.chart_yaml.api_version, "v2");
        assert!(u.chart_yaml.dependencies.is_empty());
        assert_eq!(u.values, json!({}));
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
        assert_eq!(d.alias.as_deref(), Some("id_a"));

        // Values nested under the SAME alias.
        assert!(u.values.as_object().unwrap().contains_key("id_a"));
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
    fn mixed_sources_separate_correctly() {
        let sources = vec![
            helm("a", "redis", "7.0.0"),
            oci("b", "oci://ghcr.io/org/postgres", "15.0.0"),
        ];
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &sources);
        assert_eq!(u.chart_yaml.dependencies.len(), 2);

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
    fn two_same_chart_different_names_get_distinct_aliases() {
        let sources = vec![helm("a", "redis", "7.0.0"), helm("b", "redis", "7.0.0")];
        let u = build_umbrella_chart_unwrapped("pkg", "0.1.0", &sources);
        assert_eq!(u.chart_yaml.dependencies.len(), 2);
        let alias_a = u.chart_yaml.dependencies[0].alias.as_deref().unwrap();
        let alias_b = u.chart_yaml.dependencies[1].alias.as_deref().unwrap();
        assert_eq!(alias_a, "a");
        assert_eq!(alias_b, "b");
    }

    #[test]
    fn chart_yaml_serializes_to_helm_compatible_shape() {
        let s = helm("cache", "redis", "7.0.0");
        let u = build_umbrella_chart_unwrapped("my-pkg", "0.1.0", &[s]);
        let yaml = serde_yaml::to_string(&u.chart_yaml).unwrap();
        assert!(yaml.contains("apiVersion: v2"));
        assert!(yaml.contains("name: my-pkg"));
        assert!(yaml.contains("type: application"));
        assert!(yaml.contains("dependencies:"));
        assert!(yaml.contains("- name: redis"));
        assert!(yaml.contains("alias: cache"));
    }
}
