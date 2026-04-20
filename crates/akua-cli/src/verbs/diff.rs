//! `akua diff` — structural diff between two rendered-output directories.
//!
//! Compare two directories of rendered manifests (typically the output
//! of two `akua render` runs against different inputs or different
//! Package versions). MVP: file-level diff via sha256; line-level YAML
//! diff is a follow-up.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{dir_diff, DirDiff, DirDiffError};

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct DiffArgs<'a> {
    pub before: &'a Path,
    pub after: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error(transparent)]
    DirDiff(#[from] DirDiffError),
}

impl DiffError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            DiffError::DirDiff(DirDiffError::NotDir { path }) => {
                StructuredError::new(codes::E_DIFF_NOT_DIR, self.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            DiffError::DirDiff(DirDiffError::Io { path, source }) => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            DiffError::DirDiff(DirDiffError::Io { source, .. })
                if source.kind() != std::io::ErrorKind::NotFound =>
            {
                ExitCode::SystemError
            }
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &DiffArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, DiffError> {
    let result = dir_diff(args.before, args.after)?;

    emit_output(stdout, ctx, &result, |w| write_text(w, &result))
        .map_err(|e| DiffError::DirDiff(DirDiffError::Io {
            path: PathBuf::from("<stdout>"),
            source: e,
        }))?;

    // diff(1) convention: exit 1 when differences detected, 0 when
    // clean. CI gates can branch on the exit code without parsing.
    Ok(if result.is_clean() {
        ExitCode::Success
    } else {
        ExitCode::UserError
    })
}

fn write_text<W: Write>(writer: &mut W, d: &DirDiff) -> std::io::Result<()> {
    if d.is_clean() {
        writeln!(writer, "no differences")?;
        if !d.skipped.is_empty() {
            writeln!(writer, "  ({} non-file entries skipped)", d.skipped.len())?;
        }
        return Ok(());
    }

    for path in &d.added {
        writeln!(writer, "+ {}", path.display())?;
    }
    for path in &d.removed {
        writeln!(writer, "- {}", path.display())?;
    }
    for change in &d.changed {
        writeln!(
            writer,
            "~ {}  ({} → {})",
            change.path.display(),
            short_hash(&change.before),
            short_hash(&change.after),
        )?;
    }
    if !d.skipped.is_empty() {
        writeln!(writer, "  ({} non-file entries skipped)", d.skipped.len())?;
    }
    Ok(())
}

/// `sha256:abc123…` → `abc123…[:12]`. Keeps the text-mode output
/// readable; full hashes still appear in the JSON form.
fn short_hash(full: &str) -> String {
    full.strip_prefix("sha256:")
        .map(|rest| rest.chars().take(12).collect::<String>())
        .unwrap_or_else(|| full.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn identical_dirs_exit_success() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "x.yaml", "x");
        write(after.path(), "x.yaml", "x");

        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &DiffArgs {
                before: before.path(),
                after: after.path(),
            },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        assert!(String::from_utf8(stdout).unwrap().contains("no differences"));
    }

    #[test]
    fn changed_dirs_exit_user_error() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "x.yaml", "before");
        write(after.path(), "x.yaml", "after");

        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &DiffArgs {
                before: before.path(),
                after: after.path(),
            },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::UserError);
        assert!(String::from_utf8(stdout).unwrap().contains("~ x.yaml"));
    }

    #[test]
    fn json_output_carries_added_removed_changed() {
        let before = TempDir::new().unwrap();
        let after = TempDir::new().unwrap();
        write(before.path(), "stay.yaml", "x");
        write(before.path(), "gone.yaml", "y");
        write(after.path(), "stay.yaml", "x");
        write(after.path(), "new.yaml", "z");

        let ctx = Context::resolve(
            &crate::contract::args::UniversalArgs {
                json: true,
                ..Default::default()
            },
            akua_core::cli_contract::AgentContext::none(),
        );
        let mut stdout = Vec::new();
        run(
            &ctx,
            &DiffArgs {
                before: before.path(),
                after: after.path(),
            },
            &mut stdout,
        )
        .expect("run");

        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["added"][0], "new.yaml");
        assert_eq!(parsed["removed"][0], "gone.yaml");
        assert_eq!(parsed["changed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn missing_before_dir_surfaces_typed_error() {
        let after = TempDir::new().unwrap();
        let err = run(
            &Context::human(),
            &DiffArgs {
                before: Path::new("/no/such/dir/anywhere/please"),
                after: after.path(),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
    }

    #[test]
    fn file_argument_surfaces_not_dir_error() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file");
        fs::write(&file, "x").unwrap();
        let err = run(
            &Context::human(),
            &DiffArgs {
                before: &file,
                after: tmp.path(),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_DIFF_NOT_DIR);
    }
}
