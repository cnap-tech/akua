//! `akua tree` — print the manifest's declared deps + lockfile entries.
//!
//! Pure walker logic lives in `akua_core::tree`; this verb reads
//! files, delegates, renders the human-mode text on top of the
//! typed output. The SDK reaches `tree_from_sources` through the
//! WASM bindings — same logic, no binary.

use std::io::Write;
use std::path::Path;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{tree_from_sources, LockLoadError, ManifestLoadError, TreeSourceError};

use crate::contract::{emit_output, Context};

/// Re-exports so external callers importing
/// `akua_cli::verbs::tree::{TreeOutput, DepRow, …}` keep compiling.
pub use akua_core::tree::{DepRow, LockedInfo, PackageInfo, TreeOutput};

#[derive(Debug, Clone)]
pub struct TreeArgs<'a> {
    pub workspace: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error(transparent)]
    Manifest(#[from] ManifestLoadError),

    #[error(transparent)]
    Lock(#[from] LockLoadError),

    #[error("akua.toml parse: {0}")]
    ManifestParse(akua_core::mod_file::ManifestError),

    #[error("akua.lock parse: {0}")]
    LockParse(akua_core::lock_file::LockError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl TreeError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            TreeError::Manifest(e) => e.to_structured(),
            TreeError::Lock(e) => e.to_structured(),
            TreeError::ManifestParse(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            TreeError::LockParse(e) => {
                StructuredError::new(codes::E_LOCK_PARSE, e.to_string()).with_default_docs()
            }
            TreeError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            TreeError::Manifest(e) if e.is_system() => ExitCode::SystemError,
            TreeError::Lock(e) if e.is_system() => ExitCode::SystemError,
            TreeError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &TreeArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, TreeError> {
    let manifest_path = args.workspace.join("akua.toml");
    let lock_path = args.workspace.join("akua.lock");

    let manifest_source = std::fs::read_to_string(&manifest_path).map_err(|source| {
        TreeError::Manifest(ManifestLoadError::Io {
            path: manifest_path.clone(),
            source,
        })
    })?;
    let lock_source = match std::fs::read_to_string(&lock_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(TreeError::Lock(LockLoadError::Io {
                path: lock_path,
                source,
            }))
        }
    };

    let output =
        tree_from_sources(&manifest_source, lock_source.as_deref()).map_err(map_source_error)?;

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(TreeError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn map_source_error(e: TreeSourceError) -> TreeError {
    match e {
        TreeSourceError::Manifest(e) => TreeError::ManifestParse(e),
        TreeSourceError::Lock(e) => TreeError::LockParse(e),
    }
}

fn write_text<W: Write>(writer: &mut W, output: &TreeOutput) -> std::io::Result<()> {
    writeln!(
        writer,
        "{}@{} ({} deps, edition={})",
        output.package.name,
        output.package.version,
        output.dependencies.len(),
        output.package.edition,
    )?;
    if output.dependencies.is_empty() {
        writeln!(writer, "  (no dependencies)")?;
        return Ok(());
    }
    for dep in &output.dependencies {
        let version = dep
            .version
            .as_deref()
            .map(|v| format!("@{v}"))
            .unwrap_or_default();
        let lock_marker = match &dep.locked {
            Some(l) => {
                let sig_marker = if l.signature.is_some() {
                    "signed"
                } else {
                    "unsigned"
                };
                format!(" [locked {} ({})]", short_digest(&l.digest), sig_marker)
            }
            None => String::new(),
        };
        let replace_marker = dep
            .locked
            .as_ref()
            .and_then(|l| l.replaced_by.as_deref())
            .map(|p| format!(" [replace -> {p}]"))
            .unwrap_or_default();
        writeln!(
            writer,
            "  - {}{} ({} {}){}{}",
            dep.name, version, dep.source, dep.source_ref, lock_marker, replace_marker,
        )?;
    }
    Ok(())
}

/// `sha256:abc123…` → `abc12345`. Keeps text-mode tree output legible.
fn short_digest(d: &str) -> String {
    d.strip_prefix("sha256:")
        .map(|rest| rest.chars().take(8).collect::<String>())
        .unwrap_or_else(|| d.chars().take(8).collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const MANIFEST: &str = r#"
[package]
name    = "demo"
version = "0.2.0"
edition = "akua.dev/v1alpha1"

[dependencies]
cnpg = { oci = "oci://ghcr.io/cnpg/charts/cluster", version = "0.20.0" }
local = { path = "../sibling" }
"#;

    const LOCK: &str = r#"
version = 1

[[package]]
name      = "cnpg"
version   = "0.20.0"
source    = "oci://ghcr.io/cnpg/charts/cluster"
digest    = "sha256:3c5d9e7f1a2b4c6d8e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d"
signature = "cosign:sigstore:cnpg"
"#;

    fn workspace(with_lock: bool) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("akua.toml"), MANIFEST).unwrap();
        if with_lock {
            fs::write(tmp.path().join("akua.lock"), LOCK).unwrap();
        }
        tmp
    }

    #[test]
    fn json_output_has_package_info_and_deps() {
        let ws = workspace(true);
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(
            &ctx,
            &TreeArgs {
                workspace: ws.path(),
            },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["package"]["name"], "demo");
        assert_eq!(parsed["package"]["version"], "0.2.0");
        let deps = parsed["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn missing_lockfile_is_not_an_error() {
        let ws = workspace(false);
        let ctx = Context::json();
        let mut stdout = Vec::new();
        let code = run(
            &ctx,
            &TreeArgs {
                workspace: ws.path(),
            },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        // Deps present, but `locked` field absent on each.
        for dep in parsed["dependencies"].as_array().unwrap() {
            assert!(dep.get("locked").is_none() || dep["locked"].is_null());
        }
    }
}
