//! `akua sign` — produce a cosign signature for a packed tarball
//! without touching a registry. Writes an `.akuasig` sidecar next to
//! the tarball for a later `akua push --sig` to upload.
//!
//! Flow:
//! 1. Read tarball bytes.
//! 2. Compute [`oci_pusher::compute_publish_digests`] — pure function,
//!    matches what the registry would advertise post-push.
//! 3. Load the cosign private key (from `--key` or
//!    `akua.toml [signing].cosign_private_key`). Passphrase via
//!    `$AKUA_COSIGN_PASSPHRASE` (never on argv).
//! 4. Build + sign a simple-signing payload bound to
//!    `(oci_ref, manifest_digest)`.
//! 5. Write `<tarball>.akuasig` alongside.
//!
//! The signature is committed to a specific `oci://<registry>/<repo>`
//! + manifest digest. The push host MUST use the same akua binary
//! version as the sign host (the config blob embeds the version →
//! divergent versions yield divergent manifest digests → the
//! signature no longer verifies).

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::cosign_sidecar::SignSidecar;
use akua_core::{cosign, cosign_sidecar, oci_pusher, AkuaManifest, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct SignArgs<'a> {
    pub tarball: &'a Path,
    pub oci_ref: &'a str,
    pub tag: &'a str,
    /// Path to a PEM-encoded PKCS#8 P-256 private key. When `None`,
    /// the key is loaded from `akua.toml [signing].cosign_private_key`
    /// via `--workspace`.
    pub key: Option<&'a Path>,
    pub workspace: &'a Path,
    /// Where to write the sidecar. When `None`, defaults to
    /// `<tarball>.akuasig`.
    pub out: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SignOutput {
    pub tarball: PathBuf,
    pub sidecar: PathBuf,
    pub oci_ref: String,
    pub tag: String,
    pub manifest_digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SignError {
    /// Failed to read an input file. `what` distinguishes the two
    /// callers ("tarball" / "signing key") for the human-readable
    /// message; both share the E_IO + UserError classification.
    #[error("reading {what} `{}`: {source}", path.display())]
    ReadInput {
        what: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("tarball `{}` is empty — nothing to sign", path.display())]
    EmptyTarball { path: PathBuf },

    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(
        "no signing key — pass `--key <path>` or set `[signing].cosign_private_key` in akua.toml"
    )]
    NoKey,

    #[error(transparent)]
    Crypto(#[from] cosign::CosignError),

    #[error(transparent)]
    Sidecar(#[from] cosign_sidecar::SidecarError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl SignError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            SignError::Manifest(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            SignError::Crypto(e) => {
                StructuredError::new(codes::E_COSIGN_VERIFY, e.to_string()).with_default_docs()
            }
            // Missing signing key is a manifest/config problem, not
            // an I/O problem — surface it as such so `jq '.code ==
            // "E_MANIFEST_PARSE"'` catches the authoring bug.
            SignError::NoKey => {
                StructuredError::new(codes::E_MANIFEST_PARSE, self.to_string())
                    .with_default_docs()
            }
            _ => StructuredError::new(codes::E_IO, self.to_string()).with_default_docs(),
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            SignError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &SignArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, SignError> {
    let bytes = std::fs::read(args.tarball).map_err(|source| SignError::ReadInput {
        what: "tarball",
        path: args.tarball.to_path_buf(),
        source,
    })?;
    if bytes.is_empty() {
        return Err(SignError::EmptyTarball {
            path: args.tarball.to_path_buf(),
        });
    }

    let digests = oci_pusher::compute_publish_digests(&bytes);

    let key_pem = load_key(args)?;
    let passphrase = std::env::var("AKUA_COSIGN_PASSPHRASE")
        .ok()
        .filter(|s| !s.is_empty());

    let docker_reference = args.oci_ref.strip_prefix("oci://").unwrap_or(args.oci_ref);
    let payload = cosign::build_simple_signing_payload(docker_reference, &digests.manifest_digest);
    let signature_b64 = cosign::sign_keyed(&key_pem, &payload, passphrase.as_deref())?;

    let sidecar = SignSidecar {
        oci_ref: args.oci_ref.to_string(),
        tag: args.tag.to_string(),
        manifest_digest: digests.manifest_digest.clone(),
        // build_simple_signing_payload is serde_json::to_vec output,
        // which is UTF-8 by construction. Unreachable.
        simple_signing_payload: String::from_utf8(payload)
            .expect("simple-signing payload must be UTF-8 (serde_json invariant)"),
        signature_b64,
        akua_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let out_path = default_sidecar_path(args);
    sidecar.write_to(&out_path)?;

    let output = SignOutput {
        tarball: args.tarball.to_path_buf(),
        sidecar: out_path,
        oci_ref: args.oci_ref.to_string(),
        tag: args.tag.to_string(),
        manifest_digest: digests.manifest_digest,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(SignError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn default_sidecar_path(args: &SignArgs<'_>) -> PathBuf {
    match args.out {
        Some(p) => p.to_path_buf(),
        None => {
            let mut s = args.tarball.as_os_str().to_os_string();
            s.push(".akuasig");
            PathBuf::from(s)
        }
    }
}

fn load_key(args: &SignArgs<'_>) -> Result<String, SignError> {
    if let Some(p) = args.key {
        return std::fs::read_to_string(p).map_err(|source| SignError::ReadInput {
            what: "signing key",
            path: p.to_path_buf(),
            source,
        });
    }
    let manifest = AkuaManifest::load(args.workspace)?;
    let signing = manifest.signing.as_ref().ok_or(SignError::NoKey)?;
    let rel = signing.cosign_private_key.as_deref().ok_or(SignError::NoKey)?;
    let key_path = args.workspace.join(rel);
    std::fs::read_to_string(&key_path).map_err(|source| SignError::ReadInput {
        what: "signing key",
        path: key_path,
        source,
    })
}

fn write_text<W: Write>(w: &mut W, out: &SignOutput) -> std::io::Result<()> {
    writeln!(w, "signed: {}", out.tarball.display())?;
    writeln!(w, "  ref       {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(w, "  sidecar   {}", out.sidecar.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
    use rand::rngs::OsRng;

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    fn gen_key_pem() -> String {
        let sk = SigningKey::random(&mut OsRng);
        sk.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
    }

    fn pack_minimal_tarball(workspace: &Path, out: &Path) {
        std::fs::write(
            workspace.join("akua.toml"),
            b"[package]\nname = \"sign-test\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();
        std::fs::write(workspace.join("package.k"), b"resources = []\n").unwrap();
        let bytes = akua_core::package_tar::pack_workspace(workspace).unwrap();
        std::fs::write(out, bytes).unwrap();
    }

    #[test]
    fn sign_writes_sidecar_verifiable_via_public_key() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        pack_minimal_tarball(&workspace, &tar_path);

        // Key on disk + referenced by --key
        let key_pem = gen_key_pem();
        let key_path = tmp.path().join("priv.pem");
        std::fs::write(&key_path, &key_pem).unwrap();

        let mut stdout = Vec::new();
        let args = SignArgs {
            tarball: &tar_path,
            oci_ref: "oci://ghcr.io/acme/sign-test",
            tag: "0.1.0",
            key: Some(&key_path),
            workspace: &workspace,
            out: None,
        };
        let code = run(&ctx_json(), &args, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);

        // Sidecar exists next to the tarball with the default extension.
        let sidecar_path = tmp.path().join("p.tgz.akuasig");
        assert!(sidecar_path.is_file(), "sidecar missing");

        let sidecar = SignSidecar::read_from(&sidecar_path).unwrap();
        assert_eq!(sidecar.oci_ref, "oci://ghcr.io/acme/sign-test");
        assert_eq!(sidecar.tag, "0.1.0");
        assert!(sidecar.manifest_digest.starts_with("sha256:"));

        // Extract the public key from the same PKCS#8 PEM to verify.
        let sk = p256::ecdsa::SigningKey::from_pkcs8_pem(&key_pem).unwrap();
        let vk = sk.verifying_key();
        let public_pem = p256::pkcs8::EncodePublicKey::to_public_key_pem(vk, LineEnding::LF)
            .unwrap();

        akua_core::cosign::verify_keyed(
            &public_pem,
            sidecar.simple_signing_payload.as_bytes(),
            &sidecar.signature_b64,
            &sidecar.manifest_digest,
        )
        .expect("signature should verify");
    }

    #[test]
    fn sign_with_custom_out_path_writes_there() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        pack_minimal_tarball(&workspace, &tar_path);
        let key_path = tmp.path().join("priv.pem");
        std::fs::write(&key_path, gen_key_pem()).unwrap();
        let custom = tmp.path().join("custom.akuasig");

        let args = SignArgs {
            tarball: &tar_path,
            oci_ref: "oci://ghcr.io/x/y",
            tag: "0.1.0",
            key: Some(&key_path),
            workspace: &workspace,
            out: Some(&custom),
        };
        run(&ctx_json(), &args, &mut Vec::new()).unwrap();
        assert!(custom.is_file(), "custom sidecar path missing");
    }

    #[test]
    fn missing_tarball_surfaces_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let key_path = tmp.path().join("priv.pem");
        std::fs::write(&key_path, gen_key_pem()).unwrap();
        let err = run(
            &ctx_json(),
            &SignArgs {
                tarball: &tmp.path().join("nope.tgz"),
                oci_ref: "oci://x/y",
                tag: "0.1.0",
                key: Some(&key_path),
                workspace: &workspace,
                out: None,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, SignError::ReadInput { what: "tarball", .. }));
    }

    #[test]
    fn no_key_path_and_no_manifest_signing_surfaces_no_key() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        pack_minimal_tarball(&workspace, &tar_path);

        let err = run(
            &ctx_json(),
            &SignArgs {
                tarball: &tar_path,
                oci_ref: "oci://x/y",
                tag: "0.1.0",
                key: None,
                workspace: &workspace,
                out: None,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, SignError::NoKey), "got {err:?}");
    }

    #[test]
    fn manifest_digest_matches_what_push_would_compute() {
        // Critical invariant: the sidecar's manifest_digest equals
        // oci_pusher::compute_publish_digests(tarball_bytes). If the
        // push host computes a different digest, the signature is
        // invalid. Guard against drift.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        pack_minimal_tarball(&workspace, &tar_path);
        let key_path = tmp.path().join("priv.pem");
        std::fs::write(&key_path, gen_key_pem()).unwrap();

        run(
            &ctx_json(),
            &SignArgs {
                tarball: &tar_path,
                oci_ref: "oci://x/y",
                tag: "0.1.0",
                key: Some(&key_path),
                workspace: &workspace,
                out: None,
            },
            &mut Vec::new(),
        )
        .unwrap();

        let sidecar = SignSidecar::read_from(&tmp.path().join("p.tgz.akuasig")).unwrap();
        let expected =
            oci_pusher::compute_publish_digests(&std::fs::read(&tar_path).unwrap())
                .manifest_digest;
        assert_eq!(sidecar.manifest_digest, expected);
    }
}
