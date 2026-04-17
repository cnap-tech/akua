//! Helm source representation and alias computation.
//!
//! Ported from `chart-generation.utils.ts`. Handles three repository types:
//!
//! - **Helm HTTP repo**: `chart.repo_url` is the base URL, `chart.chart` is the
//!   chart name, `chart.target_revision` is the version, `chart.path` is unused.
//! - **OCI registry**: `chart.repo_url` is a full `oci://` path where the last
//!   segment is the chart name. `chart.chart` and `chart.path` are unused.
//! - **Git repo**: `chart.repo_url` is a Git URL, `chart.path` is the chart
//!   path within the repo, `chart.target_revision` is a branch/tag/commit.
//!   `chart.chart` is unused.

use serde::{Deserialize, Serialize};

use crate::hash::hash_to_suffix;

/// A Helm source (chart reference + optional values + optional stable ID).
///
/// The `id` field, when present, enables deterministic aliasing via
/// [`get_source_alias`] — useful for umbrella charts that embed multiple
/// sources with potentially-colliding chart names (e.g., two Redis instances).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelmSource {
    /// Stable identifier for this source. If set, alias computation uses it
    /// so the alias is stable across runs even when unrelated sources change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Engine that produces a chart fragment from this source.
    /// Defaults to `"helm"` (pass-through — the source is already a chart).
    /// Unknown engines cause a package-level error at build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Chart reference (repo URL + chart name + revision + path).
    pub chart: ChartRef,
    /// Default values for the source. May be overridden at install time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<serde_json::Value>,
}

/// A chart reference across the three supported repository types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChartRef {
    pub repo_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    pub target_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Returns true if the URL is an OCI registry URL (`oci://...`).
pub fn is_oci(url: &str) -> bool {
    url.starts_with("oci://")
}

/// Extract the chart name from an OCI repository URL.
///
/// The chart name is the last path segment. Returns `None` if the URL is not
/// OCI, unparseable, or has no path segments.
///
/// ```
/// use akua_core::source::extract_chart_name_from_oci;
///
/// assert_eq!(
///     extract_chart_name_from_oci("oci://ghcr.io/cnap-tech/charts/cilium"),
///     Some("cilium".to_string())
/// );
/// assert_eq!(extract_chart_name_from_oci("https://example.com/foo"), None);
/// ```
pub fn extract_chart_name_from_oci(repo_url: &str) -> Option<String> {
    if !is_oci(repo_url) {
        return None;
    }
    // Strip scheme, split on '/', drop the host, filter empty segments.
    let without_scheme = repo_url.strip_prefix("oci://")?;
    // Everything after the first '/' is the path.
    let (_host, path) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    // Strip any trailing tag ("chart:v1.2.3" → "chart")
    let last_segment = path.split('/').rfind(|s| !s.is_empty())?;
    let without_tag = last_segment.split(':').next()?;
    if without_tag.is_empty() {
        None
    } else {
        Some(without_tag.to_string())
    }
}

/// Extract a chart name from any source (Helm HTTP or OCI).
///
/// Git sources have no chart name and return `None` — their values merge at
/// the umbrella chart's root rather than under an alias.
pub fn get_chart_name_from_source(source: &HelmSource) -> Option<String> {
    if let Some(chart) = &source.chart.chart {
        if !chart.is_empty() {
            return Some(chart.clone());
        }
    }
    extract_chart_name_from_oci(&source.chart.repo_url)
}

/// Compute the deterministic alias for a source when used as a chart dependency.
///
/// Returns `Some("<chart-name>-<hash>")` when the source has both an `id` and
/// a derivable chart name. Returns `None` for Git sources (no chart name) or
/// sources missing an ID.
///
/// The hash suffix stabilizes the alias across runs even if the chart-name
/// collides with another source (e.g., two Redis instances in the same
/// umbrella chart).
pub fn get_source_alias(source: &HelmSource) -> Option<String> {
    let id = source.id.as_ref()?;
    let chart_name = get_chart_name_from_source(source)?;
    let hash = hash_to_suffix(id, 4);
    Some(format!("{chart_name}-{hash}"))
}

