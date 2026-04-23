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

    /// `--no-sign`: skip cosign signing even when a private key is
    /// configured in `akua.toml [signing]`. Defaults to `false`.
    pub no_sign: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublishOutput {
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,

    /// Tag the cosign sidecar was pushed under (`sha256-<hex>.sig`).
    /// `None` when the publish didn't sign.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_tag: Option<String>,
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

    #[error("reading cosign private key at {path}: {source}")]
    SigningKeyIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cosign signing: {0}")]
    Sign(akua_core::cosign::CosignError),

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
            PublishError::SigningKeyIo { path, source } => {
                StructuredError::new(codes::E_PUBLISH_FAILED, source.to_string())
                    .with_path(path.display().to_string())
                    .with_suggestion("akua.toml [signing].cosign_private_key must resolve to a PEM-encoded PKCS#8 P-256 private key file.")
                    .with_default_docs()
            }
            PublishError::Sign(inner) => {
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

    // Optional cosign-sign the just-published manifest. Skipped when
    // the manifest has no [signing].cosign_private_key or the caller
    // passed --no-sign. Signing failures abort — the artifact is
    // already up, but we never want an ambiguously-"signed" publish.
    let signature_tag = if args.no_sign {
        None
    } else {
        sign_published_artifact(
            args.workspace,
            &manifest,
            args.oci_ref,
            &pushed.manifest_digest,
            &creds,
        )?
    };

    let output = PublishOutput {
        oci_ref: pushed.oci_ref,
        tag: pushed.tag,
        manifest_digest: pushed.manifest_digest,
        layer_digest: pushed.layer_digest,
        layer_size: pushed.layer_size,
        signature_tag,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PublishError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, out: &PublishOutput) -> std::io::Result<()> {
    writeln!(w, "published: {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(w, "  layer     {} ({} bytes)", out.layer_digest, out.layer_size)?;
    if let Some(sig_tag) = &out.signature_tag {
        writeln!(w, "  signed    {}", sig_tag)?;
    }
    Ok(())
}

/// Read the private key referenced by `[signing].cosign_private_key`
/// (if any), build the simple-signing payload for `manifest_digest`,
/// sign it, and push the `.sig` sidecar. Returns the sig tag on
/// success, `None` when no private key is configured.
fn sign_published_artifact(
    workspace: &Path,
    manifest: &AkuaManifest,
    oci_ref: &str,
    manifest_digest: &str,
    creds: &CredsStore,
) -> Result<Option<String>, PublishError> {
    let Some(signing) = manifest.signing.as_ref() else {
        return Ok(None);
    };
    let Some(rel) = signing.cosign_private_key.as_deref() else {
        return Ok(None);
    };
    let key_path = workspace.join(rel);
    let private_pem =
        std::fs::read_to_string(&key_path).map_err(|source| PublishError::SigningKeyIo {
            path: key_path.clone(),
            source,
        })?;

    // docker-reference: human-readable OCI ref without the scheme,
    // matching what cosign-cli records for `cosign sign oci://...`.
    let docker_reference = oci_ref.strip_prefix("oci://").unwrap_or(oci_ref);
    let payload = akua_core::cosign::build_simple_signing_payload(docker_reference, manifest_digest);
    let signature =
        akua_core::cosign::sign_keyed(&private_pem, &payload).map_err(PublishError::Sign)?;

    let sig_tag =
        oci_pusher::push_cosign_signature(oci_ref, manifest_digest, &payload, &signature, creds)?;
    Ok(Some(sig_tag))
}

// Silence unused-import warning when only some paths use PathBuf.
#[allow(dead_code)]
const _: fn(&PathBuf) = |_| {};
