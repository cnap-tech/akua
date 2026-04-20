//! Per-invocation execution context resolving args + env → effective
//! behavior for the verb.
//!
//! The verb gets one [`Context`] built at CLI entry; it tells the verb
//! whether to emit JSON, whether it's running under an agent, and
//! (optionally) the timeout / idempotency key for writes.

use akua_core::cli_contract::AgentContext;

use super::args::UniversalArgs;

/// Resolved output mode — what the verb should actually print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-readable text to stdout; colors + spinners may be enabled.
    Text,
    /// Structured JSON to stdout; colors + spinners suppressed.
    Json,
}

/// Per-invocation execution bag handed to every verb.
#[derive(Debug, Clone)]
pub struct Context {
    /// Effective output mode after agent-detection and explicit flags.
    pub output: OutputMode,

    /// The detected agent context, unmodified by CLI flags. Present so
    /// verbs can surface it (e.g. `whoami --json` includes this under
    /// `agent_context`).
    pub agent: AgentContext,

    /// Whether stdout color codes should be emitted. Always `false`
    /// when `output == Json` or when agent context is active.
    pub color: bool,

    /// Whether animated progress is allowed. Same suppression rules as
    /// `color`.
    pub progress: bool,

    /// Whether the verb may block on stdin. `false` in agent context or
    /// under `--no-interactive`.
    pub interactive: bool,

    /// Raw timeout string (e.g. `"30s"`, `"5m"`). Parsing to Duration
    /// is the verb's responsibility so the error surfaces at the right
    /// spot.
    pub timeout: Option<String>,

    /// Write-operation idempotency key (§3).
    pub idempotency_key: Option<String>,

    /// Whether `--plan` was set (§4).
    pub plan: bool,
}

impl Context {
    /// Build from CLI args + detected agent context. Explicit flags win
    /// over agent auto-detection per cli-contract §1.5 override table.
    pub fn resolve(args: &UniversalArgs, agent: AgentContext) -> Self {
        let effective_agent = if args.no_agent_mode {
            AgentContext::none()
        } else {
            agent
        };

        // Output mode resolution (§1.5 override table):
        //   --json           → Json
        //   --no-json        → Text
        //   agent + no flag  → Json
        //   no agent, no flag → Text
        let output = if args.json {
            OutputMode::Json
        } else if args.no_json {
            OutputMode::Text
        } else if effective_agent.detected {
            OutputMode::Json
        } else {
            OutputMode::Text
        };

        let agent_active = effective_agent.detected;
        let color = !args.no_color && output == OutputMode::Text && !agent_active;
        let progress = !args.no_progress && output == OutputMode::Text && !agent_active;
        let interactive = !args.no_interactive && !agent_active;

        Context {
            output,
            agent: effective_agent,
            color,
            progress,
            interactive,
            timeout: args.timeout.clone(),
            idempotency_key: args.idempotency_key.clone(),
            plan: args.plan,
        }
    }

    /// Convenience for tests and offline callers: build a default
    /// human-shell context with no agent, no flags.
    pub fn human() -> Self {
        Self::resolve(&UniversalArgs::default(), AgentContext::none())
    }

    /// Convenience for tests: build a JSON-mode context with no agent
    /// and no other flags set. Equivalent to
    /// `resolve(&UniversalArgs { json: true, ..Default::default() },
    /// AgentContext::none())`.
    pub fn json() -> Self {
        Self::resolve(
            &UniversalArgs {
                json: true,
                ..UniversalArgs::default()
            },
            AgentContext::none(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akua_core::cli_contract::AgentSource;

    fn agent_claude() -> AgentContext {
        AgentContext {
            detected: true,
            source: Some(AgentSource::ClaudeCode),
            name: Some("1".into()),
            disabled_via_env: false,
        }
    }

    #[test]
    fn human_shell_default_is_text_with_color_and_interactivity() {
        let ctx = Context::human();
        assert_eq!(ctx.output, OutputMode::Text);
        assert!(ctx.color);
        assert!(ctx.progress);
        assert!(ctx.interactive);
        assert!(!ctx.agent.detected);
    }

    #[test]
    fn agent_context_auto_enables_json_and_suppresses_ui() {
        let ctx = Context::resolve(&UniversalArgs::default(), agent_claude());
        assert_eq!(ctx.output, OutputMode::Json);
        assert!(!ctx.color);
        assert!(!ctx.progress);
        assert!(!ctx.interactive);
    }

    #[test]
    fn explicit_json_wins_in_human_shell() {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, AgentContext::none());
        assert_eq!(ctx.output, OutputMode::Json);
        // Color/progress auto-off under JSON even without agent.
        assert!(!ctx.color);
        assert!(!ctx.progress);
    }

    #[test]
    fn explicit_no_json_wins_in_agent_context() {
        let args = UniversalArgs {
            no_json: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, agent_claude());
        // Override honored per §1.5 table.
        assert_eq!(ctx.output, OutputMode::Text);
        // But UI suppression stays on because agent is still active.
        assert!(!ctx.color);
        assert!(!ctx.progress);
        assert!(!ctx.interactive);
    }

    #[test]
    fn no_agent_mode_flag_clears_the_agent_context() {
        let args = UniversalArgs {
            no_agent_mode: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, agent_claude());
        // Agent blown away → behaves like human shell.
        assert_eq!(ctx.output, OutputMode::Text);
        assert!(ctx.color);
        assert!(ctx.progress);
        assert!(ctx.interactive);
        assert!(!ctx.agent.detected);
    }

    #[test]
    fn timeout_and_idempotency_key_pass_through() {
        let args = UniversalArgs {
            timeout: Some("30s".into()),
            idempotency_key: Some("abc-123".into()),
            plan: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, AgentContext::none());
        assert_eq!(ctx.timeout.as_deref(), Some("30s"));
        assert_eq!(ctx.idempotency_key.as_deref(), Some("abc-123"));
        assert!(ctx.plan);
    }

    #[test]
    fn explicit_no_color_respected_in_human_shell() {
        let args = UniversalArgs {
            no_color: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, AgentContext::none());
        assert_eq!(ctx.output, OutputMode::Text);
        assert!(!ctx.color);
    }
}