/// Parsed OCI URL split into chart name and parent repository URL.
///
/// Useful for Helm OCI dependencies where `repository` must not include the
/// chart name — the chart name goes in the `name` field, the parent path goes
/// in `repository`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOciUrl {
    pub chart_name: String,
    pub repository: String,
}

/// Parse an OCI URL into `{chart_name, repository}`.
///
/// ```
/// use akua_core::source::parse_oci_url;
///
/// let p = parse_oci_url("oci://ghcr.io/cnap-tech/charts/cilium").unwrap();
/// assert_eq!(p.chart_name, "cilium");
/// assert_eq!(p.repository, "oci://ghcr.io/cnap-tech/charts");
/// ```
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
    let chart_name = last.split(':').next()?.to_string();
    if chart_name.is_empty() {
        return None;
    }
    let parent = segments[..segments.len() - 1].join("/");
    let repository = if parent.is_empty() {
        format!("oci://{host}")
    } else {
        format!("oci://{host}/{parent}")
    };
    Some(ParsedOciUrl {
        chart_name,
        repository,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: Option<&str>, repo: &str, chart: Option<&str>, path: Option<&str>) -> HelmSource {
        HelmSource {
            id: id.map(String::from),
            engine: None,
            chart: ChartRef {
                repo_url: repo.to_string(),
                chart: chart.map(String::from),
                target_revision: "1.0.0".to_string(),
                path: path.map(String::from),
            },
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
    fn chart_name_from_source_prefers_explicit_chart() {
        let s = src(
            Some("id1"),
            "https://charts.example.com",
            Some("redis"),
            None,
        );
        assert_eq!(get_chart_name_from_source(&s), Some("redis".to_string()));
    }

    #[test]
    fn chart_name_from_source_falls_back_to_oci() {
        let s = src(Some("id1"), "oci://ghcr.io/org/postgres", None, None);
        assert_eq!(get_chart_name_from_source(&s), Some("postgres".to_string()));
    }

    #[test]
    fn chart_name_from_source_returns_none_for_git() {
        let s = src(
            Some("id1"),
            "https://github.com/org/repo",
            None,
            Some("charts/app"),
        );
        assert_eq!(get_chart_name_from_source(&s), None);
    }

    #[test]
    fn alias_requires_id_and_chart_name() {
        // No ID → no alias.
        let s = src(None, "oci://ghcr.io/org/foo", None, None);
        assert_eq!(get_source_alias(&s), None);

        // No chart name (Git) → no alias.
        let s = src(
            Some("id1"),
            "https://github.com/org/repo",
            None,
            Some("charts/app"),
        );
        assert_eq!(get_source_alias(&s), None);
    }

    #[test]
    fn alias_is_chart_name_dash_hash() {
        let s = src(
            Some("thsrc_abc123"),
            "https://charts.example.com",
            Some("redis"),
            None,
        );
        let alias = get_source_alias(&s).unwrap();
        assert!(alias.starts_with("redis-"));
        assert_eq!(alias.len(), "redis-".len() + 4);
    }

    #[test]
    fn alias_is_stable_across_calls() {
        let s = src(
            Some("thsrc_abc123"),
            "oci://ghcr.io/org/postgres",
            None,
            None,
        );
        let a = get_source_alias(&s).unwrap();
        let b = get_source_alias(&s).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_oci_url_basic() {
        let p = parse_oci_url("oci://ghcr.io/cnap-tech/charts/cilium").unwrap();
        assert_eq!(p.chart_name, "cilium");
        assert_eq!(p.repository, "oci://ghcr.io/cnap-tech/charts");
    }

    #[test]
    fn parse_oci_url_single_path_segment() {
        let p = parse_oci_url("oci://ghcr.io/onlychart").unwrap();
        assert_eq!(p.chart_name, "onlychart");
        assert_eq!(p.repository, "oci://ghcr.io");
    }

    #[test]
    fn parse_oci_url_none_for_non_oci() {
        assert!(parse_oci_url("https://example.com/foo").is_none());
    }
}
