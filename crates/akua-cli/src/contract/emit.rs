//! Error emission per [cli-contract §1.2](../../../../docs/cli-contract.md#12-structured-errors-on-stderr).
//!
//! One helper every verb calls at the top of its error path. Writes a
//! JSON-lines record to stderr when [`Context::output`] is `Json`;
//! writes a human-readable block otherwise. The machine-readable `code`
//! appears in both modes so scripts can still grep for it.

use std::io::Write;

use akua_core::cli_contract::StructuredError;

use super::context::{Context, OutputMode};

/// Write a structured error to the given stderr writer.
///
/// Abstracted over `impl Write` so tests can capture output into a
/// Vec<u8>; production callers pass `&mut std::io::stderr().lock()`.
pub fn emit_error<W: Write>(writer: &mut W, ctx: &Context, err: &StructuredError) -> std::io::Result<()> {
    match ctx.output {
        OutputMode::Json => {
            writeln!(writer, "{}", err.to_json_line())?;
        }
        OutputMode::Text => {
            // Human block: one-line summary with code, then the
            // optional context fields. Code is always present so grep
            // still works.
            writeln!(writer, "error[{}]: {}", err.code, err.message)?;
            if let Some(path) = &err.path {
                match (&err.field, err.line) {
                    (Some(field), Some(line)) => {
                        writeln!(writer, "  at {path}:{line} ({field})")?;
                    }
                    (Some(field), None) => {
                        writeln!(writer, "  at {path} ({field})")?;
                    }
                    (None, Some(line)) => {
                        writeln!(writer, "  at {path}:{line}")?;
                    }
                    (None, None) => {
                        writeln!(writer, "  at {path}")?;
                    }
                }
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
        let parsed: serde_json::Value =
            serde_json::from_str(out.trim_end()).expect("valid json");
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
        assert!(out.contains("at apps/api/inputs.yaml:14 (spec.replicas)"), "{out}");
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
