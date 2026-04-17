//! Akua build-time provenance (`.akua/metadata.yaml`).
//!
//! Emitted alongside `Chart.yaml` during `akua build`, captures:
//!
//! - **Which Akua built the chart** — version, build time
//! - **Which sources went in** — engine, origin URL, version, alias
//! - **Which transforms ran** — field path, expression, whether applied
//!
//! Consumers: `akua inspect`, debuggability, supply-chain auditing,
//! reproducing builds. Strippable via `akua build --strip-metadata`.
//!
//! See `docs/design-notes.md` §8 for the full layer model (why this lives
//! in the chart, why SLSA attestations live as an adjacent OCI artifact).

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::schema::ExtractedInstallField;
use crate::source::HelmSource;

/// Root of `.akua/metadata.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AkuaMetadata {
    pub akua: BuildInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<TransformInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Version of the akua-core crate that emitted this metadata.
    pub version: String,
    /// RFC 3339 UTC timestamp.
    #[serde(rename = "buildTime")]
    pub build_time: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub engine: String,
    pub origin: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformInfo {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slugify: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_in: Option<String>,
    pub required: bool,
}

/// Build a provenance block from a package's sources and extracted fields.
pub fn build_metadata(sources: &[HelmSource], fields: &[ExtractedInstallField]) -> AkuaMetadata {
    AkuaMetadata {
        akua: BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_time: rfc3339_now(),
        },
        sources: sources.iter().map(source_info).collect(),
        transforms: fields.iter().map(field_to_transform).collect(),
    }
}

fn source_info(source: &HelmSource) -> SourceInfo {
    let engine = source
        .engine
        .clone()
        .unwrap_or_else(|| crate::engine::DEFAULT_ENGINE.to_string());
    let origin = source
        .chart
        .chart
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|chart| format!("{}/{}", source.chart.repo_url.trim_end_matches('/'), chart))
        .unwrap_or_else(|| source.chart.repo_url.clone());
    SourceInfo {
        id: source.id.clone().unwrap_or_else(|| "<unnamed>".to_string()),
        engine,
        origin,
        version: source.chart.target_revision.clone(),
        alias: crate::source::get_source_alias(source),
    }
}

fn field_to_transform(field: &ExtractedInstallField) -> TransformInfo {
    let expression = field
        .cel
        .clone()
        .or_else(|| field.hostname_template.clone());
    TransformInfo {
        field: field.path.clone(),
        expression,
        slugify: field.slugify,
        unique_in: field.unique_in.clone(),
        required: field.required,
    }
}

fn rfc3339_now() -> String {
    let now = SystemTime::now();
    OffsetDateTime::from(now)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ChartRef;
    use serde_json::json;

    fn helm_source(id: &str, chart: &str, version: &str, engine: Option<&str>) -> HelmSource {
        HelmSource {
            id: Some(id.to_string()),
            engine: engine.map(String::from),
            chart: ChartRef {
                repo_url: "https://charts.example.com".to_string(),
                chart: Some(chart.to_string()),
                target_revision: version.to_string(),
                path: None,
            },
            values: None,
        }
    }

    fn field(path: &str, cel: Option<&str>, slugify: bool) -> ExtractedInstallField {
        ExtractedInstallField {
            path: path.to_string(),
            schema: json!({}),
            required: false,
            hostname_template: None,
            cel: cel.map(String::from),
            slugify,
            unique_in: None,
            order: None,
        }
    }

    #[test]
    fn captures_akua_version_and_timestamp() {
        let meta = build_metadata(&[], &[]);
        assert_eq!(meta.akua.version, env!("CARGO_PKG_VERSION"));
        // RFC 3339 shape check (20-char prefix like "2026-04-17T12:34:56Z")
        assert!(meta.akua.build_time.len() >= 20);
        assert!(meta.akua.build_time.contains('T'));
    }

    #[test]
    fn source_info_default_engine_is_helm() {
        let s = helm_source("app", "redis", "7.0.0", None);
        let meta = build_metadata(&[s], &[]);
        assert_eq!(meta.sources.len(), 1);
        assert_eq!(meta.sources[0].engine, "helm");
        assert_eq!(meta.sources[0].origin, "https://charts.example.com/redis");
        assert_eq!(meta.sources[0].version, "7.0.0");
    }

    #[test]
    fn source_info_preserves_custom_engine() {
        let s = helm_source("app", "", "7.0.0", Some("kcl"));
        let meta = build_metadata(&[s], &[]);
        assert_eq!(meta.sources[0].engine, "kcl");
    }

    #[test]
    fn transforms_capture_cel_expression() {
        let f = field(
            "httpRoute.hostname",
            Some("value + '.apps.example.com'"),
            true,
        );
        let meta = build_metadata(&[], &[f]);
        assert_eq!(meta.transforms.len(), 1);
        assert_eq!(meta.transforms[0].field, "httpRoute.hostname");
        assert_eq!(
            meta.transforms[0].expression.as_deref(),
            Some("value + '.apps.example.com'")
        );
        assert!(meta.transforms[0].slugify);
    }

    #[test]
    fn serializes_to_yaml() {
        let s = helm_source("app", "redis", "7.0.0", None);
        let f = field("name", Some("value"), false);
        let meta = build_metadata(&[s], &[f]);
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(yaml.contains("akua:"));
        assert!(yaml.contains("version:"));
        assert!(yaml.contains("buildTime:"));
        assert!(yaml.contains("sources:"));
        assert!(yaml.contains("engine: helm"));
        assert!(yaml.contains("transforms:"));
    }
}
