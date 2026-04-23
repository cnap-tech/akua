//! `akua pull` — retrieve a published akua Package from an OCI registry
//! and extract it to a target directory.
//!
//! Inverse of `akua publish`. The resolved manifest digest is emitted
//! to stdout so callers can pin it in downstream automation (CI
//! scripts, `akua.lock` entries, etc).

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::oci_auth::CredsStore;
use akua_core::{oci_puller, package_tar};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct PullArgs<'a> {
    /// `oci://<registry>/<repo>` of the published akua Package.
    pub oci_ref: &'a str,

    /// Tag to pull. Required — unlike `akua publish`, there's no
    /// workspace-local default to fall back to.
    pub tag: &'a str,

    /// Target directory. Tarball is extracted here; dir is created
    /// if absent. Existing files inside are overwritten last-pull-wins.
    pub out: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PullOutput {
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub out: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error("reading auth config: {0}")]
    AuthConfig(String),

    #[error(transparent)]
    Pull(#[from] oci_puller::OciPullError),

    #[error(transparent)]
    Unpack(#[from] package_tar::PackageTarError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl PullError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            PullError::AuthConfig(detail) => {
                StructuredError::new(codes::E_IO, detail.clone()).with_default_docs()
            }
            PullError::Pull(inner) => {
                StructuredError::new(codes::E_PULL_FAILED, inner.to_string()).with_default_docs()
            }
            PullError::Unpack(inner) => {
                StructuredError::new(codes::E_IO, inner.to_string()).with_default_docs()
            }
            PullError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            PullError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &PullArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, PullError> {
    let creds = CredsStore::load().map_err(|e| PullError::AuthConfig(e.to_string()))?;

    let pulled = oci_puller::pull(args.oci_ref, args.tag, &creds)?;
    package_tar::unpack_to(&pulled.tarball, args.out)?;

    let output = PullOutput {
        oci_ref: args.oci_ref.to_string(),
        tag: args.tag.to_string(),
        manifest_digest: pulled.manifest_digest,
        layer_digest: pulled.layer_digest,
        out: args.out.to_path_buf(),
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PullError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, out: &PullOutput) -> std::io::Result<()> {
    writeln!(w, "pulled: {}:{} → {}", out.oci_ref, out.tag, out.out.display())?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(w, "  layer     {}", out.layer_digest)?;
    Ok(())
}
