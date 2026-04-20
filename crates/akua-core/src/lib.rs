//! # akua-core
//!
//! The core pipeline for Akua: fetch sources, merge schemas, generate umbrella
//! charts, run WASM transforms, render manifests, and produce OCI-addressable
//! artifacts.
//!
//! ## Modules
//!
//! Pure algorithms:
//! - [`source`] — helm/kcl/helmfile source representation, chart-name extraction, alias.
//! - [`values`] — value merging with umbrella alias nesting, dot-notation paths.
//! - [`schema`] — JSON Schema merging with `x-user-input`/`x-input` extensions,
//!   CEL-based input transforms (time- and source-length-capped).
//! - [`umbrella`] — umbrella Helm chart assembly (Chart.yaml + merged values.yaml).
//! - [`manifest`] — `package.yaml` loader + v1alpha1 validation.
//! - [`metadata`] — `.akua/metadata.yaml` provenance sidecar.
//! - [`attest`] — SLSA v1 provenance predicate for cosign.
//! - [`engine`] — engine trait + built-in helm / kcl / helmfile engines.
//!
//! I/O (feature-gated):
//! - [`fetch`] — native OCI + HTTP Helm dep fetcher (SSRF guard, size caps,
//!   content-addressed cache).
//! - [`publish`] — native OCI push via `oci-client` (Helm v4 media types).
//! - [`render`] — embedded Helm v4 template engine (WASM) + legacy shell-out path.
//! - [`ssrf`] — private-IP-literal host rejection shared by all fetch paths.

#![allow(dead_code)]

pub mod attest;
pub mod cli_contract;
pub mod diff;
pub mod engine;
#[cfg(feature = "fetch")]
pub mod fetch;
pub(crate) mod hex;
pub mod lock_file;
pub mod manifest;
pub mod metadata;
pub mod mod_file;
#[cfg(feature = "engine-kcl")]
pub mod package_k;
#[cfg(feature = "engine-kcl")]
pub mod package_render;
#[cfg(feature = "publish")]
pub mod publish;
#[cfg(feature = "helm-cli")]
pub mod render;
pub mod schema;
pub mod source;
#[cfg(feature = "fetch")]
pub mod ssrf;
pub mod umbrella;
pub mod values;

#[cfg(test)]
pub(crate) mod test_util;

pub use attest::{build_provenance, SlsaProvenance};
pub use cli_contract::{AgentContext, AgentSource, ExitCode, StructuredError};
pub use lock_file::{
    AkuaLock, LockError, LockLoadError, LockedPackage, Replaced, CURRENT_VERSION as LOCK_VERSION,
};
pub use mod_file::{
    AkuaManifest, Dependency as ManifestDependency, DependencySource, ManifestError,
    ManifestLoadError, PackageSection, Replace, WorkspaceSection,
};
#[cfg(feature = "engine-kcl")]
pub use package_k::{OutputSpec, PackageK, PackageKError, RenderedPackage};
#[cfg(feature = "engine-kcl")]
pub use package_render::{
    render_outputs, OutputSummary, RenderError as PackageRenderError, RenderSummary,
    FORMAT_RAW_MANIFESTS,
};
pub use diff::{compare as compare_charts, ChartDiff, ChartSnapshot};
pub use engine::{Engine, EngineError, HelmEngine, PrepareContext, PreparedSource};
#[cfg(feature = "fetch")]
pub use fetch::{
    fetch_dependencies, fetch_dependencies_with_auth, fetch_dependencies_with_options,
    fetch_oci_manifest_digest, fetch_oci_manifest_digest_blocking, redact_userinfo, FetchError,
    FetchOptions, HttpError, LimitKind, OciAuth, OciRef, RegistryCredentials,
};
pub use manifest::{load_manifest, PackageManifest};
pub use metadata::{build_metadata, build_metadata_at, AkuaMetadata};
#[cfg(feature = "publish")]
pub use publish::{
    package_chart, publish_chart, PackageOutcome, PublishError, PublishOptions, PublishOutcome,
};
#[cfg(feature = "helm-wasm")]
pub use render::render_umbrella_embedded;
#[cfg(feature = "helm-cli")]
pub use render::{render_umbrella, write_metadata, write_umbrella, RenderError, RenderOptions};
pub use schema::{
    apply_input_transforms, extract_user_input_fields, merge_values_schemas, validate_values_schema,
    ExtractedUserInputField, JsonSchema,
};
pub use source::{
    extract_chart_name_from_oci, get_source_alias, is_oci, HelmBlock, HelmfileBlock, KclBlock,
    Source,
};
pub use umbrella::{
    build_umbrella_chart, build_umbrella_chart_in, BuildError, ChartYaml, Dependency, UmbrellaChart,
};
pub use values::{deep_merge_values, merge_source_values, set_nested_value};

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

