//! # akua-core
//!
//! The core pipeline for Akua: fetch sources, merge schemas, generate umbrella
//! charts, run WASM transforms, render manifests, and produce OCI-addressable
//! artifacts.
//!
//! ## Pipeline stages
//!
//! 1. **Source fetch** — pluggable `SourceFetcher` trait; implementations for
//!    local Git, server PAT, CI tokens, and browser-proxy contexts.
//! 2. **Schema merge** — combine `values.schema.json` across components, honor
//!    `x-user-input` annotations.
//! 3. **Umbrella chart generation** — alias dependencies (`redis-ab12`), nest
//!    values under aliases, produce a valid Helm chart tarball.
//! 4. **Transform execution** — run Extism WASM plugins to resolve customer
//!    inputs into final values.
//! 5. **Validation** — schema + transform output + Helm render check.
//! 6. **Package assembly** — tar.gz umbrella chart + transforms bundle + schema
//!    snapshot + metadata.
//! 7. **OCI push** — content-addressed push to any OCI registry.
//!
//! ## Status
//!
//! Pre-alpha. Public API will change.

#![allow(dead_code)] // allow while stubs fill in

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Identifier for a source component within a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    pub kind: SourceKind,
    pub uri: String,
}

/// The kinds of sources a package can contain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    HelmRepo,
    OciRegistry,
    GitRepo,
    LocalPath,
    KnativeApp,
    RawManifests,
}

/// A fetched source, ready for pipeline processing.
#[derive(Debug)]
pub struct FetchedSource {
    pub source_ref: SourceRef,
    pub content: Vec<u8>,
}

/// Pluggable source fetcher. Different contexts (server, CI, local, browser)
/// provide different implementations with their own auth story.
#[async_trait]
pub trait SourceFetcher: Send + Sync {
    async fn fetch(&self, source_ref: &SourceRef) -> Result<FetchedSource, Error>;
}

/// The top-level package manifest produced by the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub components: Vec<Component>,
    pub user_inputs: serde_json::Value, // JSON Schema Draft 7
    pub transforms: Vec<TransformRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub source: SourceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformRef {
    pub name: String,
    pub wasm: Vec<u8>, // Extism-compatible
}

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

/// Build a package from sources + schema + transforms.
///
/// This is the main entry point. It runs all pipeline stages and returns the
/// assembled package ready for OCI push.
pub async fn build_package(
    _fetcher: &dyn SourceFetcher,
    _sources: &[SourceRef],
) -> Result<Package, Error> {
    // TODO: implement pipeline stages
    //   1. fetch_all(fetcher, sources)
    //   2. merge_schemas
    //   3. generate_umbrella
    //   4. execute_transforms (skipped if none)
    //   5. validate
    //   6. assemble
    unimplemented!("pipeline stages — implementation in progress; see milestone v4")
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {
        assert_eq!(2 + 2, 4);
    }
}
