//! Source representation — one entry under `sources:` in `package.yaml`.
//!
//! Each source carries:
//! - a `name` (required, immutable-by-convention — drives deterministic alias
//!   computation so two Redis instances in one package don't collide)
//! - exactly one engine block (`helm`, `kcl`, or `helmfile`) that carries
//!   engine-specific fields
//! - optional `values` merged into the rendered chart
//!
//! Engine dispatch is by block presence, not by a separate `engine:` string.
//! Strict parse-time validation enforces "exactly one block". See
//! `docs/design-package-yaml-v1.md` for the full rationale.

use serde::{Deserialize, Serialize};

/// A package source. Exactly one of `helm`, `kcl`, `helmfile` is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Stable identifier. Drives deterministic alias computation. Treated as
    /// immutable-by-convention after first publish; see design doc.
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helm: Option<HelmBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kcl: Option<KclBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helmfile: Option<HelmfileBlock>,

    /// Default values for the source. Deep-merged with any install-time
    /// overrides; arrays replace rather than merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<serde_json::Value>,
}

/// Helm engine block — pulls an existing chart from an HTTP repo or OCI
/// registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HelmBlock {
    /// Repository URL. `https://...` for HTTP Helm repos,
    /// `oci://host/path` for OCI registries.
    pub repo: String,
    /// Chart name. Required when `repo` does not already include it (HTTP
    /// repos; OCI repos where the chart name is not the last path segment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    /// Chart version to pin. Exact versions only — no ranges.
    pub version: String,
}

/// KCL engine block — compiles a `.k` entrypoint to Kubernetes YAML at
/// authoring time and wraps the output as a static Helm chart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KclBlock {
    /// Path to the `.k` entrypoint, relative to the package directory.
    pub entrypoint: String,
    /// Chart version for the generated subchart.
    pub version: String,
}

/// helmfile engine block — runs `helmfile template` against the referenced
/// file and wraps the rendered YAML as a static Helm chart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HelmfileBlock {
    /// Path to the `helmfile.yaml`, relative to the package directory.
    pub path: String,
    /// Chart version for the generated subchart.
    pub version: String,
}

/// Errors raised during source-level validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SourceValidationError {
    #[error("source `{name}` declares no engine block (expected one of: helm, kcl, helmfile)")]
    NoEngine { name: String },
    #[error(
        "source `{name}` declares both `{first}` and `{second}`; exactly one engine block allowed"
    )]
    MultipleEngines {
        name: String,
        first: &'static str,
        second: &'static str,
    },
}

/// Which engine a source uses, determined by block presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Helm,
    Kcl,
    Helmfile,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Helm => "helm",
            SourceKind::Kcl => "kcl",
            SourceKind::Helmfile => "helmfile",
        }
    }
}

impl Source {
    /// Return the engine kind, or an error if zero/multiple blocks are set.
    pub fn kind(&self) -> Result<SourceKind, SourceValidationError> {
        let blocks: [(bool, &'static str); 3] = [
            (self.helm.is_some(), "helm"),
            (self.kcl.is_some(), "kcl"),
            (self.helmfile.is_some(), "helmfile"),
        ];
        let set: Vec<&'static str> = blocks.iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
        match set.as_slice() {
            [] => Err(SourceValidationError::NoEngine {
                name: self.name.clone(),
            }),
            [only] => Ok(match *only {
                "helm" => SourceKind::Helm,
                "kcl" => SourceKind::Kcl,
                "helmfile" => SourceKind::Helmfile,
                _ => unreachable!(),
            }),
            [first, second, ..] => Err(SourceValidationError::MultipleEngines {
                name: self.name.clone(),
                first,
                second,
            }),
        }
    }
}

/// Returns true if the URL is an OCI registry URL (`oci://...`).
pub fn is_oci(url: &str) -> bool {
    url.starts_with("oci://")
}

/// Extract the chart name from an OCI repository URL when the chart name
/// is embedded as the last path segment.
///
/// Returns `None` if the URL is not OCI, unparseable, or has no path.
pub fn extract_chart_name_from_oci(repo_url: &str) -> Option<String> {
    if !is_oci(repo_url) {
        return None;
    }
    let without_scheme = repo_url.strip_prefix("oci://")?;
    let (_host, path) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    let last_segment = path.split('/').rfind(|s| !s.is_empty())?;
    let without_tag = last_segment.split(':').next()?;
    if without_tag.is_empty() {
        None
    } else {
        Some(without_tag.to_string())
    }
}

/// Extract a chart name for a helm-block source. Prefers the explicit
/// `chart:` field; falls back to the last path segment of an OCI URL.
/// Returns `None` for non-helm sources.
pub fn get_chart_name_from_source(source: &Source) -> Option<String> {
    let helm = source.helm.as_ref()?;
    if let Some(chart) = &helm.chart {
        if !chart.is_empty() {
            return Some(chart.clone());
        }
    }
    extract_chart_name_from_oci(&helm.repo)
}

/// Compute the deterministic alias for a source used as a chart dependency.
///
/// Uniformly `"<source-name>"` regardless of engine. Rationale:
///
/// - **Predictable** — whatever the author writes as `name:` in
///   `package.yaml` is exactly what they see as the alias, the values
///   path, and the resource-name prefix. No hidden join.
/// - **Consistent across engines** — helm sources and KCL / helmfile
///   sources pick aliases the same way, so mental model stays simple.
/// - **Uniqueness already enforced** — manifest validation rejects
///   duplicate `name` within a package, so the alias is collision-free
///   without the chart-name suffix we used to prepend.
pub fn get_source_alias(source: &Source) -> Option<String> {
    source.kind().ok().map(|_| source.name.clone())
}

/// Parsed OCI reference. `tag` is `Some(...)` when the input ended in
/// `:version`; callers that require a tag (e.g. `akua inspect oci://…`)
/// assert that separately. [`parse_oci_url`] is the shared entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOciUrl {
    pub chart_name: String,
    pub repository: String,
    pub tag: Option<String>,
}

