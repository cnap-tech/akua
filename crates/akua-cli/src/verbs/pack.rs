//! `akua pack` — build the same tarball `akua publish` uploads, but
//! to a local file instead of a registry. Use cases:
//!
//! - **Air-gap workflows**: pack in one environment, transfer the
//!   `.tar.gz` across a boundary, sign + push from another.
//! - **Offline sign**: pack locally, hand the tarball to a detached
//!   cosign + uploader without giving the signer network access to
//!   the workspace.
//! - **Archive / diff**: keep a reference tarball from a known-good
//!   state; diff a later pack against it bit-for-bit.
//!
//! Output is byte-deterministic given identical inputs (same contract
//! as `akua publish`), so re-packing an unchanged workspace produces
//! an unchanged tarball digest — callers can pin the layer digest
//! downstream.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{package_tar, AkuaManifest, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct PackArgs<'a> {
    pub workspace: &'a Path,

    /// Target tarball. When `None`, defaults to
    /// `<workspace>/dist/<name>-<version>.tar.gz`. The `dist/`
    /// subdir is walker-skipped, so repeated `akua pack` runs
    /// produce byte-identical tarballs.
    pub out: Option<&'a Path>,

    /// Suppress dep vendoring under `.akua/vendor/`. Shrinks the
    /// tarball but the result won't render offline — consumers must
    /// have network access to resolve the OCI/git deps.
    pub no_vendor: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackOutput {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub layer_digest: String,
    pub vendored_deps: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Pack(#[from] package_tar::PackageTarError),

    #[error("writing tarball to `{}`: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl PackError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            PackError::Manifest(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            PackError::Pack(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
            PackError::Write { source, .. } => {
                StructuredError::new(codes::E_IO, source.to_string()).with_default_docs()
            }
            PackError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            PackError::StdoutWrite(_) | PackError::Write { .. } => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &PackArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, PackError> {
    let manifest = AkuaManifest::load(args.workspace)?;

    let vendored_pairs = if args.no_vendor {
        Vec::new()
    } else {
        crate::verbs::vendor::collect_vendor_pairs(args.workspace, &manifest, "akua pack")
    };
    let vendored_deps: Vec<String> = vendored_pairs.iter().map(|(n, _)| n.clone()).collect();

    let tar_gz =
        package_tar::pack_workspace_with_vendored_deps(args.workspace, &vendored_pairs)?;

    let out_path = match args.out {
        Some(p) => p.to_path_buf(),
        None => default_out_path(args.workspace, &manifest),
    };
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| PackError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }
    std::fs::write(&out_path, &tar_gz).map_err(|source| PackError::Write {
        path: out_path.clone(),
        source,
    })?;

    let layer_digest = package_tar::layer_digest(&tar_gz);

    let output = PackOutput {
        path: out_path,
        size_bytes: tar_gz.len() as u64,
        layer_digest,
        vendored_deps,
    };
    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(PackError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn default_out_path(workspace: &Path, manifest: &AkuaManifest) -> PathBuf {
    // `dist/` is in the walker's skip-dir list, so re-packing is
    // idempotent (the previous tarball is never folded into the new
    // one). Conventional for build outputs across Rust/JS/Python.
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    workspace
        .join("dist")
        .join(format!("{name}-{version}.tar.gz"))
}

fn write_text<W: Write>(w: &mut W, out: &PackOutput) -> std::io::Result<()> {
    writeln!(w, "packed: {}", out.path.display())?;
    writeln!(w, "  layer  {}", out.layer_digest)?;
    writeln!(w, "  size   {} bytes", out.size_bytes)?;
    if !out.vendored_deps.is_empty() {
        writeln!(w, "  vendored: {}", out.vendored_deps.join(", "))?;
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

    fn minimal_workspace(tmp: &Path) {
        std::fs::write(
            tmp.join("akua.toml"),
            r#"
[package]
name    = "pack-test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"
"#,
        )
        .unwrap();
        std::fs::write(tmp.join("package.k"), b"resources = []\n").unwrap();
    }

    #[test]
    fn pack_writes_tarball_to_default_path() {
        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());

        let mut stdout = Vec::new();
        let args = PackArgs {
            workspace: tmp.path(),
            out: None,
            no_vendor: true,
        };
        let code = run(&ctx_json(), &args, &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);

        let expected = tmp.path().join("dist/pack-test-0.1.0.tar.gz");
        assert!(expected.is_file(), "default tarball path missing");

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(parsed["layer_digest"].as_str().unwrap().starts_with("sha256:"));
        assert!(parsed["size_bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn pack_writes_to_explicit_out_path() {
        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());
        let out = tmp.path().join("dist/custom.tgz");

        let args = PackArgs {
            workspace: tmp.path(),
            out: Some(&out),
            no_vendor: true,
        };
        let mut stdout = Vec::new();
        run(&ctx_json(), &args, &mut stdout).unwrap();
        assert!(out.is_file(), "explicit out path missing");
    }

    #[test]
    fn pack_is_deterministic_with_default_path() {
        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());

        // Default path lands under `dist/` which is walker-skipped,
        // so the prior tarball is never folded into the next run.
        let first_bytes;
        {
            let args = PackArgs {
                workspace: tmp.path(),
                out: None,
                no_vendor: true,
            };
            let mut stdout = Vec::new();
            run(&ctx_json(), &args, &mut stdout).unwrap();
            first_bytes =
                std::fs::read(tmp.path().join("dist/pack-test-0.1.0.tar.gz")).unwrap();
        }
        let args = PackArgs {
            workspace: tmp.path(),
            out: None,
            no_vendor: true,
        };
        let mut stdout = Vec::new();
        run(&ctx_json(), &args, &mut stdout).unwrap();
        let second_bytes =
            std::fs::read(tmp.path().join("dist/pack-test-0.1.0.tar.gz")).unwrap();
        assert_eq!(
            first_bytes, second_bytes,
            "re-packing the same workspace should be byte-identical"
        );
    }

    #[test]
    fn pack_explicit_out_is_deterministic_outside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());

        let outs = tempfile::tempdir().unwrap();
        let first = outs.path().join("a.tgz");
        let second = outs.path().join("b.tgz");
        for out in [&first, &second] {
            let args = PackArgs {
                workspace: tmp.path(),
                out: Some(out),
                no_vendor: true,
            };
            let mut stdout = Vec::new();
            run(&ctx_json(), &args, &mut stdout).unwrap();
        }
        let a = std::fs::read(&first).unwrap();
        let b = std::fs::read(&second).unwrap();
        assert_eq!(
            a, b,
            "pack output not byte-deterministic across back-to-back runs"
        );
    }

    #[test]
    fn pack_round_trips_via_unpack_to() {
        use akua_core::package_tar;

        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());
        let out = tmp.path().join("pack.tgz");
        let args = PackArgs {
            workspace: tmp.path(),
            out: Some(&out),
            no_vendor: true,
        };
        let mut stdout = Vec::new();
        run(&ctx_json(), &args, &mut stdout).unwrap();

        let bytes = std::fs::read(&out).unwrap();
        let restore = tempfile::tempdir().unwrap();
        package_tar::unpack_to(&bytes, restore.path()).unwrap();
        assert!(restore.path().join("akua.toml").is_file());
        assert!(restore.path().join("package.k").is_file());
    }

    #[test]
    fn pack_default_path_uses_dist_subdir_and_name_version() {
        let tmp = tempfile::tempdir().unwrap();
        minimal_workspace(tmp.path());
        let manifest = AkuaManifest::load(tmp.path()).unwrap();
        let p = default_out_path(tmp.path(), &manifest);
        assert!(p.ends_with("dist/pack-test-0.1.0.tar.gz"), "got {p:?}");
    }
}
