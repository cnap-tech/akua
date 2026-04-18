//! SLSA v1 provenance attestation.
//!
//! Emits a JSON document conforming to the SLSA v1 Provenance predicate
//! (<https://slsa.dev/spec/v1.0/provenance>). The output is an unsigned
//! in-toto statement — callers sign + push it as an adjacent OCI artifact
//! with `cosign attest --predicate <file> --type slsaprovenance1 <image>`
//! or `oras attach`.
//!
//! We emit only the predicate portion (not the full in-toto Statement with
//! subject). The subject is the chart's OCI digest, which only becomes
//! available after publish — so callers either:
//!
//! 1. `akua build` → get predicate JSON; `akua publish` → get digest;
//!    manually compose a Statement + cosign-sign, OR
//! 2. Use `cosign attest` which composes the Statement automatically from
//!    the pushed image digest and a predicate file.
//!
//! Option 2 is the recommended flow and what the README documents.

use serde::Serialize;

use crate::metadata::AkuaMetadata;

/// SLSA v1 provenance predicate. Serialized as the `predicate` field of an
/// in-toto Statement when published via cosign.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlsaProvenance {
    pub build_definition: BuildDefinition,
    pub run_details: RunDetails,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildDefinition {
    pub build_type: String,
    pub external_parameters: ExternalParameters,
    pub internal_parameters: InternalParameters,
    pub resolved_dependencies: Vec<ResolvedDependency>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalParameters {
    /// User-visible source of the build (package name + version).
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalParameters {
    pub akua_version: String,
    pub engines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedDependency {
    pub name: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetails {
    pub builder: Builder,
    pub metadata: RunMetadata,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Builder {
    pub id: String,
    pub version: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMetadata {
    pub invocation_id: String,
    pub started_on: String,
}

/// Build a SLSA v1 provenance predicate from Akua's build metadata.
///
/// The caller signs + pushes. Typical cosign workflow:
///
/// ```bash
/// akua attest --chart dist/chart --out attestation.json
/// akua publish --chart dist/chart --to oci://ghcr.io/acme/charts  # outputs digest
/// cosign attest \
///   --predicate attestation.json \
///   --type slsaprovenance1 \
///   ghcr.io/acme/charts/my-app@sha256:<digest>
/// ```
pub fn build_provenance(
    package_name: &str,
    package_version: &str,
    metadata: &AkuaMetadata,
) -> SlsaProvenance {
    let mut builder_version = std::collections::BTreeMap::new();
    builder_version.insert("akua-core".to_string(), metadata.akua.version.clone());

    let engines: Vec<String> = {
        let mut seen = std::collections::BTreeSet::new();
        for s in &metadata.sources {
            seen.insert(s.engine.clone());
        }
        seen.into_iter().collect()
    };

    let resolved = metadata
        .sources
        .iter()
        .map(|s| ResolvedDependency {
            name: s.name.clone(),
            uri: s.origin.clone(),
            version: Some(s.version.clone()),
        })
        .collect();

    SlsaProvenance {
        build_definition: BuildDefinition {
            build_type: "https://akua.dev/spec/v1/build-type".to_string(),
            external_parameters: ExternalParameters {
                source: format!("{package_name}@{package_version}"),
            },
            internal_parameters: InternalParameters {
                akua_version: metadata.akua.version.clone(),
                engines,
            },
            resolved_dependencies: resolved,
        },
        run_details: RunDetails {
            builder: Builder {
                id: "https://github.com/cnap-tech/akua".to_string(),
                version: builder_version,
            },
            metadata: RunMetadata {
                invocation_id: invocation_id(),
                started_on: metadata.akua.build_time.clone(),
            },
        },
    }
}

fn invocation_id() -> String {
    // Deterministic per-build id derived from build time + pid. Good enough
    // for audit trails; real reproducibility comes from content-addressing
    // the chart digest, not this field.
    format!(
        "akua-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{BuildInfo, SourceInfo};

    fn fixture_metadata() -> AkuaMetadata {
        AkuaMetadata {
            akua: BuildInfo {
                version: "0.3.0".to_string(),
                build_time: "2026-04-17T12:00:00Z".to_string(),
            },
            sources: vec![
                SourceInfo {
                    name: "redis".to_string(),
                    engine: "helm".to_string(),
                    origin: "https://charts.bitnami.com/bitnami/redis".to_string(),
                    version: "20.1.3".to_string(),
                    alias: Some("redis".to_string()),
                },
                SourceInfo {
                    name: "app".to_string(),
                    engine: "kcl".to_string(),
                    origin: "file://./app.k".to_string(),
                    version: "0.1.0".to_string(),
                    alias: None,
                },
            ],
            transforms: vec![],
        }
    }

    #[test]
    fn captures_external_parameters() {
        let p = build_provenance("my-pkg", "1.0.0", &fixture_metadata());
        assert_eq!(
            p.build_definition.external_parameters.source,
            "my-pkg@1.0.0"
        );
    }

    #[test]
    fn builder_id_is_akua_repo() {
        let p = build_provenance("pkg", "0.1.0", &fixture_metadata());
        assert!(p.run_details.builder.id.contains("cnap-tech/akua"));
        assert_eq!(
            p.run_details.builder.version.get("akua-core").unwrap(),
            "0.3.0"
        );
    }

    #[test]
    fn resolved_dependencies_capture_sources() {
        let p = build_provenance("pkg", "0.1.0", &fixture_metadata());
        assert_eq!(p.build_definition.resolved_dependencies.len(), 2);
        assert_eq!(p.build_definition.resolved_dependencies[0].name, "redis");
        assert_eq!(
            p.build_definition.resolved_dependencies[0]
                .version
                .as_deref(),
            Some("20.1.3")
        );
    }

    #[test]
    fn engines_are_deduped_and_sorted() {
        let mut meta = fixture_metadata();
        // Two helm sources + one kcl should collapse to ["helm", "kcl"]
        meta.sources.push(SourceInfo {
            name: "nginx".to_string(),
            engine: "helm".to_string(),
            origin: "https://charts.example.com/nginx".to_string(),
            version: "1.0.0".to_string(),
            alias: None,
        });
        let p = build_provenance("pkg", "0.1.0", &meta);
        assert_eq!(
            p.build_definition.internal_parameters.engines,
            vec!["helm", "kcl"]
        );
    }

    #[test]
    fn serializes_to_camel_case_json() {
        let p = build_provenance("pkg", "0.1.0", &fixture_metadata());
        let json = serde_json::to_string_pretty(&p).unwrap();
        assert!(json.contains("\"buildDefinition\""));
        assert!(json.contains("\"externalParameters\""));
        assert!(json.contains("\"resolvedDependencies\""));
        assert!(json.contains("\"runDetails\""));
        assert!(json.contains("\"buildType\""));
        // Should NOT contain snake_case:
        assert!(!json.contains("build_definition"));
    }
}
