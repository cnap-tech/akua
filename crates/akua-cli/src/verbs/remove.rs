//! `akua remove` — drop a dependency from `akua.toml`.
//!
//! Mirror of [`crate::verbs::add`]. Pure manifest edit; no lockfile
//! mutation, no cache eviction.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::mod_file::ManifestError;
use akua_core::{AkuaManifest, ManifestLoadError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct RemoveArgs<'a> {
    pub workspace: &'a Path,
    pub name: &'a str,

    /// Don't error when the named dep is already absent.
    pub ignore_missing: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoveOutput {
    pub name: String,

    /// `false` only when `--ignore-missing` was set and the dep wasn't
    /// in the manifest to begin with.
    pub removed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error(transparent)]
    Load(#[from] ManifestLoadError),

    #[error(transparent)]
    Serialize(#[from] ManifestError),

    #[error("dep `{name}` not present in akua.toml")]
    NotFound { name: String },

    #[error("i/o error writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl RemoveError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            RemoveError::Load(e) => e.to_structured(),
            RemoveError::Serialize(e) => {
                StructuredError::new(codes::E_MANIFEST_PARSE, e.to_string()).with_default_docs()
            }
            RemoveError::NotFound { .. } => {
                StructuredError::new(codes::E_REMOVE_NOT_FOUND, self.to_string())
                    .with_suggestion("pass --ignore-missing to make this a no-op")
                    .with_default_docs()
            }
            RemoveError::Io { path, source } => {
                StructuredError::new(codes::E_IO, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            RemoveError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            RemoveError::Load(e) if e.is_system() => ExitCode::SystemError,
            RemoveError::Io { .. } | RemoveError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &RemoveArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, RemoveError> {
    let mut manifest = AkuaManifest::load(args.workspace)?;
    let removed = manifest.dependencies.remove(args.name).is_some();

    if !removed && !args.ignore_missing {
        return Err(RemoveError::NotFound {
            name: args.name.to_string(),
        });
    }

    if removed {
        let serialized = manifest.to_toml()?;
        let path = args.workspace.join("akua.toml");
        std::fs::write(&path, serialized).map_err(|e| RemoveError::Io {
            path,
            source: e,
        })?;
    }

    let output = RemoveOutput {
        name: args.name.to_string(),
        removed,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(RemoveError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(writer: &mut W, output: &RemoveOutput) -> std::io::Result<()> {
    if output.removed {
        writeln!(writer, "removed {}", output.name)
    } else {
        writeln!(writer, "no-op (dep `{}` was already absent)", output.name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const MANIFEST_WITH_DEP: &str = r#"
[package]
name    = "test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
cnpg = { oci = "oci://ghcr.io/cnpg/charts/cluster", version = "0.20.0" }
webapp = { oci = "oci://ghcr.io/acme/webapp", version = "1.0.0" }
"#;

    fn workspace(body: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("akua.toml"), body).unwrap();
        tmp
    }

    fn args<'a>(ws: &'a Path, name: &'a str) -> RemoveArgs<'a> {
        RemoveArgs {
            workspace: ws,
            name,
            ignore_missing: false,
        }
    }

    #[test]
    fn removes_existing_dep_and_writes_back() {
        let ws = workspace(MANIFEST_WITH_DEP);
        run(&Context::human(), &args(ws.path(), "cnpg"), &mut Vec::new()).expect("run");

        let after = AkuaManifest::load(ws.path()).expect("load");
        assert!(!after.dependencies.contains_key("cnpg"));
        assert!(after.dependencies.contains_key("webapp"));
    }

    #[test]
    fn missing_dep_errors_by_default() {
        let ws = workspace(MANIFEST_WITH_DEP);
        let err = run(&Context::human(), &args(ws.path(), "nope"), &mut Vec::new())
            .unwrap_err();
        assert!(matches!(err, RemoveError::NotFound { .. }));
        assert_eq!(err.to_structured().code, codes::E_REMOVE_NOT_FOUND);
    }

    #[test]
    fn missing_dep_is_noop_with_ignore_missing() {
        let ws = workspace(MANIFEST_WITH_DEP);
        let a = RemoveArgs {
            ignore_missing: true,
            ..args(ws.path(), "nope")
        };
        let ctx = Context::json();
        let mut stdout = Vec::new();
        let code = run(&ctx, &a, &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["removed"], false);

        // Manifest untouched.
        let after = AkuaManifest::load(ws.path()).unwrap();
        assert_eq!(after.dependencies.len(), 2);
    }

    #[test]
    fn missing_manifest_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let err = run(&Context::human(), &args(tmp.path(), "x"), &mut Vec::new())
            .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_MANIFEST_MISSING);
    }
}
