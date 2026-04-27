//! `akua export` — emit the Package's `Input` schema in a standard
//! interchange format (JSON Schema 2020-12 or OpenAPI 3.1).
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua export` section.
//!
//! Backed by `akua_core::export::export_input_schema` /
//! `export_input_openapi`. The verb reads the Package source from
//! disk, dispatches on `--format`, and emits the resulting JSON to
//! stdout (or the file passed via `--out`).

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::export::{export_input_openapi, export_input_schema, ExportError};
use serde::Serialize;
use serde_json::Value;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum ExportFormat {
    /// Raw JSON Schema 2020-12 for `Input`.
    JsonSchema,
    /// OpenAPI 3.1 doc with `Input` under `components.schemas`.
    Openapi,
}

#[derive(Debug, Clone)]
pub struct ExportArgs<'a> {
    pub package_path: &'a Path,
    pub format: ExportFormat,
    /// Optional output file. When absent, JSON is written to stdout.
    pub out: Option<&'a Path>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportVerbError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Export(#[from] ExportError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl ExportVerbError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            ExportVerbError::Read { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            ExportVerbError::Write { path, source } => {
                StructuredError::new(codes::E_IO, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            ExportVerbError::Export(e) => {
                StructuredError::new(codes::E_RENDER_KCL, e.to_string()).with_default_docs()
            }
            ExportVerbError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            ExportVerbError::Read { source, .. }
                if source.kind() != std::io::ErrorKind::NotFound =>
            {
                ExitCode::SystemError
            }
            ExportVerbError::Write { .. } | ExportVerbError::StdoutWrite(_) => {
                ExitCode::SystemError
            }
            _ => ExitCode::UserError,
        }
    }
}

/// Wrapper used when emitting through `--json` so the structured-output
/// envelope carries `format` + `schema` keys instead of dumping raw
/// JSON Schema (which would shadow the universal CLI envelope).
#[derive(Debug, Serialize)]
struct ExportEnvelope<'a> {
    format: &'static str,
    schema: &'a Value,
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &ExportArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, ExportVerbError> {
    let source = std::fs::read_to_string(args.package_path).map_err(|e| ExportVerbError::Read {
        path: args.package_path.to_path_buf(),
        source: e,
    })?;
    let filename = args
        .package_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("package.k");

    let schema = match args.format {
        ExportFormat::JsonSchema => export_input_schema(filename, &source)?,
        ExportFormat::Openapi => export_input_openapi(filename, &source)?,
    };

    let pretty = serde_json::to_string_pretty(&schema)
        .map_err(|e| ExportVerbError::Export(ExportError::Serialize(e)))?;

    let written_bytes = if let Some(out_path) = args.out {
        let body = format!("{pretty}\n");
        let len = body.len();
        std::fs::write(out_path, body).map_err(|e| ExportVerbError::Write {
            path: out_path.to_path_buf(),
            source: e,
        })?;
        Some(len)
    } else {
        None
    };

    let format_str = match args.format {
        ExportFormat::JsonSchema => "json-schema",
        ExportFormat::Openapi => "openapi",
    };
    let envelope = ExportEnvelope {
        format: format_str,
        schema: &schema,
    };

    // JSON mode wraps in `{format, schema}` so machine consumers can
    // branch on `format`. Human mode prints raw pretty JSON — operators
    // pipe to `jq` or redirect to a file, no envelope wanted.
    let is_json = matches!(ctx.output, crate::contract::OutputMode::Json);
    if let (Some(bytes), false) = (written_bytes, is_json) {
        writeln!(
            stdout,
            "wrote {} ({bytes} bytes)",
            args.out.unwrap().display()
        )
        .map_err(ExportVerbError::StdoutWrite)?;
    } else {
        emit_output(stdout, ctx, &envelope, |w| writeln!(w, "{pretty}"))
            .map_err(ExportVerbError::StdoutWrite)?;
    }

    Ok(ExitCode::Success)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const PKG: &str = r#"
schema Input:
    """Public inputs."""

    @ui(order=10, group="Identity")
    name: str = "hello"

    @ui(widget="slider", min=1, max=10)
    replicas: int = 2

resources = []
"#;

    fn write_package(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("package.k");
        fs::write(&p, body).unwrap();
        (tmp, p)
    }

    #[test]
    fn human_emits_pretty_json_schema_to_stdout() {
        let (_tmp, path) = write_package(PKG);
        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &ExportArgs {
                package_path: &path,
                format: ExportFormat::JsonSchema,
                out: None,
            },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        let body = String::from_utf8(stdout).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            parsed["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(parsed["properties"]["name"]["x-ui"]["order"], 10);
        assert_eq!(parsed["properties"]["replicas"]["x-ui"]["widget"], "slider");
    }

    #[test]
    fn json_envelope_carries_format_and_schema_keys() {
        let (_tmp, path) = write_package(PKG);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &ExportArgs {
                package_path: &path,
                format: ExportFormat::Openapi,
                out: None,
            },
            &mut stdout,
        )
        .expect("run");
        let parsed: Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["format"], "openapi");
        assert_eq!(parsed["schema"]["openapi"], "3.1.0");
        assert!(parsed["schema"]["components"]["schemas"]["Input"].is_object());
    }

    #[test]
    fn out_writes_file_and_human_prints_confirmation() {
        let (tmp, path) = write_package(PKG);
        let target = tmp.path().join("inputs.schema.json");
        let mut stdout = Vec::new();
        run(
            &Context::human(),
            &ExportArgs {
                package_path: &path,
                format: ExportFormat::JsonSchema,
                out: Some(&target),
            },
            &mut stdout,
        )
        .expect("run");
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.starts_with("wrote "), "got: {text}");
        let written: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(written["properties"]["name"]["type"], "string");
    }

    #[test]
    fn missing_file_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.k");
        let err = run(
            &Context::human(),
            &ExportArgs {
                package_path: &missing,
                format: ExportFormat::JsonSchema,
                out: None,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }
}
