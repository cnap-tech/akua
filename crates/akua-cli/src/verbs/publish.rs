//! `akua publish` — tarball the workspace + push it to an OCI registry.
//!
//! The reciprocal of `akua add`: where add consumes a registry-hosted
//! chart, publish *produces* one. Shape:
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
use akua_core::lock_file::{AkuaLock, LockLoadError};
use akua_core::oci_auth::CredsStore;
use akua_core::{oci_pusher, package_tar, slsa, AkuaManifest, ManifestLoadError};
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

    /// `--no-attest`: skip SLSA attestation generation even when a
    /// private key is configured. Attestation auto-pairs with
    /// signing; this lets ops opt out for debugging / scratch
    /// publishes. Has no effect when `no_sign` is set (attestation
    /// always rides with signing).
    pub no_attest: bool,
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

    /// Tag the SLSA attestation sidecar was pushed under
    /// (`sha256-<hex>.att`). `None` when the publish didn't attest
    /// (no signing key configured, `--no-attest`, or `--no-sign`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation_tag: Option<String>,
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

    /// Signing + attestation failures share one variant. Both paths
    /// funnel into `E_PUBLISH_FAILED` and the inner `CosignError`
    /// carries the specific cause in its `Display`.
    #[error("cosign: {0}")]
    Crypto(akua_core::cosign::CosignError),

    #[error("reading akua.lock for attestation materials: {0}")]
    LockLoad(String),

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
            PublishError::Crypto(inner) => {
                StructuredError::new(codes::E_PUBLISH_FAILED, inner.to_string())
                    .with_default_docs()
            }
            PublishError::LockLoad(detail) => {
                StructuredError::new(codes::E_LOCK_PARSE, detail.clone()).with_default_docs()
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

    // Resolve non-path deps so their content gets vendored into the
    // tarball at `.akua/vendor/<name>/`. Resolver errors here
    // (expired creds, registry 5xx, digest drift) mean the
    // published artifact WILL NOT render offline — we emit a loud
    // stderr warning in that case rather than silently shipping an
    // un-vendored artifact.
    let vendored_pairs = crate::verbs::vendor::collect_vendor_pairs(args.workspace, &manifest);

    let tar_gz = package_tar::pack_workspace_with_vendored_deps(args.workspace, &vendored_pairs)?;

    let pushed = oci_pusher::push(args.oci_ref, &tag, &tar_gz, &creds)?;

    // Load the signing key once — both the cosign `.sig` push and
    // the SLSA `.att` push reuse it. `None` → no key configured,
    // publish remains unsigned + unattested.
    let private_pem = if args.no_sign {
        None
    } else {
        load_signing_key(args.workspace, &manifest)?
    };

    // Passphrase is only meaningful when we're actually signing.
    // Deferring the env read past `no_sign` avoids unnecessary
    // syscalls (minor) + keeps the secret out of the process
    // address space when the code path can't use it.
    let passphrase = if private_pem.is_some() {
        std::env::var("AKUA_COSIGN_PASSPHRASE")
            .ok()
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    let signature_tag = if let Some(pem) = &private_pem {
        Some(sign_published_artifact(
            args.oci_ref,
            pem,
            passphrase.as_deref(),
            &pushed.manifest_digest,
            &creds,
        )?)
    } else {
        None
    };

    // SLSA attestation auto-pairs with signing: if we just signed
    // the manifest, push an attestation signed by the same key
    // unless --no-attest. Skipping attestation when the artifact
    // *is* signed would weaken the supply-chain story unnecessarily.
    let attestation_tag = if let Some(pem) = &private_pem {
        if args.no_attest {
            None
        } else {
            Some(attest_published_artifact(
                args.workspace,
                args.oci_ref,
                &pushed.tag,
                &pushed.manifest_digest,
                pem,
                passphrase.as_deref(),
                &creds,
            )?)
        }
    } else {
        None
    };

    let output = PublishOutput {
        oci_ref: pushed.oci_ref,
        tag: pushed.tag,
        manifest_digest: pushed.manifest_digest,
        layer_digest: pushed.layer_digest,
        layer_size: pushed.layer_size,
        signature_tag,
        attestation_tag,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PublishError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, out: &PublishOutput) -> std::io::Result<()> {
    writeln!(w, "published: {}:{}", out.oci_ref, out.tag)?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(
        w,
        "  layer     {} ({} bytes)",
        out.layer_digest, out.layer_size
    )?;
    if let Some(sig_tag) = &out.signature_tag {
        writeln!(w, "  signed    {}", sig_tag)?;
    }
    if let Some(att_tag) = &out.attestation_tag {
        writeln!(w, "  attested  {}", att_tag)?;
    }
    Ok(())
}

/// Load `[signing].cosign_private_key` contents relative to the
/// workspace. `None` when no key is configured — publish stays
/// unsigned. Reads happen once per publish so a follow-up
/// attestation step doesn't re-open the file.
fn load_signing_key(
    workspace: &Path,
    manifest: &AkuaManifest,
) -> Result<Option<String>, PublishError> {
    let Some(signing) = manifest.signing.as_ref() else {
        return Ok(None);
    };
    let Some(rel) = signing.cosign_private_key.as_deref() else {
        return Ok(None);
    };
    let key_path = workspace.join(rel);
    let body = std::fs::read_to_string(&key_path).map_err(|source| PublishError::SigningKeyIo {
        path: key_path,
        source,
    })?;
    Ok(Some(body))
}

/// Build the simple-signing payload for `manifest_digest`, sign with
/// `private_pem` (`passphrase` decrypts an encrypted PKCS#8 key),
/// push the `.sig` sidecar.
fn sign_published_artifact(
    oci_ref: &str,
    private_pem: &str,
    passphrase: Option<&str>,
    manifest_digest: &str,
    creds: &CredsStore,
) -> Result<String, PublishError> {
    // docker-reference: human-readable OCI ref without the scheme,
    // matching what cosign-cli records for `cosign sign oci://...`.
    let docker_reference = oci_ref.strip_prefix("oci://").unwrap_or(oci_ref);
    let payload =
        akua_core::cosign::build_simple_signing_payload(docker_reference, manifest_digest);
    let signature = akua_core::cosign::sign_keyed(private_pem, &payload, passphrase)
        .map_err(PublishError::Crypto)?;
    Ok(oci_pusher::push_cosign_signature(
        oci_ref,
        manifest_digest,
        &payload,
        &signature,
        creds,
    )?)
}

/// Build an SLSA v1 provenance statement for the just-published
/// artifact, wrap in a DSSE envelope signed with `private_pem`,
/// push as an `.att` sidecar.
fn attest_published_artifact(
    workspace: &Path,
    oci_ref: &str,
    tag: &str,
    manifest_digest: &str,
    private_pem: &str,
    passphrase: Option<&str>,
    creds: &CredsStore,
) -> Result<String, PublishError> {
    // Lockfile is best-effort: absent → no materials in the
    // predicate (still a valid attestation, just less informative).
    // Parse failures surface so we don't silently attest with empty
    // materials when a corrupt lockfile is the real story.
    let lock = match AkuaLock::load(workspace) {
        Ok(l) => Some(l),
        Err(LockLoadError::Missing { .. }) => None,
        Err(e) => return Err(PublishError::LockLoad(e.to_string())),
    };

    let subject_name = oci_ref.strip_prefix("oci://").unwrap_or(oci_ref);
    let statement =
        slsa::build_publish_attestation(subject_name, manifest_digest, oci_ref, tag, lock.as_ref());
    let statement_bytes = slsa::statement_bytes(&statement)
        .map_err(|e| PublishError::Crypto(akua_core::cosign::CosignError::BadPayload(e)))?;
    let envelope = akua_core::cosign::sign_dsse(
        private_pem,
        "application/vnd.in-toto+json",
        &statement_bytes,
        passphrase,
    )
    .map_err(PublishError::Crypto)?;
    Ok(oci_pusher::push_attestation(
        oci_ref,
        manifest_digest,
        &envelope,
        creds,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::workspace_with;
    use akua_core::oci_pusher::OCI_MANIFEST_MEDIA_TYPE;
    use httpmock::prelude::*;

    /// Workspace with a `[signing]` block pointing at a freshly-written
    /// `cosign.key`. Single write of `akua.toml` (no double-write of
    /// the manifest) — the signing-related publish tests reuse this.
    fn workspace_with_signing(priv_pem: &str) -> tempfile::TempDir {
        let dir = workspace_with(&format!(
            "{NO_SIGN_MANIFEST}\n[signing]\ncosign_private_key = \"cosign.key\"\n"
        ));
        std::fs::write(dir.path().join("cosign.key"), priv_pem).unwrap();
        dir
    }

    /// Stand up a mock registry that accepts a full `akua publish`:
    /// the two-blob upload pair (layer + config) plus the manifest
    /// PUT. Returns `(server, oci_ref)`.
    fn mock_registry_accepting_publishes(repo: &str, tag: &str) -> (MockServer, String) {
        let server = MockServer::start();
        let upload_path = format!("/v2/{repo}/blobs/uploads/");
        let manifest_path = format!("/v2/{repo}/manifests/{tag}");
        let location = "/upload-session/x";

        server.mock(|when, then| {
            when.method(POST).path(upload_path);
            then.status(202).header("Location", location);
        });
        server.mock(|when, then| {
            when.method(PUT).path(location);
            then.status(201);
        });
        server.mock(|when, then| {
            when.method(PUT)
                .path(manifest_path)
                .header("content-type", OCI_MANIFEST_MEDIA_TYPE);
            then.status(201);
        });
        let oci_ref = format!("oci://127.0.0.1:{}/{}", server.port(), repo);
        (server, oci_ref)
    }

    /// Generate a P-256 PKCS#8 PEM keypair for signing tests. Same
    /// shape `keypair_fixture` uses inside akua-core; inlined here so
    /// the helper isn't cross-crate exposed.
    fn keypair_pem() -> (String, String) {
        use p256::ecdsa::SigningKey;
        use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
        let mut rng = rand::rngs::OsRng;
        let signing = SigningKey::random(&mut rng);
        let verifying = signing.verifying_key();
        let priv_pem = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let pub_pem = verifying.to_public_key_pem(LineEnding::LF).unwrap();
        (pub_pem, priv_pem)
    }

    const NO_SIGN_MANIFEST: &str = r#"[package]
name = "publish-test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"
"#;

    /// Default-tag path: `--tag` omitted → publish picks the
    /// `[package].version` from `akua.toml`. Asserts the mock got
    /// every leg of the upload + the JSON output is shaped right.
    #[test]
    fn publish_uses_package_version_when_tag_omitted() {
        let ws = workspace_with(NO_SIGN_MANIFEST);
        let (_server, oci_ref) = mock_registry_accepting_publishes("team/pub", "0.1.0");

        let ctx = Context::json();
        let args = PublishArgs {
            workspace: ws.path(),
            oci_ref: &oci_ref,
            tag: None,
            no_sign: true,
            no_attest: true,
        };
        let mut stdout = Vec::new();
        let exit = run(&ctx, &args, &mut stdout).expect("publish must succeed");
        assert!(matches!(exit, ExitCode::Success));

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["tag"], "0.1.0");
        assert_eq!(parsed["oci_ref"], oci_ref);
        assert!(parsed["manifest_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(parsed["layer_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        // No signing → both sidecar tags absent.
        assert!(parsed.get("signature_tag").is_none());
        assert!(parsed.get("attestation_tag").is_none());
    }

    /// `--tag` overrides `[package].version`.
    #[test]
    fn publish_honors_explicit_tag_override() {
        let ws = workspace_with(NO_SIGN_MANIFEST);
        let (_server, oci_ref) = mock_registry_accepting_publishes("team/pub", "rc-7");

        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &PublishArgs {
                workspace: ws.path(),
                oci_ref: &oci_ref,
                tag: Some("rc-7"),
                no_sign: true,
                no_attest: true,
            },
            &mut stdout,
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["tag"], "rc-7");
    }

    /// With `[signing].cosign_private_key` configured, publish pushes
    /// a `.sig` sidecar AND a `.att` sidecar (attestation auto-pairs
    /// with signing unless `--no-attest`). Both tag fields populate.
    #[test]
    fn publish_signs_and_attests_when_key_configured() {
        let (_pub_pem, priv_pem) = keypair_pem();
        let ws = workspace_with_signing(&priv_pem);

        let server = MockServer::start();
        let repo = "team/signed";
        let tag = "0.1.0";

        // Catch-all handlers: any blob upload + any manifest PUT
        // accept. We don't introspect tags here — just need the
        // pusher to walk all three legs (artifact, sig, attestation)
        // without 5xx.
        server.mock(|when, then| {
            when.method(POST).path_contains("blobs/uploads");
            then.status(202).header("Location", "/upload-session/x");
        });
        server.mock(|when, then| {
            when.method(PUT).path("/upload-session/x");
            then.status(201);
        });
        server.mock(|when, then| {
            when.method(PUT).path_contains("manifests");
            then.status(201);
        });

        let oci_ref = format!("oci://127.0.0.1:{}/{}", server.port(), repo);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &PublishArgs {
                workspace: ws.path(),
                oci_ref: &oci_ref,
                tag: Some(tag),
                no_sign: false,
                no_attest: false,
            },
            &mut stdout,
        )
        .expect("signed publish must succeed");

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let sig = parsed["signature_tag"].as_str().expect("sig tag emitted");
        let att = parsed["attestation_tag"].as_str().expect("att tag emitted");
        assert!(sig.ends_with(".sig"), "sig tag shape: {sig}");
        assert!(att.ends_with(".att"), "att tag shape: {att}");
        // Both sidecar tags share the same `sha256-<hex>` prefix
        // because they reference the same artifact digest.
        let sig_prefix = sig.trim_end_matches(".sig");
        let att_prefix = att.trim_end_matches(".att");
        assert_eq!(sig_prefix, att_prefix);
    }

    /// `--no-attest` with signing on: sig pushed, attestation skipped.
    #[test]
    fn publish_no_attest_skips_attestation_only() {
        let (_pub_pem, priv_pem) = keypair_pem();
        let ws = workspace_with_signing(&priv_pem);

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path_contains("blobs/uploads");
            then.status(202).header("Location", "/upload-session/x");
        });
        server.mock(|when, then| {
            when.method(PUT).path("/upload-session/x");
            then.status(201);
        });
        server.mock(|when, then| {
            when.method(PUT).path_contains("manifests");
            then.status(201);
        });

        let oci_ref = format!("oci://127.0.0.1:{}/team/no-att", server.port());
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &PublishArgs {
                workspace: ws.path(),
                oci_ref: &oci_ref,
                tag: Some("0.1.0"),
                no_sign: false,
                no_attest: true,
            },
            &mut stdout,
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(parsed["signature_tag"].is_string(), "sig still emitted");
        assert!(
            parsed.get("attestation_tag").is_none(),
            "att skipped under --no-attest"
        );
    }

    /// Signing key path that doesn't exist → `SigningKeyIo` with the
    /// path surfaced in the structured error so users see *which*
    /// path the resolver looked at.
    #[test]
    fn publish_surfaces_missing_signing_key_path() {
        let ws = workspace_with(NO_SIGN_MANIFEST);
        std::fs::write(
            ws.path().join("akua.toml"),
            format!("{NO_SIGN_MANIFEST}\n[signing]\ncosign_private_key = \"missing.key\"\n"),
        )
        .unwrap();

        let (_server, oci_ref) = mock_registry_accepting_publishes("team/key-missing", "0.1.0");
        let mut stdout = Vec::new();
        let err = run(
            &Context::json(),
            &PublishArgs {
                workspace: ws.path(),
                oci_ref: &oci_ref,
                tag: None,
                no_sign: false,
                no_attest: false,
            },
            &mut stdout,
        )
        .unwrap_err();
        assert!(matches!(err, PublishError::SigningKeyIo { .. }));
        let structured = err.to_structured();
        assert_eq!(structured.code, codes::E_PUBLISH_FAILED);
        assert!(
            structured
                .path
                .as_deref()
                .is_some_and(|p| p.ends_with("missing.key")),
            "structured error path field carries the resolved key path"
        );
    }
}
