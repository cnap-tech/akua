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
#[cfg(feature = "cosign-verify")]
use akua_core::cosign_sidecar::{self, SignSidecar};
use akua_core::oci_auth::CredsStore;
use akua_core::oci_pusher;
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct PushArgs<'a> {
    pub tarball: &'a Path,
    pub oci_ref: &'a str,
    pub tag: &'a str,
    /// Optional pre-signed sidecar (from `akua sign`). When present,
    /// the sidecar's `manifest_digest` is matched against the just-
    /// pushed manifest digest; mismatch rejects the upload. Compiled
    /// out without the `cosign-verify` feature.
    #[cfg(feature = "cosign-verify")]
    pub sig: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PushOutput {
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,
    pub tarball: PathBuf,
    /// Cosign `.sig` sidecar tag the registry now serves. `None`
    /// when `--sig` wasn't passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_tag: Option<String>,
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

    #[cfg(feature = "cosign-verify")]
    #[error(transparent)]
    Sidecar(#[from] cosign_sidecar::SidecarError),

    /// Sidecar was signed against a different manifest than what the
    /// registry just accepted. The push already succeeded; the sig
    /// upload is aborted so we don't attach a bogus signature. Most
    /// likely cause: sign host and push host ran different akua
    /// binary versions (see compute_publish_digests version-coupling
    /// note).
    #[cfg(feature = "cosign-verify")]
    #[error(
        "sidecar manifest digest `{sidecar}` doesn't match pushed manifest `{pushed}` — \
         signature was produced for a different artifact; skipping .sig upload"
    )]
    SidecarDigestMismatch { sidecar: String, pushed: String },

    #[cfg(feature = "cosign-verify")]
    #[error(
        "sidecar ref/tag (`{sidecar_ref}:{sidecar_tag}`) doesn't match push target \
         (`{push_ref}:{push_tag}`)"
    )]
    SidecarRefMismatch {
        sidecar_ref: String,
        sidecar_tag: String,
        push_ref: String,
        push_tag: String,
    },

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
                StructuredError::new(codes::E_PUBLISH_FAILED, inner.to_string()).with_default_docs()
            }
            #[cfg(feature = "cosign-verify")]
            PushError::Sidecar(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
            #[cfg(feature = "cosign-verify")]
            PushError::SidecarDigestMismatch { .. } | PushError::SidecarRefMismatch { .. } => {
                StructuredError::new(codes::E_COSIGN_VERIFY, self.to_string()).with_default_docs()
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
            _ => ExitCode::UserError,
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

    // Read + validate the sidecar BEFORE the push, so a mismatched
    // sidecar fails fast without uploading an orphan layer. The
    // ref/tag and locally-computed manifest digest are known without
    // network traffic; if any diverge, we abort.
    #[cfg(feature = "cosign-verify")]
    let sidecar = read_and_validate_sidecar(args, &bytes)?;

    let creds = CredsStore::load().map_err(|e| PushError::AuthConfig(e.to_string()))?;
    let pushed = oci_pusher::push(args.oci_ref, args.tag, &bytes, &creds)?;

    // Defense-in-depth: also check the registry-advertised digest
    // matches what the sidecar signed. `oci_pusher::push` computes
    // the digest from our manifest bytes (not a registry header),
    // so this equality is already guaranteed by
    // compute_publish_digests above — but asserting keeps the
    // invariant load-bearing if push's internals ever drift.
    #[cfg(feature = "cosign-verify")]
    let signature_tag = if let Some(s) = sidecar {
        if s.manifest_digest != pushed.manifest_digest {
            return Err(PushError::SidecarDigestMismatch {
                sidecar: s.manifest_digest,
                pushed: pushed.manifest_digest.clone(),
            });
        }
        let tag = oci_pusher::push_cosign_signature(
            &pushed.oci_ref,
            &pushed.manifest_digest,
            s.simple_signing_payload.as_bytes(),
            &s.signature_b64,
            &creds,
        )?;
        Some(tag)
    } else {
        None
    };

    #[cfg(not(feature = "cosign-verify"))]
    let signature_tag: Option<String> = None;

    let output = PushOutput {
        oci_ref: pushed.oci_ref,
        tag: pushed.tag,
        manifest_digest: pushed.manifest_digest,
        layer_digest: pushed.layer_digest,
        layer_size: pushed.layer_size,
        tarball: args.tarball.to_path_buf(),
        signature_tag,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PushError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

#[cfg(feature = "cosign-verify")]
fn read_and_validate_sidecar(
    args: &PushArgs<'_>,
    layer_bytes: &[u8],
) -> Result<Option<SignSidecar>, PushError> {
    let Some(path) = args.sig else {
        return Ok(None);
    };
    let s = SignSidecar::read_from(path)?;

    if s.oci_ref != args.oci_ref || s.tag != args.tag {
        return Err(PushError::SidecarRefMismatch {
            sidecar_ref: s.oci_ref,
            sidecar_tag: s.tag,
            push_ref: args.oci_ref.to_string(),
            push_tag: args.tag.to_string(),
        });
    }

    // Local digest from the same layer bytes we're about to push.
    // Any divergence here means the sidecar was signed against a
    // different tarball (or an akua version whose config blob has
    // moved).
    let expected = oci_pusher::compute_publish_digests(layer_bytes).manifest_digest;
    if s.manifest_digest != expected {
        return Err(PushError::SidecarDigestMismatch {
            sidecar: s.manifest_digest,
            pushed: expected,
        });
    }
    Ok(Some(s))
}

fn write_text<W: Write>(w: &mut W, out: &PushOutput) -> std::io::Result<()> {
    writeln!(w, "pushed: {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  tarball   {}", out.tarball.display())?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(
        w,
        "  layer     {} ({} bytes)",
        out.layer_digest, out.layer_size
    )?;
    if let Some(sig) = &out.signature_tag {
        writeln!(w, "  signed    {sig}")?;
    }
    Ok(())
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
            #[cfg(feature = "cosign-verify")]
            sig: None,
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
            #[cfg(feature = "cosign-verify")]
            sig: None,
        };
        let mut stdout = Vec::new();
        let err = run(&ctx_json(), &args, &mut stdout).unwrap_err();
        assert!(matches!(err, PushError::EmptyTarball { .. }), "got {err:?}");
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
            #[cfg(feature = "cosign-verify")]
            sig: None,
        };
        let mut stdout = Vec::new();
        let err = run(&ctx_json(), &args, &mut stdout).unwrap_err();
        assert!(matches!(err, PushError::Push(_)), "got {err:?}");
    }

    #[cfg(feature = "cosign-verify")]
    #[test]
    fn sidecar_with_wrong_ref_fails_before_push() {
        use akua_core::cosign_sidecar::SignSidecar;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.tgz");
        std::fs::write(&path, b"some-bytes-nonempty").unwrap();

        // Sidecar refers to a different repo than the push target.
        let sig_path = tmp.path().join("t.tgz.akuasig");
        SignSidecar {
            oci_ref: "oci://different.example/y".into(),
            tag: "0.1.0".into(),
            manifest_digest: "sha256:whatever".into(),
            simple_signing_payload: "{}".into(),
            signature_b64: "".into(),
            akua_version: "0.1.0".into(),
        }
        .write_to(&sig_path)
        .unwrap();

        let args = PushArgs {
            tarball: &path,
            oci_ref: "oci://registry.example/foo",
            tag: "0.1.0",
            sig: Some(&sig_path),
        };
        let err = run(&ctx_json(), &args, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(err, PushError::SidecarRefMismatch { .. }),
            "got {err:?}"
        );
    }

    #[cfg(feature = "cosign-verify")]
    #[test]
    fn sidecar_with_mismatched_manifest_digest_fails_before_push() {
        use akua_core::cosign_sidecar::SignSidecar;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.tgz");
        let layer = b"bytes-for-digest-derivation";
        std::fs::write(&path, layer).unwrap();

        // Ref/tag match push target, but manifest_digest is a lie.
        let sig_path = tmp.path().join("t.tgz.akuasig");
        SignSidecar {
            oci_ref: "oci://registry.example/foo".into(),
            tag: "0.1.0".into(),
            manifest_digest: "sha256:00000000000000000000".into(),
            simple_signing_payload: "{}".into(),
            signature_b64: "".into(),
            akua_version: "0.1.0".into(),
        }
        .write_to(&sig_path)
        .unwrap();

        let args = PushArgs {
            tarball: &path,
            oci_ref: "oci://registry.example/foo",
            tag: "0.1.0",
            sig: Some(&sig_path),
        };
        let err = run(&ctx_json(), &args, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(err, PushError::SidecarDigestMismatch { .. }),
            "got {err:?}"
        );
    }

    #[cfg(feature = "cosign-verify")]
    #[test]
    fn missing_sidecar_file_surfaces_sidecar_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.tgz");
        std::fs::write(&path, b"nonempty").unwrap();
        let missing_sig = tmp.path().join("gone.akuasig");

        let args = PushArgs {
            tarball: &path,
            oci_ref: "oci://registry.example/foo",
            tag: "0.1.0",
            sig: Some(&missing_sig),
        };
        let err = run(&ctx_json(), &args, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, PushError::Sidecar(_)), "got {err:?}");
    }
}
