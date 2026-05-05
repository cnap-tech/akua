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
    writeln!(
        w,
        "pulled: {}:{} → {}",
        out.oci_ref,
        out.tag,
        out.out.display()
    )?;
    writeln!(w, "  manifest  {}", out.manifest_digest)?;
    writeln!(w, "  layer     {}", out.layer_digest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use akua_core::oci_pusher::{
        compute_publish_digests, AKUA_PACKAGE_LAYER_MEDIA_TYPE, OCI_MANIFEST_MEDIA_TYPE,
    };
    use akua_core::package_tar;
    use httpmock::prelude::*;
    use std::fs;

    /// Pack a tiny workspace tree (akua.toml + package.k) into the
    /// `.tar.gz` shape `oci_pusher::push` would have uploaded — that's
    /// what `pull` expects to find at the layer blob endpoint.
    fn fake_published_tarball() -> Vec<u8> {
        let src = tempfile::tempdir().unwrap();
        fs::write(
            src.path().join("akua.toml"),
            "[package]\nname=\"pulled\"\nversion=\"0.1.0\"\nedition=\"akua.dev/v1alpha1\"\n",
        )
        .unwrap();
        fs::write(src.path().join("package.k"), "resources = []\n").unwrap();
        package_tar::pack_workspace(src.path()).unwrap()
    }

    /// Spin up a mock registry that serves the manifest + blob the
    /// publisher would have written for `tarball`. Returns the
    /// `oci://127.0.0.1:<port>/<repo>` ref + the tag.
    fn mock_registry_serving(tarball: &[u8], repo: &str, tag: &str) -> (MockServer, String) {
        let server = MockServer::start();
        let d = compute_publish_digests(tarball);
        let manifest_bytes = d.manifest_bytes.clone();
        let layer_digest = d.layer_digest.clone();
        let layer_bytes = tarball.to_vec();

        server.mock(|when, then| {
            when.method(GET).path(format!("/v2/{repo}/manifests/{tag}"));
            then.status(200)
                .header("content-type", OCI_MANIFEST_MEDIA_TYPE)
                .body(manifest_bytes);
        });
        server.mock(|when, then| {
            when.method(GET)
                .path(format!("/v2/{repo}/blobs/{layer_digest}"));
            then.status(200)
                .header("content-type", AKUA_PACKAGE_LAYER_MEDIA_TYPE)
                .body(layer_bytes);
        });

        let oci_ref = format!("oci://127.0.0.1:{}/{}", server.port(), repo);
        (server, oci_ref)
    }

    /// Happy path: mock registry serves a published tarball, `pull`
    /// fetches it, unpacks into `out`, and emits a `PullOutput` with
    /// the resolved digests.
    #[test]
    fn pull_fetches_and_unpacks_into_out() {
        let tarball = fake_published_tarball();
        let (_server, oci_ref) = mock_registry_serving(&tarball, "team/pkg", "0.1.0");
        let out = tempfile::tempdir().unwrap();

        let ctx = Context::json();
        let args = PullArgs {
            oci_ref: &oci_ref,
            tag: "0.1.0",
            out: out.path(),
        };
        let mut stdout = Vec::new();
        let exit = run(&ctx, &args, &mut stdout).expect("pull must succeed");
        assert!(matches!(exit, ExitCode::Success));

        // Unpacked workspace files landed in `out`.
        assert!(out.path().join("akua.toml").is_file());
        assert!(out.path().join("package.k").is_file());

        // JSON output carries the digests the registry advertised.
        let parsed: serde_json::Value =
            serde_json::from_slice(&stdout).expect("stdout must be JSON in Context::json");
        let d = compute_publish_digests(&tarball);
        assert_eq!(parsed["manifest_digest"], d.manifest_digest);
        assert_eq!(parsed["layer_digest"], d.layer_digest);
        assert_eq!(parsed["tag"], "0.1.0");
        assert_eq!(parsed["oci_ref"], oci_ref);
    }

    /// Registry 404 on manifest → typed `PullError::Pull` carrying a
    /// transport status, exit code maps to UserError.
    #[test]
    fn pull_surfaces_registry_404_as_user_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path_contains("manifests");
            then.status(404).body("not found");
        });
        let oci_ref = format!("oci://127.0.0.1:{}/missing/pkg", server.port());
        let out = tempfile::tempdir().unwrap();

        let ctx = Context::json();
        let args = PullArgs {
            oci_ref: &oci_ref,
            tag: "0.1.0",
            out: out.path(),
        };
        let mut stdout = Vec::new();
        let err = run(&ctx, &args, &mut stdout).unwrap_err();
        assert!(matches!(err, PullError::Pull(_)));
        assert!(matches!(err.exit_code(), ExitCode::UserError));

        // Structured-error mapping uses E_PULL_FAILED.
        let structured = err.to_structured();
        assert_eq!(structured.code, codes::E_PULL_FAILED);
    }
}