/// Parse an `oci://host/path/chart[:tag]` URL into its components.
///
/// - `repository` = `oci://host/<everything-before-chart>` (no trailing slash).
/// - `chart_name` = last path segment, tag stripped.
/// - `tag` = `Some(...)` iff the input carried `:version`, else `None`.
///
/// Returns `None` for unparseable / non-OCI input.
pub fn parse_oci_url(repo_url: &str) -> Option<ParsedOciUrl> {
    if !is_oci(repo_url) {
        return None;
    }
    let without_scheme = repo_url.strip_prefix("oci://")?;
    let (host, path) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    let last = segments[segments.len() - 1];
    let (chart_raw, tag) = match last.split_once(':') {
        // Trailing colon with empty tag: treat as untagged but still
        // strip the colon from the chart name.
        Some((name, tag)) => (name, (!tag.is_empty()).then(|| tag.to_string())),
        None => (last, None),
    };
    if chart_raw.is_empty() {
        return None;
    }
    let parent = segments[..segments.len() - 1].join("/");
    let repository = if parent.is_empty() {
        format!("oci://{host}")
    } else {
        format!("oci://{host}/{parent}")
    };
    Some(ParsedOciUrl {
        chart_name: chart_raw.to_string(),
        repository,
        tag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: construct a helm-block source.
    pub fn helm_source(name: &str, repo: &str, chart: Option<&str>, version: &str) -> Source {
        Source {
            name: name.to_string(),
            helm: Some(HelmBlock {
                repo: repo.to_string(),
                chart: chart.map(String::from),
                version: version.to_string(),
            }),
            kcl: None,
            helmfile: None,
            values: None,
        }
    }

    #[test]
    fn is_oci_recognizes_scheme() {
        assert!(is_oci("oci://ghcr.io/org/chart"));
        assert!(!is_oci("https://charts.example.com"));
        assert!(!is_oci("git://github.com/org/repo"));
        assert!(!is_oci(""));
    }

    #[test]
    fn chart_name_from_oci_url() {
        assert_eq!(
            extract_chart_name_from_oci("oci://ghcr.io/cnap-tech/charts/cilium"),
            Some("cilium".to_string())
        );
    }

    #[test]
    fn chart_name_from_oci_strips_tag() {
        assert_eq!(
            extract_chart_name_from_oci("oci://ghcr.io/cnap-tech/charts/cilium:1.2.0"),
            Some("cilium".to_string())
        );
    }

    #[test]
    fn chart_name_returns_none_for_non_oci() {
        assert_eq!(
            extract_chart_name_from_oci("https://charts.example.com/chart"),
            None
        );
    }

    #[test]
    fn chart_name_prefers_explicit() {
        let s = helm_source("id1", "https://charts.example.com", Some("redis"), "1.0.0");
        assert_eq!(get_chart_name_from_source(&s), Some("redis".to_string()));
    }

    #[test]
    fn chart_name_falls_back_to_oci_last_segment() {
        let s = helm_source("id1", "oci://ghcr.io/org/postgres", None, "1.0.0");
        assert_eq!(get_chart_name_from_source(&s), Some("postgres".to_string()));
    }

    #[test]
    fn helm_alias_is_source_name() {
        // Alias is whatever the author wrote as `name:` — no chart-name
        // prefix, no hash. Matches KCL/helmfile so the rule is uniform.
        let s = helm_source("web", "https://charts.example.com", Some("redis"), "1.0.0");
        assert_eq!(get_source_alias(&s), Some("web".to_string()));
    }

    #[test]
    fn helm_alias_ignores_chart_name_and_url_shape() {
        // Changing chart name or repo shape doesn't affect the alias —
        // only `source.name` drives it. Regression guard.
        let oci = helm_source("cache", "oci://ghcr.io/org/postgres", None, "1.0.0");
        assert_eq!(get_source_alias(&oci), Some("cache".to_string()));
        let http = helm_source(
            "cache",
            "https://charts.example.com",
            Some("postgres"),
            "1.0.0",
        );
        assert_eq!(get_source_alias(&http), Some("cache".to_string()));
    }

    #[test]
    fn kcl_alias_is_source_name() {
        let s = Source {
            name: "my-kcl".to_string(),
            helm: None,
            kcl: Some(KclBlock {
                entrypoint: "./app.k".to_string(),
                version: "0.1.0".to_string(),
            }),
            helmfile: None,
            values: None,
        };
        assert_eq!(get_source_alias(&s), Some("my-kcl".to_string()));
    }

    #[test]
    fn kind_detects_single_engine() {
        let h = helm_source("a", "https://x", Some("y"), "1.0.0");
        assert_eq!(h.kind().unwrap(), SourceKind::Helm);

        let k = Source {
            name: "a".into(),
            helm: None,
            kcl: Some(KclBlock {
                entrypoint: "./a.k".into(),
                version: "1.0.0".into(),
            }),
            helmfile: None,
            values: None,
        };
        assert_eq!(k.kind().unwrap(), SourceKind::Kcl);
    }

    #[test]
    fn kind_rejects_zero_engines() {
        let s = Source {
            name: "a".into(),
            helm: None,
            kcl: None,
            helmfile: None,
            values: None,
        };
        assert!(matches!(
            s.kind(),
            Err(SourceValidationError::NoEngine { .. })
        ));
    }

    #[test]
    fn kind_rejects_multiple_engines() {
        let s = Source {
            name: "a".into(),
            helm: Some(HelmBlock {
                repo: "https://x".into(),
                chart: Some("y".into()),
                version: "1.0.0".into(),
            }),
            kcl: Some(KclBlock {
                entrypoint: "./a.k".into(),
                version: "1.0.0".into(),
            }),
            helmfile: None,
            values: None,
        };
        assert!(matches!(
            s.kind(),
            Err(SourceValidationError::MultipleEngines { .. })
        ));
    }

    #[test]
    fn parse_oci_url_basic() {
        let p = parse_oci_url("oci://ghcr.io/cnap-tech/charts/cilium").unwrap();
        assert_eq!(p.chart_name, "cilium");
        assert_eq!(p.repository, "oci://ghcr.io/cnap-tech/charts");
        assert!(p.tag.is_none());
    }

    #[test]
    fn parse_oci_url_with_tag() {
        let p = parse_oci_url("oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1").unwrap();
        assert_eq!(p.chart_name, "podinfo");
        assert_eq!(p.repository, "oci://ghcr.io/stefanprodan/charts");
        assert_eq!(p.tag.as_deref(), Some("6.7.1"));
    }

    #[test]
    fn parse_oci_url_single_path_segment() {
        let p = parse_oci_url("oci://ghcr.io/onlychart").unwrap();
        assert_eq!(p.chart_name, "onlychart");
        assert_eq!(p.repository, "oci://ghcr.io");
        assert!(p.tag.is_none());
    }

    #[test]
    fn parse_oci_url_single_segment_with_tag() {
        let p = parse_oci_url("oci://registry.local/mychart:1.0.0").unwrap();
        assert_eq!(p.chart_name, "mychart");
        assert_eq!(p.repository, "oci://registry.local");
        assert_eq!(p.tag.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn parse_oci_url_empty_tag_drops_tag() {
        let p = parse_oci_url("oci://registry.local/mychart:").unwrap();
        assert_eq!(p.chart_name, "mychart");
        assert!(p.tag.is_none());
    }

    #[test]
    fn parse_oci_url_none_for_non_oci() {
        assert!(parse_oci_url("https://example.com/foo").is_none());
    }

    #[test]
    fn deny_unknown_fields_on_source() {
        let yaml = "name: x\nhelm: { repo: https://a, version: 1.0.0 }\nengine: helm\n";
        let err = serde_yaml::from_str::<Source>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn deny_unknown_fields_on_helm_block() {
        let yaml = "name: x\nhelm: { repo: https://a, version: 1.0.0, repoUrl: https://b }\n";
        let err = serde_yaml::from_str::<Source>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }
}
