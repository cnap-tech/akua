//! `akua inspect` — report a Package.k's input surface without running it.
//!
//! Parse-only — uses kcl_lang's `list_options` to enumerate every
//! `option()` call-site. Agents can use the JSON shape to discover
//! what inputs a Package expects before invoking it.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{list_options_kcl, OptionInfo, PackageKError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct InspectArgs<'a> {
    pub package_path: &'a Path,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InspectOutput {
    pub path: PathBuf,
    pub options: Vec<OptionInfo>,
}

#[derive(Debug, thiserror::Error)]
pub enum InspectError {
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

impl InspectError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            InspectError::Io { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            InspectError::Kcl(e) => {
                StructuredError::new(codes::E_INSPECT_FAIL, e.to_string()).with_default_docs()
            }
            InspectError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            InspectError::Io { source, .. } if source.kind() != std::io::ErrorKind::NotFound => {
                ExitCode::SystemError
            }
            InspectError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &InspectArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, InspectError> {
    if !args.package_path.exists() {
        return Err(InspectError::Io {
            path: args.package_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found", args.package_path.display()),
            ),
        });
    }

    let options = list_options_kcl(args.package_path)?;
    let output = InspectOutput {
        path: args.package_path.to_path_buf(),
        options,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(InspectError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(writer: &mut W, output: &InspectOutput) -> std::io::Result<()> {
    writeln!(writer, "{}", output.path.display())?;
    if output.options.is_empty() {
        writeln!(writer, "  (no options)")?;
        return Ok(());
    }
    writeln!(writer, "  options:")?;
    for o in &output.options {
        let required = if o.required { " [required]" } else { "" };
        let ty = if o.r#type.is_empty() {
            String::new()
        } else {
            format!(": {}", o.r#type)
        };
        let default = o
            .default
            .as_deref()
            .map(|d| format!(" = {d}"))
            .unwrap_or_default();
        writeln!(writer, "    - {}{}{}{}", o.name, ty, default, required)?;
        if let Some(help) = &o.help {
            writeln!(writer, "        {help}")?;
        }
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

    const TYPED_INPUT: &str = r#"
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = []
outputs = [{ kind: "RawManifests", target: "./" }]
"#;

    const NO_OPTIONS: &str = r#"
resources = []
outputs = [{ kind: "RawManifests", target: "./" }]
"#;

    fn write_pkg(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("package.k");
        fs::write(&p, body).unwrap();
        (tmp, p)
    }

    #[test]
    fn lists_the_canonical_input_option() {
        let (_tmp, path) = write_pkg(TYPED_INPUT);
        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &InspectArgs { package_path: &path },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("- input"), "{text}");
    }

    #[test]
    fn json_output_shape_is_stable() {
        let (_tmp, path) = write_pkg(TYPED_INPUT);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &InspectArgs { package_path: &path },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert!(parsed["path"].as_str().unwrap().ends_with("package.k"));
        let opts = parsed["options"].as_array().unwrap();
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0]["name"], "input");
        // `type` is empty: list_options doesn't infer the type from
        // the enclosing `input: Input = option(...)` binding; it only
        // reads a type arg passed directly to `option()`. The field
        // stays on the output shape so the contract is forward-
        // compatible with richer type recovery later.
        assert_eq!(opts[0].get("type").and_then(|t| t.as_str()).unwrap_or(""), "");
    }

    #[test]
    fn package_with_no_options_reports_empty_list() {
        let (_tmp, path) = write_pkg(NO_OPTIONS);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &InspectArgs { package_path: &path },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert!(parsed["options"].as_array().unwrap().is_empty());
    }

    #[test]
    fn missing_package_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.k");
        let err = run(
            &Context::human(),
            &InspectArgs { package_path: &missing },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
    }
}
