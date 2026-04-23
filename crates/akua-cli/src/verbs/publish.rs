//! `akua publish` — tarball the workspace + push it to an OCI registry.
//!
//! Phase 7 slice. The reciprocal of `akua add`: where add consumes
//! a registry-hosted chart, publish *produces* one. Shape:
//!
//! ```text
//! akua publish --ref oci://ghcr.io/acme/my-pkg [--tag 0.2.0]
//! ```
//!
//! Default tag is the `version` field of `[package]` in `akua.toml`,
//! so re-publishing the same workspace under an older ref is a
//! conscious `--tag` opt-in rather than the default.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::oci_auth::CredsStore;
use akua_core::{package_tar, oci_pusher, AkuaManifest, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct PublishArgs<'a> {
    pub workspace: &'a Path,

    /// Target repository — `oci://<registry>/<path/to/repo>`. Required.
    pub oci_ref: &'a str,

    /// Tag to publish under. `None` → use `[package].version`.
    pub tag: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublishOutput {
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error("reading auth config: {0}")]
    AuthConfig(String),

    #[error(transparent)]
    Tarball(#[from] package_tar::PackageTarError),

    #[error(transparent)]
    Push(#[from] oci_pusher::OciPushError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl PublishError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            PublishError::Manifest(e) => e.to_structured(),
            PublishError::AuthConfig(detail) => {
                StructuredError::new(codes::E_IO, detail.clone()).with_default_docs()
            }
            PublishError::Tarball(inner) => {
                StructuredError::new(codes::E_IO, inner.to_string()).with_default_docs()
            }
            PublishError::Push(inner) => {
                StructuredError::new(codes::E_PUBLISH_FAILED, inner.to_string())
                    .with_default_docs()
            }
            PublishError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            PublishError::Manifest(e) if e.is_system() => ExitCode::SystemError,
            PublishError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &PublishArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, PublishError> {
    let manifest = AkuaManifest::load(args.workspace)?;

    let tag = args
        .tag
        .map(str::to_string)
        .unwrap_or_else(|| manifest.package.version.clone());

    let creds = CredsStore::load().map_err(|e| PublishError::AuthConfig(e.to_string()))?;

    let tar_gz = package_tar::pack_workspace(args.workspace)?;

    let pushed = oci_pusher::push(args.oci_ref, &tag, &tar_gz, &creds)?;

    let output = PublishOutput {
        oci_ref: pushed.oci_ref,
        tag: pushed.tag,
        manifest_digest: pushed.manifest_digest,
        layer_digest: pushed.layer_digest,
        layer_size: pushed.layer_size,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PublishError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, out: &PublishOutput) -> std::io::Result<()> {
    writeln!(w, "published: {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(w, "  layer     {} ({} bytes)", out.layer_digest, out.layer_size)?;
    Ok(())
}

// Silence unused-import warning when only some paths use PathBuf.
#[allow(dead_code)]
const _: fn(&PathBuf) = |_| {};
