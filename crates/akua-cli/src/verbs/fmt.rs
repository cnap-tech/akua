//! `akua fmt` — format a Package.k via KCL's formatter.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua fmt` section.
//!
//! KCL-only. Rego formatting not yet implemented.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{format_kcl, PackageKError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct FmtArgs<'a> {
    pub package_path: &'a Path,

    /// `--check`: exit 1 if the file would change; do not write.
    pub check: bool,

    /// `--stdout`: print the formatted source to stdout; do not write.
    pub stdout_mode: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FmtOutput {
    pub files: Vec<FmtFile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FmtFile {
    pub path: PathBuf,

    /// True when the file's contents differ from the formatter's
    /// output. Under `--check`, this flips the exit code to
    /// [`ExitCode::UserError`]; without `--check`, the file is
    /// rewritten in place.
    pub changed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FmtError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Kcl(#[from] PackageKError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl FmtError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            FmtError::Io { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            FmtError::Kcl(PackageKError::KclEval(msg)) => {
                StructuredError::new(codes::E_FMT_KCL, msg.clone()).with_default_docs()
            }
            FmtError::Kcl(other) => {
                StructuredError::new(codes::E_FMT_KCL, other.to_string()).with_default_docs()
            }
            FmtError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            FmtError::Io { source, .. } if source.kind() != std::io::ErrorKind::NotFound => {
                ExitCode::SystemError
            }
            FmtError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &FmtArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, FmtError> {
    let original = std::fs::read_to_string(args.package_path).map_err(|e| FmtError::Io {
        path: args.package_path.to_path_buf(),
        source: e,
    })?;

    let formatted = format_kcl(&original)?;
    let changed = formatted != original;

    if args.stdout_mode {
        stdout
            .write_all(formatted.as_bytes())
            .map_err(FmtError::StdoutWrite)?;
        return Ok(ExitCode::Success);
    }

    if changed && !args.check {
        std::fs::write(args.package_path, &formatted).map_err(|e| FmtError::Io {
            path: args.package_path.to_path_buf(),
            source: e,
        })?;
    }

    let output = FmtOutput {
        files: vec![FmtFile {
            path: args.package_path.to_path_buf(),
            changed,
        }],
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output, args.check))
        .map_err(FmtError::StdoutWrite)?;

    // Under --check, a change detected means the file is not
    // well-formatted — fail so CI gates on it.
    let code = if args.check && changed {
        ExitCode::UserError
    } else {
        ExitCode::Success
    };
    Ok(code)
}

fn write_text<W: Write>(writer: &mut W, output: &FmtOutput, check: bool) -> std::io::Result<()> {
    for f in &output.files {
        let verb = match (check, f.changed) {
            (true, true) => "would reformat",
            (true, false) => "ok",
            (false, true) => "formatted",
            (false, false) => "unchanged",
        };
        writeln!(writer, "{verb}: {}", f.path.display())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Deliberately non-canonical whitespace so the formatter has
    /// something to change.
    const UNFORMATTED: &str = "schema Input:\n  x:int=1\n\ninput:Input=option(\"input\") or Input{}\nresources = []\noutputs=[{kind:\"RawManifests\",target:\"./\"}]\n";

    fn write_package(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("package.k");
        fs::write(&path, body).unwrap();
        (tmp, path)
    }

    fn args(path: &Path) -> FmtArgs<'_> {
        FmtArgs {
            package_path: path,
            check: false,
            stdout_mode: false,
        }
    }

    #[test]
    fn formats_and_writes_back_by_default() {
        let (_tmp, path) = write_package(UNFORMATTED);
        let code = run(&Context::human(), &args(&path), &mut Vec::new()).expect("run");
        assert_eq!(code, ExitCode::Success);
        let after = fs::read_to_string(&path).unwrap();
        // Idempotent: formatting the already-formatted result changes nothing.
        assert_eq!(format_kcl(&after).unwrap(), after);
    }

    #[test]
    fn check_mode_does_not_write() {
        let (_tmp, path) = write_package(UNFORMATTED);
        let a = FmtArgs {
            check: true,
            ..args(&path)
        };
        let code = run(&Context::human(), &a, &mut Vec::new()).expect("run");
        assert_eq!(code, ExitCode::UserError, "expected change-detected exit");
        assert_eq!(fs::read_to_string(&path).unwrap(), UNFORMATTED);
    }

    #[test]
    fn check_mode_succeeds_on_already_formatted_source() {
        // First format to get a canonical version, write that, then --check should pass.
        let canonical = format_kcl(UNFORMATTED).unwrap();
        let (_tmp, path) = write_package(&canonical);
        let a = FmtArgs {
            check: true,
            ..args(&path)
        };
        let code = run(&Context::human(), &a, &mut Vec::new()).expect("run");
        assert_eq!(code, ExitCode::Success);
    }

    #[test]
    fn stdout_mode_prints_formatted_and_leaves_file_untouched() {
        let (_tmp, path) = write_package(UNFORMATTED);
        let a = FmtArgs {
            stdout_mode: true,
            ..args(&path)
        };
        let mut stdout = Vec::new();
        run(&Context::human(), &a, &mut stdout).expect("run");
        assert_eq!(fs::read_to_string(&path).unwrap(), UNFORMATTED);
        // The stdout payload is canonical formatted source.
        let printed = String::from_utf8(stdout).unwrap();
        assert_eq!(printed, format_kcl(UNFORMATTED).unwrap());
    }

    #[test]
    fn unchanged_file_passes_without_rewrite() {
        let canonical = format_kcl(UNFORMATTED).unwrap();
        let (_tmp, path) = write_package(&canonical);
        let code = run(&Context::human(), &args(&path), &mut Vec::new()).expect("run");
        assert_eq!(code, ExitCode::Success);
        assert_eq!(fs::read_to_string(&path).unwrap(), canonical);
    }

    #[test]
    fn json_output_carries_changed_flag() {
        let (_tmp, path) = write_package(UNFORMATTED);
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &args(&path), &mut stdout).expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["files"][0]["changed"], true);
    }

    #[test]
    fn missing_package_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.k");
        let err = run(&Context::human(), &args(&missing), &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }
}
