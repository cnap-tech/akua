//! `akua push` — upload a pre-packed `.tar.gz` to an OCI registry.
//!
//! The push half of `akua publish`. Pair with `akua pack` to get the
//! air-gap workflow: pack on one host, transfer the tarball across a
//! boundary, push from another host.
//!
//! Deliberately minimal: no signing, no attestation, no workspace
//! read. `akua publish` is the all-in-one verb for the common case;
//! `akua push` is for operators who already have a tarball in hand.
//!
//! Unlike `akua publish`, `--tag` is required — the tarball has no
//! workspace-local default to fall back to. (We could extract
//! akua.toml from the archive to read the version, but that's a
//! surprising bit of magic for a verb whose contract is "push this
//! byte stream"; operators who want workspace-derived tags should
//! use `akua publish` directly.)

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::oci_auth::CredsStore;
use akua_core::oci_pusher;
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct PushArgs<'a> {
    pub tarball: &'a Path,
    pub oci_ref: &'a str,
    pub tag: &'a str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PushOutput {
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,
    pub tarball: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("reading tarball `{}`: {source}", path.display())]
    ReadTarball {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("tarball `{}` is empty — nothing to push", path.display())]
    EmptyTarball { path: PathBuf },

    #[error("reading auth config: {0}")]
    AuthConfig(String),

    #[error(transparent)]
    Push(#[from] oci_pusher::OciPushError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl PushError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            PushError::ReadTarball { source, .. } => {
                StructuredError::new(codes::E_IO, source.to_string()).with_default_docs()
            }
            PushError::EmptyTarball { path } => StructuredError::new(
                codes::E_IO,
                format!("tarball `{}` is empty", path.display()),
            )
            .with_default_docs(),
            PushError::AuthConfig(detail) => {
                StructuredError::new(codes::E_IO, detail.clone()).with_default_docs()
            }
            PushError::Push(inner) => {
                StructuredError::new(codes::E_PUBLISH_FAILED, inner.to_string())
                    .with_default_docs()
            }
            PushError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            PushError::StdoutWrite(_) => ExitCode::SystemError,
            PushError::Push(_) | PushError::AuthConfig(_) => ExitCode::SystemError,
            PushError::ReadTarball { .. } | PushError::EmptyTarball { .. } => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &PushArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, PushError> {
    let bytes = std::fs::read(args.tarball).map_err(|source| PushError::ReadTarball {
        path: args.tarball.to_path_buf(),
        source,
    })?;
    if bytes.is_empty() {
        return Err(PushError::EmptyTarball {
            path: args.tarball.to_path_buf(),
        });
    }

    let creds = CredsStore::load().map_err(|e| PushError::AuthConfig(e.to_string()))?;
    let pushed = oci_pusher::push(args.oci_ref, args.tag, &bytes, &creds)?;

    let output = PushOutput {
        oci_ref: pushed.oci_ref,
        tag: pushed.tag,
        manifest_digest: pushed.manifest_digest,
        layer_digest: pushed.layer_digest,
        layer_size: pushed.layer_size,
        tarball: args.tarball.to_path_buf(),
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PushError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, out: &PushOutput) -> std::io::Result<()> {
    writeln!(w, "pushed: {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  tarball   {}", out.tarball.display())?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(
        w,
        "  layer     {} ({} bytes)",
        out.layer_digest, out.layer_size
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    #[test]
    fn missing_tarball_surfaces_user_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.tgz");
        let args = PushArgs {
            tarball: &missing,
            oci_ref: "oci://registry.example/foo",
            tag: "0.1.0",
        };
        let mut stdout = Vec::new();
        let err = run(&ctx_json(), &args, &mut stdout).unwrap_err();
        assert!(matches!(err, PushError::ReadTarball { .. }));
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn empty_tarball_surfaces_user_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.tgz");
        std::fs::write(&path, b"").unwrap();
        let args = PushArgs {
            tarball: &path,
            oci_ref: "oci://registry.example/foo",
            tag: "0.1.0",
        };
        let mut stdout = Vec::new();
        let err = run(&ctx_json(), &args, &mut stdout).unwrap_err();
        assert!(
            matches!(err, PushError::EmptyTarball { .. }),
            "got {err:?}"
        );
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }

    #[test]
    fn malformed_oci_ref_surfaces_push_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.tgz");
        // Non-empty bytes — we want to get past the empty check and
        // trip the ref parser.
        std::fs::write(&path, b"not-really-a-tarball-but-non-empty").unwrap();
        let args = PushArgs {
            tarball: &path,
            oci_ref: "not-an-oci-ref",
            tag: "0.1.0",
        };
        let mut stdout = Vec::new();
        let err = run(&ctx_json(), &args, &mut stdout).unwrap_err();
        assert!(matches!(err, PushError::Push(_)), "got {err:?}");
    }

}
