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
use crate::source::{Source, SourceKind};

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
    pub name: String,
    pub engine: String,
    pub origin: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformInfo {
    pub field: String,
    pub required: bool,
    /// Raw `x-input` bag from the schema, verbatim. We don't privilege
    /// Akua-reference keys (`cel`, `uniqueIn`) over third-party transform
    /// languages — whatever the author put there round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
}

/// Build a provenance block from a package's sources and extracted fields.
pub fn build_metadata(sources: &[Source], fields: &[ExtractedInstallField]) -> AkuaMetadata {
    build_metadata_at(sources, fields, rfc3339_now())
}

/// Like [`build_metadata`] but with an explicit `buildTime`. WASM hosts
/// lack `SystemTime::now()`, so callers in those environments must pass
/// the timestamp in (read `SOURCE_DATE_EPOCH` / `Date.now()` JS-side).
pub fn build_metadata_at(
    sources: &[Source],
    fields: &[ExtractedInstallField],
    build_time: String,
) -> AkuaMetadata {
    AkuaMetadata {
        akua: BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_time,
        },
        sources: sources.iter().map(source_info).collect(),
        transforms: fields.iter().map(field_to_transform).collect(),
    }
}

fn source_info(source: &Source) -> SourceInfo {
    let kind = source.kind().ok();
    let (engine, origin, version) = match kind {
        Some(SourceKind::Helm) => {
            let h = source.helm.as_ref().expect("kind matches");
            let origin = h
                .chart
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|c| format!("{}/{}", h.repo.trim_end_matches('/'), c))
                .unwrap_or_else(|| h.repo.clone());
            ("helm".to_string(), origin, h.version.clone())
        }
        Some(SourceKind::Kcl) => {
            let k = source.kcl.as_ref().expect("kind matches");
            (
                "kcl".to_string(),
                format!("file://{}", k.entrypoint),
                k.version.clone(),
            )
        }
        Some(SourceKind::Helmfile) => {
            let hf = source.helmfile.as_ref().expect("kind matches");
            (
                "helmfile".to_string(),
                format!("file://{}", hf.path),
                hf.version.clone(),
            )
        }
        None => ("<invalid>".to_string(), String::new(), String::new()),
    };
    SourceInfo {
        name: source.name.clone(),
        engine,
        origin,
        version,
        alias: crate::source::get_source_alias(source),
    }
}

fn field_to_transform(field: &ExtractedInstallField) -> TransformInfo {
    TransformInfo {
        field: field.path.clone(),
        required: field.required,
        input: field.schema.get("x-input").cloned(),
    }
}

