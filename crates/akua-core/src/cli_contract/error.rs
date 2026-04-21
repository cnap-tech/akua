//! Structured errors per [cli-contract §1.2](../../../../docs/cli-contract.md#12-structured-errors-on-stderr).
//!
//! Every error akua emits on stderr conforms to this shape. Under `--json`
//! it's printed as a single JSON-lines record per error; in text mode the
//! same fields are rendered as a human-readable block but the `code`
//! remains machine-parseable.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts-export")]
use ts_rs::TS;

/// The canonical error shape. One struct; every verb reuses it.
///
/// `code` is the stable, load-bearing identifier. `message` is the one-line
/// summary. The rest are optional pointers — a file path, a field name, a
/// suggestion, a docs URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(TS))]
#[cfg_attr(feature = "ts-export", ts(export, export_to = "../../../sdk-types/"))]
pub struct StructuredError {
    /// Log level. Always `"error"` for hard failures; `"warn"` for
    /// recoverable issues emitted alongside a success exit.
    #[serde(default = "default_level")]
    pub level: Level,

    /// Stable, machine-readable identifier (SHOUTY_SNAKE_CASE).
    /// Example: `E_SCHEMA_INVALID`, `E_OCI_DIGEST_MISMATCH`.
    pub code: String,

    /// Human-readable one-line summary.
    pub message: String,

    /// File path, OCI ref, or resource identifier the error refers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Dotted field path inside `path` when the error is field-specific.
    /// Example: `spec.replicas`, `metadata.labels.team`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,

    /// 1-based line number in `path` when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,

    /// Actionable suggestion — a one-liner the caller can apply.
    /// Absent when there's no generic fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    /// URL to the error's documentation page. Convention:
    /// `https://akua.dev/errors/<CODE>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,

    /// Machine-executable recovery steps. Agents can use these to fix
    /// the error programmatically. Each entry is a complete command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<String>,
}

/// Log severity. Almost always `Error`; `Warn` for recoverable issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts-export", derive(TS))]
#[cfg_attr(feature = "ts-export", ts(export, export_to = "../../../sdk-types/"))]
pub enum Level {
    Error,
    Warn,
}

fn default_level() -> Level {
    Level::Error
}

impl StructuredError {
    /// Construct a minimal error with just code + message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            field: None,
            line: None,
            suggestion: None,
            docs: None,
            next_actions: Vec::new(),
        }
    }

    /// Attach a file / OCI ref / resource path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Attach a field path inside `path`.
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// Attach a 1-based line number.
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Attach an actionable suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Auto-fill `docs` using the standard URL convention
    /// `https://akua.dev/errors/<code>`. Call after `new()` or `with_code()`.
    pub fn with_default_docs(mut self) -> Self {
        self.docs = Some(format!("https://akua.dev/errors/{}", self.code));
        self
    }

    /// Attach a specific docs URL (overrides the default convention).
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }

    /// Add a machine-executable recovery command.
    pub fn with_next_action(mut self, action: impl Into<String>) -> Self {
        self.next_actions.push(action.into());
        self
    }

    /// Demote to a warning (e.g. for recoverable issues that don't halt
    /// the operation but should still surface on stderr).
    pub fn as_warning(mut self) -> Self {
        self.level = Level::Warn;
        self
    }

    /// Emit as a single JSON-lines record, suitable for stderr under
    /// `--json`. Does not include a trailing newline.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("StructuredError serialization is infallible")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_error_has_code_and_message() {
        let err = StructuredError::new("E_TEST", "something broke");
        assert_eq!(err.code, "E_TEST");
        assert_eq!(err.message, "something broke");
        assert_eq!(err.level, Level::Error);
        assert!(err.path.is_none());
        assert!(err.next_actions.is_empty());
    }

    #[test]
    fn builder_composes_all_fields() {
        let err = StructuredError::new("E_SCHEMA_INVALID", "expected integer, got string")
            .with_path("apps/api/inputs.yaml")
            .with_field("spec.replicas")
            .with_line(14)
            .with_suggestion("remove quotes around 3")
            .with_default_docs()
            .with_next_action("sed -i 's/\\\"3\\\"/3/' apps/api/inputs.yaml");

        assert_eq!(err.path.as_deref(), Some("apps/api/inputs.yaml"));
        assert_eq!(err.field.as_deref(), Some("spec.replicas"));
        assert_eq!(err.line, Some(14));
        assert_eq!(err.suggestion.as_deref(), Some("remove quotes around 3"));
        assert_eq!(
            err.docs.as_deref(),
            Some("https://akua.dev/errors/E_SCHEMA_INVALID")
        );
        assert_eq!(err.next_actions.len(), 1);
    }

    #[test]
    fn json_line_matches_contract_example() {
        // Mirror the example in cli-contract §1.2 closely.
        let err = StructuredError::new("E_SCHEMA_INVALID", "expected integer, got string")
            .with_path("apps/api/inputs.yaml")
            .with_field("replicas")
            .with_suggestion("remove quotes around 3")
            .with_docs("https://akua.dev/errors/E_SCHEMA_INVALID");

        let line = err.to_json_line();
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid json");

        assert_eq!(parsed["level"], "error");
        assert_eq!(parsed["code"], "E_SCHEMA_INVALID");
        assert_eq!(parsed["message"], "expected integer, got string");
        assert_eq!(parsed["path"], "apps/api/inputs.yaml");
        assert_eq!(parsed["field"], "replicas");
        assert_eq!(parsed["suggestion"], "remove quotes around 3");
        assert_eq!(parsed["docs"], "https://akua.dev/errors/E_SCHEMA_INVALID");

        // No newline in the serialized line itself.
        assert!(!line.contains('\n'));
    }

    #[test]
    fn optional_fields_are_omitted_when_unset() {
        let err = StructuredError::new("E_X", "y");
        let line = err.to_json_line();
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid json");

        // Only `level`, `code`, `message` present.
        assert_eq!(parsed["level"], "error");
        assert_eq!(parsed["code"], "E_X");
        assert_eq!(parsed["message"], "y");
        // These should be absent (serialized `None` with skip_serializing_if).
        assert!(parsed.get("path").is_none());
        assert!(parsed.get("field").is_none());
        assert!(parsed.get("line").is_none());
        assert!(parsed.get("suggestion").is_none());
        assert!(parsed.get("docs").is_none());
        assert!(parsed.get("next_actions").is_none());
    }

    #[test]
    fn warning_level_serializes_lowercase() {
        let err = StructuredError::new("E_X", "y").as_warning();
        let line = err.to_json_line();
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid json");
        assert_eq!(parsed["level"], "warn");
    }

    #[test]
    fn round_trips_through_json() {
        let original = StructuredError::new("E_OCI_DIGEST_MISMATCH", "digest drift")
            .with_path("oci://ghcr.io/foo@sha256:abc")
            .with_next_action("akua pull --refresh oci://ghcr.io/foo");

        let json = original.to_json_line();
        let parsed: StructuredError = serde_json::from_str(&json).expect("round-trip parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn default_docs_uses_canonical_url() {
        let err = StructuredError::new("E_FOO_BAR", "m").with_default_docs();
        assert_eq!(
            err.docs.as_deref(),
            Some("https://akua.dev/errors/E_FOO_BAR")
        );
    }

    #[test]
    fn next_actions_accumulate_in_order() {
        let err = StructuredError::new("E", "m")
            .with_next_action("step 1")
            .with_next_action("step 2")
            .with_next_action("step 3");
        assert_eq!(err.next_actions, vec!["step 1", "step 2", "step 3"]);
    }
}
