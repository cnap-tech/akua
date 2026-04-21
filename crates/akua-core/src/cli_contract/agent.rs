//! Agent-context auto-detection per [cli-contract §1.5](../../../../docs/cli-contract.md#15-agent-context-auto-detection).
//!
//! When `akua` is invoked inside an AI-agent session, it detects this from
//! the environment and implicitly enables agent-friendly output defaults.
//! Detection is silent by design — no banner, no stderr announcement, no
//! prelude on stdout.
//!
//! Explicit flags always win: `--no-agent-mode` or
//! `AKUA_NO_AGENT_DETECT=1` disables detection even when the agent env
//! vars are set.

#[cfg(feature = "schema-export")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts-export")]
use ts_rs::TS;

/// Which env var triggered agent detection. Recorded in `akua whoami`
/// output for introspection and in debug-level logs for post-hoc diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(TS))]
#[cfg_attr(feature = "ts-export", ts(export, export_to = "../../../sdk-types/"))]
#[cfg_attr(feature = "schema-export", derive(JsonSchema))]
pub enum AgentSource {
    /// Generic `AGENT=<name>` variable (Goose, Amp, Codex, Cline, OpenCode).
    Agent,
    /// Claude Code (`CLAUDECODE=1`).
    ClaudeCode,
    /// Gemini CLI (`GEMINI_CLI=1`).
    GeminiCli,
    /// Cursor CLI (`CURSOR_CLI=1`).
    CursorCli,
    /// akua-specific fallback (`AKUA_AGENT=<name>`).
    AkuaAgent,
}

impl AgentSource {
    /// The env var whose presence triggered this source.
    pub const fn env_var(self) -> &'static str {
        match self {
            AgentSource::Agent => "AGENT",
            AgentSource::ClaudeCode => "CLAUDECODE",
            AgentSource::GeminiCli => "GEMINI_CLI",
            AgentSource::CursorCli => "CURSOR_CLI",
            AgentSource::AkuaAgent => "AKUA_AGENT",
        }
    }
}

/// Result of agent-context detection at process start.
///
/// When `detected == true`, the CLI layer should (per §1.5) auto-enable
/// `--json`, `--log=json`, `--no-color`, `--no-progress`, `--no-interactive`.
/// Explicit user flags win over these defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(TS))]
#[cfg_attr(feature = "ts-export", ts(export, export_to = "../../../sdk-types/"))]
#[cfg_attr(feature = "schema-export", derive(JsonSchema))]
pub struct AgentContext {
    pub detected: bool,

    /// Which env var surfaced the agent (present iff `detected`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<AgentSource>,

    /// The value of the detected env var (e.g. `"goose"` for `AGENT=goose`).
    /// For bool-style markers (`CLAUDECODE=1`) this is the raw string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Whether detection was disabled via `AKUA_NO_AGENT_DETECT=1`.
    pub disabled_via_env: bool,
}

impl AgentContext {
    /// Detect from the current process environment via `std::env::var_os`.
    pub fn detect() -> Self {
        Self::from_reader(&StdEnvReader)
    }

    /// Detect from an arbitrary env-var source. The primary use is tests,
    /// but library consumers that model their own env (e.g. a web-based
    /// playground passing fake env vars) can call this directly.
    pub fn from_reader(reader: &dyn EnvReader) -> Self {
        // Opt-out wins first. Presence check only — no allocation.
        if reader.is_set("AKUA_NO_AGENT_DETECT") {
            return Self {
                detected: false,
                source: None,
                name: None,
                disabled_via_env: true,
            };
        }

        // Precedence order matches the cli-contract table exactly.
        for (source, var) in [
            (AgentSource::Agent, "AGENT"),
            (AgentSource::ClaudeCode, "CLAUDECODE"),
            (AgentSource::GeminiCli, "GEMINI_CLI"),
            (AgentSource::CursorCli, "CURSOR_CLI"),
            (AgentSource::AkuaAgent, "AKUA_AGENT"),
        ] {
            // Shells sometimes export a variable with empty value; that
            // shouldn't be treated as a positive agent signal.
            if let Some(value) = reader.get(var).filter(|v| !v.is_empty()) {
                return Self {
                    detected: true,
                    source: Some(source),
                    name: Some(value),
                    disabled_via_env: false,
                };
            }
        }

        Self::none()
    }

    /// Convenience: build an explicitly-not-detected context (e.g. for
    /// tests or for the `--no-agent-mode` CLI flag path).
    pub fn none() -> Self {
        Self {
            detected: false,
            source: None,
            name: None,
            disabled_via_env: false,
        }
    }
}

/// Abstraction over `std::env` so tests can inject a fixture.
pub trait EnvReader {
    /// Read a variable's value as a UTF-8 string.
    fn get(&self, key: &str) -> Option<String>;

    /// Cheap presence check. Default impl delegates to `get`, but the
    /// production impl overrides with `env::var_os` to avoid the
    /// allocation when only presence matters (the opt-out path).
    fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

/// Production implementation backed by `std::env`.
pub struct StdEnvReader;

impl EnvReader for StdEnvReader {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var_os(key).and_then(|v| v.into_string().ok())
    }

