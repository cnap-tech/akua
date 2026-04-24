//! `akua verify --tarball` — verify a local tarball + sidecars
//! against a cosign public key, no registry round-trip.
//!
//! Pair to `akua sign`: closes the offline loop so operators can
//! validate an air-gap-transferred tarball before pushing it, or run
//! post-transfer checks on the receiving side without trusting the
//! transfer channel.
//!
//! Three checks today:
//! 1. **sidecar_readable** — `.akuasig` parses.
//! 2. **digest_match** — locally-computed `manifest_digest` matches
//!    what the sidecar signed. Catches wrong-sidecar-for-this-tarball
//!    + sign/push version drift (config blob embeds
//!    `env!("CARGO_PKG_VERSION")`).
//! 3. **signature_verify** — `cosign::verify_keyed` against the
//!    sidecar's `signature_b64` + `simple_signing_payload`. Skipped
//!    when no public key is configured.
//!
//! Attestation verification (`.akuaatt`) lands with the `akua attest`
//! verb in a later slice.

#![cfg(feature = "cosign-verify")]

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::cosign::{self, CosignError};
use akua_core::cosign_sidecar::{self, SignSidecar};
use akua_core::{oci_pusher, AkuaManifest, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct VerifyTarballArgs<'a> {
    pub tarball: &'a Path,
    /// Sidecar path. `None` → `<tarball>.akuasig`.
    pub sig: Option<&'a Path>,
    /// Explicit public key (PEM). `None` → fall back to
    /// `akua.toml [signing].cosign_public_key` under `workspace`.
    pub public_key: Option<&'a Path>,
    /// Used only to resolve the default public key from `akua.toml`.
    pub workspace: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VerifyTarballOutput {
    /// `"ok"` when every check passes or is skipped for a benign
    /// reason; `"fail"` when any check failed.
    pub status: &'static str,
    pub tarball: PathBuf,
    pub sidecar: PathBuf,
    /// Echo of the sidecar's recorded target. Not validated against
    /// anything — there's no registry contact — but surfaced so
    /// operators can sanity-check before pushing.
    pub oci_ref: Option<String>,
    pub tag: Option<String>,
    pub checks: Vec<VerifyCheck>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerifyCheck {
    SidecarReadable {
        status: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    DigestMatch {
        status: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actual: Option<String>,
    },
    SignatureVerify {
        status: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl VerifyCheck {
    fn status(&self) -> &'static str {
        match self {
            VerifyCheck::SidecarReadable { status, .. }
            | VerifyCheck::DigestMatch { status, .. }
            | VerifyCheck::SignatureVerify { status, .. } => status,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyTarballError {
    #[error("reading tarball `{}`: {source}", path.display())]
    ReadTarball {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("reading public key `{}`: {source}", path.display())]
    ReadPublicKey {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl VerifyTarballError {
    pub fn to_structured(&self) -> StructuredError {
        // Explicit per-variant arms so a new variant added to this
        // enum forces a conscious choice rather than silently
        // inheriting E_IO via a catch-all.
        match self {
            VerifyTarballError::Manifest(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            VerifyTarballError::ReadTarball { .. }
            | VerifyTarballError::ReadPublicKey { .. }
            | VerifyTarballError::StdoutWrite(_) => {
                StructuredError::new(codes::E_IO, self.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            VerifyTarballError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &VerifyTarballArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, VerifyTarballError> {
    let output = check(args)?;
    let exit = if output.status == "ok" {
        ExitCode::Success
    } else {
        ExitCode::UserError
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(VerifyTarballError::StdoutWrite)?;
    Ok(exit)
}

fn check(args: &VerifyTarballArgs<'_>) -> Result<VerifyTarballOutput, VerifyTarballError> {
    let bytes = std::fs::read(args.tarball).map_err(|source| VerifyTarballError::ReadTarball {
        path: args.tarball.to_path_buf(),
        source,
    })?;
    let sidecar_path = resolve_sidecar_path(args);

    let mut checks = Vec::new();
    let mut oci_ref = None;
    let mut tag = None;

    // 1. Sidecar parse.
    let sidecar = match SignSidecar::read_from(&sidecar_path) {
        Ok(s) => {
            checks.push(VerifyCheck::SidecarReadable {
                status: "pass",
                detail: None,
            });
            oci_ref = Some(s.oci_ref.clone());
            tag = Some(s.tag.clone());
            Some(s)
        }
        Err(e) => {
            checks.push(VerifyCheck::SidecarReadable {
                status: "fail",
                detail: Some(e.to_string()),
            });
            None
        }
    };

    // 2. Digest match. Skipped when sidecar didn't parse.
    if let Some(s) = &sidecar {
        let expected = oci_pusher::compute_publish_digests(&bytes).manifest_digest;
        if s.manifest_digest == expected {
            checks.push(VerifyCheck::DigestMatch {
                status: "pass",
                expected: None,
                actual: None,
            });
        } else {
            checks.push(VerifyCheck::DigestMatch {
                status: "fail",
                expected: Some(expected),
                actual: Some(s.manifest_digest.clone()),
            });
        }
    } else {
        checks.push(VerifyCheck::DigestMatch {
            status: "skip",
            expected: None,
            actual: None,
        });
    }

    // 3. Signature verify. Skipped without a public key, or when
    // sidecar didn't parse.
    let public_key_pem = load_public_key(args)?;
    match (&sidecar, public_key_pem) {
        (Some(s), Some(pem)) => {
            let verify = cosign::verify_keyed(
                &pem,
                s.simple_signing_payload.as_bytes(),
                &s.signature_b64,
                &s.manifest_digest,
            );
            match verify {
                Ok(()) => checks.push(VerifyCheck::SignatureVerify {
                    status: "pass",
                    detail: None,
                }),
                Err(e) => checks.push(VerifyCheck::SignatureVerify {
                    status: "fail",
                    detail: Some(describe_cosign_err(&e)),
                }),
            }
        }
        _ => checks.push(VerifyCheck::SignatureVerify {
            status: "skip",
            detail: Some(
                "no public key configured (pass --public-key or set akua.toml [signing].cosign_public_key)"
                    .to_string(),
            ),
        }),
    }

    let any_fail = checks.iter().any(|c| c.status() == "fail");
    Ok(VerifyTarballOutput {
        status: if any_fail { "fail" } else { "ok" },
        tarball: args.tarball.to_path_buf(),
        sidecar: sidecar_path,
        oci_ref,
        tag,
        checks,
    })
}

fn describe_cosign_err(err: &CosignError) -> String {
    // Separate crypto-verify failure from parse/key failure so
    // operators know if it's "wrong signer" vs "malformed input".
    match err {
        CosignError::VerifyFailed(msg) => format!("signature does not verify: {msg}"),
        CosignError::DigestMismatch { claimed, actual } => {
            format!("payload digest `{claimed}` doesn't match manifest digest `{actual}`")
        }
        other => other.to_string(),
    }
}

fn resolve_sidecar_path(args: &VerifyTarballArgs<'_>) -> PathBuf {
    match args.sig {
        Some(p) => p.to_path_buf(),
        None => cosign_sidecar::default_sidecar_path(args.tarball),
    }
}

fn load_public_key(args: &VerifyTarballArgs<'_>) -> Result<Option<String>, VerifyTarballError> {
    if let Some(p) = args.public_key {
        return std::fs::read_to_string(p)
            .map(Some)
            .map_err(|source| VerifyTarballError::ReadPublicKey {
                path: p.to_path_buf(),
                source,
            });
    }
    // Fall back to workspace akua.toml [signing].cosign_public_key.
    // Missing manifest or missing key is a clean `None` — the caller
    // treats "no public key" as a skipped check, not a failure. We
    // only error on a manifest that's present-but-malformed.
    let manifest = match AkuaManifest::load(args.workspace) {
        Ok(m) => m,
        Err(ManifestLoadError::Missing { .. }) => return Ok(None),
        Err(e) => return Err(VerifyTarballError::Manifest(e)),
    };
    let Some(signing) = manifest.signing.as_ref() else {
        return Ok(None);
    };
    let Some(rel) = signing.cosign_public_key.as_deref() else {
        return Ok(None);
    };
    let key_path = args.workspace.join(rel);
    std::fs::read_to_string(&key_path)
        .map(Some)
        .map_err(|source| VerifyTarballError::ReadPublicKey {
            path: key_path,
            source,
        })
}

fn write_text<W: Write>(w: &mut W, out: &VerifyTarballOutput) -> std::io::Result<()> {
    writeln!(w, "verify: {}", out.tarball.display())?;
    writeln!(w, "  sidecar {}", out.sidecar.display())?;
    if let (Some(r), Some(t)) = (&out.oci_ref, &out.tag) {
        writeln!(w, "  target  {r}:{t}")?;
    }
    for check in &out.checks {
        match check {
            VerifyCheck::SidecarReadable { status, detail } => {
                writeln!(w, "  [{status}] sidecar readable")?;
                if let Some(d) = detail {
                    writeln!(w, "        {d}")?;
                }
            }
            VerifyCheck::DigestMatch { status, expected, actual } => {
                writeln!(w, "  [{status}] manifest digest match")?;
                if let (Some(e), Some(a)) = (expected, actual) {
                    writeln!(w, "        expected {e}")?;
                    writeln!(w, "        sidecar  {a}")?;
                }
            }
            VerifyCheck::SignatureVerify { status, detail } => {
                writeln!(w, "  [{status}] signature verify")?;
                if let Some(d) = detail {
                    writeln!(w, "        {d}")?;
                }
            }
        }
    }
    writeln!(w, "status: {}", out.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;
    use akua_core::package_tar;
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rand::rngs::OsRng;

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    fn gen_key_pair() -> (String, String) {
        let sk = SigningKey::random(&mut OsRng);
        let priv_pem = sk.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let pub_pem = sk
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        (priv_pem, pub_pem)
    }

    fn pack(workspace: &Path) -> Vec<u8> {
        std::fs::write(
            workspace.join("akua.toml"),
            b"[package]\nname = \"verify-test\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();
        std::fs::write(workspace.join("package.k"), b"resources = []\n").unwrap();
        package_tar::pack_workspace(workspace).unwrap()
    }

    fn sign_sidecar(tarball: &[u8], oci_ref: &str, tag: &str, priv_pem: &str) -> SignSidecar {
        let d = oci_pusher::compute_publish_digests(tarball);
        let docker_reference = oci_ref.strip_prefix("oci://").unwrap_or(oci_ref);
        let payload =
            cosign::build_simple_signing_payload(docker_reference, &d.manifest_digest);
        let sig = cosign::sign_keyed(priv_pem, &payload, None).unwrap();
        SignSidecar {
            oci_ref: oci_ref.to_string(),
            tag: tag.to_string(),
            manifest_digest: d.manifest_digest,
            simple_signing_payload: String::from_utf8(payload).unwrap(),
            signature_b64: sig,
            akua_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    #[test]
    fn happy_path_all_three_checks_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let bytes = pack(&ws);
        let tar = tmp.path().join("p.tgz");
        std::fs::write(&tar, &bytes).unwrap();

        let (priv_pem, pub_pem) = gen_key_pair();
        let pub_path = tmp.path().join("pub.pem");
        std::fs::write(&pub_path, &pub_pem).unwrap();

        let sidecar = sign_sidecar(&bytes, "oci://registry.example/x", "0.1.0", &priv_pem);
        let sig_path = tmp.path().join("p.tgz.akuasig");
        sidecar.write_to(&sig_path).unwrap();

        let mut stdout = Vec::new();
        let code = run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &tar,
                sig: None,
                public_key: Some(&pub_path),
                workspace: &ws,
            },
            &mut stdout,
        )
        .unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "ok");
        let checks = parsed["checks"].as_array().unwrap();
        for c in checks {
            assert_eq!(c["status"], "pass", "failing check: {c:?}");
        }
    }

    #[test]
    fn missing_public_key_skips_signature_check_but_digest_still_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let bytes = pack(&ws);
        let tar = tmp.path().join("p.tgz");
        std::fs::write(&tar, &bytes).unwrap();

        let (priv_pem, _pub_pem) = gen_key_pair();
        let sidecar = sign_sidecar(&bytes, "oci://registry.example/x", "0.1.0", &priv_pem);
        let sig_path = tmp.path().join("p.tgz.akuasig");
        sidecar.write_to(&sig_path).unwrap();

        let mut stdout = Vec::new();
        let code = run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &tar,
                sig: None,
                public_key: None,
                workspace: &ws,
            },
            &mut stdout,
        )
        .unwrap();
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let checks = parsed["checks"].as_array().unwrap();
        // Sidecar parse + digest match → pass; signature verify → skip.
        assert_eq!(checks[0]["status"], "pass");
        assert_eq!(checks[1]["status"], "pass");
        assert_eq!(checks[2]["status"], "skip");
        assert_eq!(parsed["status"], "ok");
    }

    #[test]
    fn mutated_tarball_fails_digest_match_and_overall_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let bytes = pack(&ws);
        let tar = tmp.path().join("p.tgz");
        std::fs::write(&tar, &bytes).unwrap();

        let (priv_pem, pub_pem) = gen_key_pair();
        let pub_path = tmp.path().join("pub.pem");
        std::fs::write(&pub_path, &pub_pem).unwrap();

        // Sign against the original bytes, then mutate the tarball
        // so the on-disk digest diverges.
        let sidecar = sign_sidecar(&bytes, "oci://registry.example/x", "0.1.0", &priv_pem);
        let sig_path = tmp.path().join("p.tgz.akuasig");
        sidecar.write_to(&sig_path).unwrap();
        std::fs::write(&tar, b"TAMPERED").unwrap();

        let mut stdout = Vec::new();
        let code = run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &tar,
                sig: None,
                public_key: Some(&pub_path),
                workspace: &ws,
            },
            &mut stdout,
        )
        .unwrap();
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "fail");
        let checks = parsed["checks"].as_array().unwrap();
        let digest_check = &checks[1];
        assert_eq!(digest_check["kind"], "digest_match");
        assert_eq!(digest_check["status"], "fail");
    }

    #[test]
    fn wrong_public_key_fails_signature_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let bytes = pack(&ws);
        let tar = tmp.path().join("p.tgz");
        std::fs::write(&tar, &bytes).unwrap();

        let (priv_pem, _pub_pem) = gen_key_pair();
        let sidecar = sign_sidecar(&bytes, "oci://registry.example/x", "0.1.0", &priv_pem);
        let sig_path = tmp.path().join("p.tgz.akuasig");
        sidecar.write_to(&sig_path).unwrap();

        // A different key pair — signature won't verify.
        let (_other_priv, other_pub) = gen_key_pair();
        let pub_path = tmp.path().join("other.pem");
        std::fs::write(&pub_path, &other_pub).unwrap();

        let mut stdout = Vec::new();
        run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &tar,
                sig: None,
                public_key: Some(&pub_path),
                workspace: &ws,
            },
            &mut stdout,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["status"], "fail");
        let checks = parsed["checks"].as_array().unwrap();
        let sig_check = &checks[2];
        assert_eq!(sig_check["kind"], "signature_verify");
        assert_eq!(sig_check["status"], "fail");
    }

    #[test]
    fn missing_sidecar_fails_the_first_check() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let bytes = pack(&ws);
        let tar = tmp.path().join("p.tgz");
        std::fs::write(&tar, &bytes).unwrap();

        let mut stdout = Vec::new();
        let code = run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &tar,
                sig: None,
                public_key: None,
                workspace: &ws,
            },
            &mut stdout,
        )
        .unwrap();
        assert_eq!(code, ExitCode::UserError);
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let checks = parsed["checks"].as_array().unwrap();
        assert_eq!(checks[0]["kind"], "sidecar_readable");
        assert_eq!(checks[0]["status"], "fail");
        // Digest + signature skipped when sidecar can't parse.
        assert_eq!(checks[1]["status"], "skip");
        assert_eq!(checks[2]["status"], "skip");
    }

    #[test]
    fn missing_tarball_file_surfaces_typed_error_not_fail_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let missing = tmp.path().join("gone.tgz");

        let err = run(
            &ctx_json(),
            &VerifyTarballArgs {
                tarball: &missing,
                sig: None,
                public_key: None,
                workspace: &ws,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, VerifyTarballError::ReadTarball { .. }));
    }
}
