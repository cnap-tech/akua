//! Error + primary-output emission per [cli-contract](../../../../docs/cli-contract.md).
//!
//! - [`emit_output`] — write a verb's typed result to stdout. JSON
//!   mode streams the value; text mode delegates to the verb's own
//!   human-readable rendering closure.
//! - [`emit_error`] — write a structured error to stderr. §1.2.

use std::io::Write;

use akua_core::cli_contract::StructuredError;
use serde::Serialize;

use super::context::{Context, OutputMode};

/// Write a verb's primary output to stdout.
///
/// JSON mode streams `value` via `serde_json::to_writer` (no
/// intermediate `String`) followed by a trailing newline. Text mode
/// delegates to the caller's closure so each verb owns its human
/// rendering.
pub fn emit_output<W, T, F>(
    writer: &mut W,
    ctx: &Context,
    value: &T,
    text: F,
) -> std::io::Result<()>
where
    W: Write,
    T: Serialize,
    F: FnOnce(&mut W) -> std::io::Result<()>,
{
    match ctx.output {
        OutputMode::Json => {
            serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
            writeln!(writer)?;
        }
        OutputMode::Text => text(writer)?,
    }
    Ok(())
}

/// Write a structured error to the given stderr writer. Code appears
/// in both JSON and text modes so scripts can grep either.
pub fn emit_error<W: Write>(
    writer: &mut W,
    ctx: &Context,
    err: &StructuredError,
) -> std::io::Result<()> {
    match ctx.output {
        OutputMode::Json => {
            writeln!(writer, "{}", err.to_json_line())?;
        }
        OutputMode::Text => {
            writeln!(writer, "error[{}]: {}", err.code, err.message)?;
            if let Some(path) = &err.path {
                writeln!(
                    writer,
                    "  at {}",
                    format_location(path, err.field.as_deref(), err.line)
                )?;
            }
            if let Some(suggestion) = &err.suggestion {
                writeln!(writer, "  suggestion: {suggestion}")?;
            }
            if let Some(docs) = &err.docs {
                writeln!(writer, "  docs: {docs}")?;
            }
            if !err.next_actions.is_empty() {
                writeln!(writer, "  next:")?;
                for action in &err.next_actions {
                    writeln!(writer, "    - {action}")?;
                }
            }
        }
    }
    Ok(())
}

fn format_location(path: &str, field: Option<&str>, line: Option<u32>) -> String {
    match (field, line) {
        (Some(field), Some(line)) => format!("{path}:{line} ({field})"),
        (Some(field), None) => format!("{path} ({field})"),
        (None, Some(line)) => format!("{path}:{line}"),
        (None, None) => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akua_core::cli_contract::AgentContext;

    use crate::contract::args::UniversalArgs;

    fn json_ctx() -> Context {
        Context::resolve(
            &UniversalArgs {
                json: true,
                ..UniversalArgs::default()
            },
            AgentContext::none(),
        )
    }

    fn text_ctx() -> Context {
        Context::human()
    }

    #[test]
    fn json_mode_writes_a_single_jsonlines_record() {
        let mut buf = Vec::new();
        let err = StructuredError::new("E_TEST", "something broke");
        emit_error(&mut buf, &json_ctx(), &err).expect("write");
        let out = String::from_utf8(buf).expect("utf-8");
        // Exactly one line, ending with newline.
        assert_eq!(out.matches('\n').count(), 1);
        let parsed: serde_json::Value = serde_json::from_str(out.trim_end()).expect("valid json");
        assert_eq!(parsed["code"], "E_TEST");
        assert_eq!(parsed["message"], "something broke");
    }

    #[test]
    fn text_mode_includes_stable_code_for_grepping() {
        let mut buf = Vec::new();
        let err = StructuredError::new("E_SCHEMA_INVALID", "bad field");
        emit_error(&mut buf, &text_ctx(), &err).expect("write");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.contains("error[E_SCHEMA_INVALID]"), "got: {out}");
        assert!(out.contains("bad field"));
    }

    #[test]
    fn text_mode_renders_context_fields_when_present() {
        let mut buf = Vec::new();
        let err = StructuredError::new("E_SCHEMA_INVALID", "expected int, got string")
            .with_path("apps/api/inputs.yaml")
            .with_field("spec.replicas")
            .with_line(14)
            .with_suggestion("remove quotes around 3")
            .with_default_docs()
            .with_next_action("edit apps/api/inputs.yaml and re-run akua lint");
        emit_error(&mut buf, &text_ctx(), &err).expect("write");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(
            out.contains("at apps/api/inputs.yaml:14 (spec.replicas)"),
            "{out}"
        );
        assert!(out.contains("suggestion: remove quotes around 3"), "{out}");
        assert!(out.contains("docs: https://akua.dev/errors/E_SCHEMA_INVALID"));
        assert!(out.contains("next:"));
        assert!(out.contains("edit apps/api/inputs.yaml"));
    }

    #[test]
    fn text_mode_omits_absent_context_fields() {
        let mut buf = Vec::new();
        let err = StructuredError::new("E_X", "y");
        emit_error(&mut buf, &text_ctx(), &err).expect("write");
        let out = String::from_utf8(buf).expect("utf-8");
        let lines: Vec<_> = out.lines().collect();
        // Just the header line; no "at", "suggestion", "docs", "next:".
        assert_eq!(lines.len(), 1, "got unexpected extras: {out}");
    }
}
