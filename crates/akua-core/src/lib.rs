//! # akua-core
//!
//! The core pipeline for Akua: fetch sources, merge schemas, generate umbrella
//! charts, run WASM transforms, render manifests, and produce OCI-addressable
//! artifacts.
//!
//! ## Status
//!
//! Pre-alpha. Phase 0 is landed: pure-algorithm utilities ported from CNAP's
//! private chart generation service. I/O (OCI fetch/push, Git fetch, Helm
//! render, Extism WASM host) is scaffolded but not yet implemented.
//!
//! ## Modules
//!
//! - [`hash`] — djb2 hash producing short base36 suffixes for deterministic aliases
//! - [`source`] — helm source representation, chart-name extraction, alias computation
//! - [`values`] — value merging with umbrella alias nesting, dot-notation paths, deep merge
//! - [`schema`] — JSON Schema merging with x-user-input extensions, field extraction, transforms
//! - [`umbrella`] — umbrella Helm chart assembly (Chart.yaml + merged values.yaml)
//!
//! ## Intentionally out of scope (Phase 0)
//!
//! The following are scaffolded in the pipeline module but return `unimplemented!`
//! until Phase 1:
//!
//! - `SourceFetcher` implementations (Git, OCI, HTTP Helm)
//! - Extism WASM plugin host
//! - Helm render (shell to `helm` binary or embed)
//! - OCI push via `oras`

#![allow(dead_code)]

pub mod engine;
pub mod hash;
pub mod manifest;
#[cfg(feature = "helm-cli")]
pub mod render;
pub mod schema;
pub mod source;
pub mod umbrella;
pub mod values;

pub use hash::hash_to_suffix;
pub use manifest::{load_manifest, PackageManifest};
#[cfg(feature = "helm-cli")]
pub use render::{render_umbrella, write_umbrella, RenderError, RenderOptions};
pub use schema::{
    apply_install_transforms, extract_install_fields, merge_values_schemas, validate_values_schema,
    ExtractedInstallField, JsonSchema,
};
pub use source::{extract_chart_name_from_oci, get_source_alias, is_oci, HelmSource};
pub use engine::{Engine, HelmEngine, PreparedSource, DEFAULT_ENGINE};
pub use umbrella::{build_umbrella_chart, BuildError, ChartYaml, Dependency, UmbrellaChart};
pub use values::{deep_merge_values, merge_helm_source_values, set_nested_value};

/// Top-level error type for the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("fetch error: {0}")]
    Fetch(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    #[test]
    fn re_exports_compile() {
        let _ = super::hash_to_suffix("test", 4);
    }
}