/// RFC3339 build timestamp. Honours `SOURCE_DATE_EPOCH` (per the
/// reproducible-builds spec) so byte-identical metadata is possible
/// across hosts — important for OCI digest stability.
fn rfc3339_now() -> String {
    let instant = source_date_epoch().unwrap_or_else(SystemTime::now);
    OffsetDateTime::from(instant)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn source_date_epoch() -> Option<SystemTime> {
    let secs: u64 = std::env::var("SOURCE_DATE_EPOCH")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{HelmBlock, KclBlock};
    use crate::test_util::ScopedEnvVar;
    use serde_json::json;

    fn helm_source(name: &str, chart: &str, version: &str) -> Source {
        Source {
            name: name.to_string(),
            helm: Some(HelmBlock {
                repo: "https://charts.example.com".to_string(),
                chart: Some(chart.to_string()),
                version: version.to_string(),
            }),
            kcl: None,
            helmfile: None,
            values: None,
        }
    }

    fn kcl_source(name: &str, entrypoint: &str) -> Source {
        Source {
            name: name.to_string(),
            helm: None,
            kcl: Some(KclBlock {
                entrypoint: entrypoint.to_string(),
                version: "0.1.0".to_string(),
            }),
            helmfile: None,
            values: None,
        }
    }

    fn field(path: &str, cel: Option<&str>) -> ExtractedInstallField {
        let schema = match cel {
            Some(expr) => json!({ "x-user-input": true, "x-input": { "cel": expr } }),
            None => json!({ "x-user-input": true }),
        };
        ExtractedInstallField {
            path: path.to_string(),
            schema,
            required: false,
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
    fn source_info_helm_default() {
        let s = helm_source("app", "redis", "7.0.0");
        let meta = build_metadata(&[s], &[]);
        assert_eq!(meta.sources.len(), 1);
        assert_eq!(meta.sources[0].engine, "helm");
        assert_eq!(meta.sources[0].origin, "https://charts.example.com/redis");
        assert_eq!(meta.sources[0].version, "7.0.0");
    }

    #[test]
    fn source_info_kcl() {
        let s = kcl_source("hello", "./app.k");
        let meta = build_metadata(&[s], &[]);
        assert_eq!(meta.sources[0].engine, "kcl");
        assert_eq!(meta.sources[0].origin, "file://./app.k");
    }

    #[test]
    fn transforms_capture_raw_input_bag() {
        // x-input here uses Akua-reference keys (cel, uniqueIn) but the
        // metadata layer doesn't privilege them — the whole bag round-trips.
        let f = ExtractedInstallField {
            path: "httpRoute.hostname".to_string(),
            schema: json!({
                "x-user-input": true,
                "x-input": {
                    "cel": "slugify(value) + '.apps.example.com'",
                    "uniqueIn": "tenant.hostnames",
                }
            }),
            required: true,
        };
        let meta = build_metadata(&[], &[f]);
        assert_eq!(meta.transforms.len(), 1);
        assert_eq!(meta.transforms[0].field, "httpRoute.hostname");
        assert!(meta.transforms[0].required);
        assert_eq!(
            meta.transforms[0].input.as_ref().unwrap(),
            &json!({
                "cel": "slugify(value) + '.apps.example.com'",
                "uniqueIn": "tenant.hostnames",
            })
        );
    }

    #[test]
    fn transforms_preserve_unknown_input_keys() {
        // Bundle authored with a non-Akua transform language — make sure
        // we don't silently drop the bag's keys.
        let f = ExtractedInstallField {
            path: "region".to_string(),
            schema: json!({
                "x-user-input": true,
                "x-input": { "jsonnet": "std.asciiLower(value)", "custom": true }
            }),
            required: false,
        };
        let meta = build_metadata(&[], &[f]);
        let bag = meta.transforms[0].input.as_ref().unwrap();
        assert_eq!(bag.get("jsonnet").unwrap(), "std.asciiLower(value)");
        assert_eq!(bag.get("custom").unwrap(), &json!(true));
    }

    #[test]
    fn source_date_epoch_produces_deterministic_timestamp() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVar::set("SOURCE_DATE_EPOCH", "1700000000");
        assert_eq!(rfc3339_now(), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn source_date_epoch_ignored_when_invalid() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVar::set("SOURCE_DATE_EPOCH", "not-a-number");
        let stamp = rfc3339_now();
        assert!(stamp.contains('T'), "expected RFC3339, got {stamp}");
        assert!(
            !stamp.starts_with("1970-"),
            "fallback should use real clock, got {stamp}"
        );
    }

    #[test]
    fn source_date_epoch_absent_uses_wall_clock() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVar::remove("SOURCE_DATE_EPOCH");
        assert!(!rfc3339_now().starts_with("1970-"));
    }

    /// Env vars are process-global; the three SOURCE_DATE_EPOCH tests
    /// serialize through this so they don't race each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn transforms_omit_input_when_absent() {
        let f = ExtractedInstallField {
            path: "plain".to_string(),
            schema: json!({ "x-user-input": true, "type": "string" }),
            required: false,
        };
        let meta = build_metadata(&[], &[f]);
        assert!(meta.transforms[0].input.is_none());
    }

    #[test]
    fn serializes_to_yaml() {
        let s = helm_source("app", "redis", "7.0.0");
        let f = field("name", Some("value"));
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
