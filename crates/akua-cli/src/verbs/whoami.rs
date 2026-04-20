//! `akua whoami` — identity + agent context introspection.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua whoami` section;
//! [`cli-contract.md §1.5`](../../../../docs/cli-contract.md#15-agent-context-auto-detection).
//!
//! Phase A surface: no real registry credentials yet. Returns the
//! agent context and a placeholder identity. Future phases extend the
//! response shape with registry logins + scoped tokens.

use std::io::Write;

use akua_core::cli_contract::{AgentContext, ExitCode};
use serde::{Deserialize, Serialize};

use crate::contract::{Context, OutputMode};

/// Whoami response shape. Part of the stability contract — new fields
/// may be added (backward-compatible); existing field semantics never
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhoamiOutput {
    /// The current identity. `None` when not logged in to any registry.
    pub identity: Option<String>,

    /// Agent-context detection result per cli-contract §1.5.
    pub agent_context: AgentContext,

    /// akua binary version (same as `akua --version`).
    pub version: String,
}

impl WhoamiOutput {
    pub fn collect(agent: AgentContext) -> Self {
        WhoamiOutput {
            identity: None,
            agent_context: agent,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Run the verb against the given context, writing output to `stdout`.
pub fn run<W: Write>(ctx: &Context, stdout: &mut W) -> std::io::Result<ExitCode> {
    let output = WhoamiOutput::collect(ctx.agent.clone());

    match ctx.output {
        OutputMode::Json => {
            let json = serde_json::to_string(&output)
                .expect("WhoamiOutput serialization is infallible");
            writeln!(stdout, "{json}")?;
        }
        OutputMode::Text => {
            match &output.identity {
                Some(id) => writeln!(stdout, "logged in as: {id}")?,
                None => writeln!(stdout, "not logged in")?,
            }
            writeln!(stdout, "akua version: {}", output.version)?;
            if output.agent_context.detected {
                if let (Some(src), Some(name)) =
                    (output.agent_context.source, &output.agent_context.name)
                {
                    writeln!(
                        stdout,
                        "agent context: {name} (detected via {})",
                        src.env_var()
                    )?;
                }
            } else if output.agent_context.disabled_via_env {
                writeln!(stdout, "agent context: detection disabled (AKUA_NO_AGENT_DETECT)")?;
            }
        }
    }

    Ok(ExitCode::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use akua_core::cli_contract::AgentSource;

    use crate::contract::args::UniversalArgs;

    fn agent_claude() -> AgentContext {
        AgentContext {
            detected: true,
            source: Some(AgentSource::ClaudeCode),
            name: Some("1".into()),
            disabled_via_env: false,
        }
    }

    #[test]
    fn json_output_includes_agent_context() {
        let ctx = Context::resolve(
            &UniversalArgs::default(),
            agent_claude(),
        );
        let mut buf = Vec::new();
        let code = run(&ctx, &mut buf).expect("run");
        assert_eq!(code, ExitCode::Success);

        let out = String::from_utf8(buf).expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
        assert_eq!(parsed["agent_context"]["detected"], true);
        assert_eq!(parsed["agent_context"]["source"], "claude_code");
        assert_eq!(parsed["identity"], serde_json::Value::Null);
        assert!(parsed["version"].is_string());
    }

    #[test]
    fn text_output_in_human_shell_is_compact() {
        let ctx = Context::human();
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.contains("not logged in"));
        assert!(out.contains("akua version:"));
        // No agent-context line when no agent.
        assert!(!out.contains("agent context:"));
    }

    #[test]
    fn text_output_in_agent_context_names_the_detection_source() {
        let args = UniversalArgs {
            no_json: true, // explicit opt-out → text mode in agent context
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, agent_claude());
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.contains("agent context: 1 (detected via CLAUDECODE)"), "{out}");
    }

    #[test]
    fn text_output_reports_disabled_detection() {
        let ctx = Context::resolve(
            &UniversalArgs::default(),
            AgentContext {
                detected: false,
                source: None,
                name: None,
                disabled_via_env: true,
            },
        );
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.contains("AKUA_NO_AGENT_DETECT"), "{out}");
    }

    #[test]
    fn returns_success_exit_code() {
        let ctx = Context::human();
        let mut buf = Vec::new();
        let code = run(&ctx, &mut buf).expect("run");
        assert_eq!(code, ExitCode::Success);
    }
}