    fn is_set(&self, key: &str) -> bool {
        std::env::var_os(key).is_some()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// In-memory env-var source for deterministic tests.
    struct MapEnv(HashMap<String, String>);

    impl MapEnv {
        fn new() -> Self {
            MapEnv(HashMap::new())
        }
        fn with(mut self, key: &str, val: &str) -> Self {
            self.0.insert(key.to_string(), val.to_string());
            self
        }
    }

    impl EnvReader for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn none_env_means_not_detected() {
        let ctx = AgentContext::from_reader(&MapEnv::new());
        assert!(!ctx.detected);
        assert_eq!(ctx.source, None);
        assert!(!ctx.disabled_via_env);
    }

    #[test]
    fn detects_claude_code() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("CLAUDECODE", "1"));
        assert!(ctx.detected);
        assert_eq!(ctx.source, Some(AgentSource::ClaudeCode));
        assert_eq!(ctx.name.as_deref(), Some("1"));
    }

    #[test]
    fn detects_generic_agent_with_name() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("AGENT", "goose"));
        assert!(ctx.detected);
        assert_eq!(ctx.source, Some(AgentSource::Agent));
        assert_eq!(ctx.name.as_deref(), Some("goose"));
    }

    #[test]
    fn detects_gemini_cli() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("GEMINI_CLI", "1"));
        assert_eq!(ctx.source, Some(AgentSource::GeminiCli));
    }

    #[test]
    fn detects_cursor_cli() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("CURSOR_CLI", "1"));
        assert_eq!(ctx.source, Some(AgentSource::CursorCli));
    }

    #[test]
    fn detects_akua_agent_fallback() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("AKUA_AGENT", "future-agent"));
        assert_eq!(ctx.source, Some(AgentSource::AkuaAgent));
        assert_eq!(ctx.name.as_deref(), Some("future-agent"));
    }

    #[test]
    fn precedence_generic_agent_beats_specific_markers() {
        // If AGENT is set, it wins over CLAUDECODE (per cli-contract §1.5 order).
        let ctx = AgentContext::from_reader(
            &MapEnv::new().with("AGENT", "goose").with("CLAUDECODE", "1"),
        );
        assert_eq!(ctx.source, Some(AgentSource::Agent));
        assert_eq!(ctx.name.as_deref(), Some("goose"));
    }

    #[test]
    fn precedence_claude_code_beats_gemini() {
        let ctx = AgentContext::from_reader(
            &MapEnv::new()
                .with("CLAUDECODE", "1")
                .with("GEMINI_CLI", "1"),
        );
        assert_eq!(ctx.source, Some(AgentSource::ClaudeCode));
    }

    #[test]
    fn precedence_akua_agent_is_last_fallback() {
        let ctx = AgentContext::from_reader(
            &MapEnv::new()
                .with("AKUA_AGENT", "future")
                .with("CURSOR_CLI", "1"),
        );
        assert_eq!(ctx.source, Some(AgentSource::CursorCli));
    }

    #[test]
    fn opt_out_disables_detection_even_when_markers_set() {
        let ctx = AgentContext::from_reader(
            &MapEnv::new()
                .with("AKUA_NO_AGENT_DETECT", "1")
                .with("CLAUDECODE", "1")
                .with("AGENT", "goose"),
        );
        assert!(!ctx.detected);
        assert!(ctx.disabled_via_env);
        assert_eq!(ctx.source, None);
    }

    #[test]
    fn empty_env_var_value_is_ignored() {
        // Shells sometimes export a variable with empty value; that
        // shouldn't be treated as "agent detected."
        let ctx = AgentContext::from_reader(&MapEnv::new().with("AGENT", ""));
        assert!(!ctx.detected);
    }

    #[test]
    fn env_var_names_match_cli_contract() {
        assert_eq!(AgentSource::Agent.env_var(), "AGENT");
        assert_eq!(AgentSource::ClaudeCode.env_var(), "CLAUDECODE");
        assert_eq!(AgentSource::GeminiCli.env_var(), "GEMINI_CLI");
        assert_eq!(AgentSource::CursorCli.env_var(), "CURSOR_CLI");
        assert_eq!(AgentSource::AkuaAgent.env_var(), "AKUA_AGENT");
    }

    #[test]
    fn none_helper_is_not_detected() {
        let ctx = AgentContext::none();
        assert!(!ctx.detected);
        assert!(!ctx.disabled_via_env);
    }

    #[test]
    fn serializes_to_json_contract_shape() {
        let ctx = AgentContext::from_reader(&MapEnv::new().with("CLAUDECODE", "1"));
        let json = serde_json::to_value(&ctx).expect("serialize");
        assert_eq!(json["detected"], serde_json::Value::Bool(true));
        assert_eq!(
            json["source"],
            serde_json::Value::String("claude_code".into())
        );
        assert_eq!(json["name"], serde_json::Value::String("1".into()));
        assert_eq!(json["disabled_via_env"], serde_json::Value::Bool(false));
    }

    #[test]
    fn serializes_not_detected_shape_compactly() {
        let ctx = AgentContext::none();
        let json = serde_json::to_value(&ctx).expect("serialize");
        // source and name should be absent, not null, when not detected.
        assert!(json.get("source").is_none());
        assert!(json.get("name").is_none());
        assert_eq!(json["detected"], serde_json::Value::Bool(false));
    }
}
