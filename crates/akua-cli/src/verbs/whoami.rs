//! `akua whoami` — identity + agent context introspection.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua whoami` section;
//! [`cli-contract.md §1.5`](../../../../docs/cli-contract.md#15-agent-context-auto-detection).

use std::io::Write;

use akua_core::cli_contract::{AgentContext, ExitCode};
#[cfg(feature = "schema-export")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts-export")]
use ts_rs::TS;

use crate::contract::{emit_output, Context};

/// Whoami response shape. Part of the stability contract — new fields
/// may be added (backward-compatible); existing field semantics never
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(TS))]
#[cfg_attr(feature = "ts-export", ts(export, export_to = "../../../sdk-types/"))]
#[cfg_attr(feature = "schema-export", derive(JsonSchema))]
pub struct WhoamiOutput {
    /// Agent-context detection result per cli-contract §1.5.
    pub agent_context: AgentContext,

    /// akua binary version (same as `akua --version`).
    pub version: String,
}

impl WhoamiOutput {
    pub fn collect(agent: AgentContext) -> Self {
        WhoamiOutput {
            agent_context: agent,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

pub fn run<W: Write>(ctx: &Context, stdout: &mut W) -> std::io::Result<ExitCode> {
    let output = WhoamiOutput::collect(ctx.agent.clone());

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(stdout: &mut W, output: &WhoamiOutput) -> std::io::Result<()> {
    writeln!(stdout, "not logged in")?;
    writeln!(stdout, "akua version: {}", output.version)?;
    if output.agent_context.detected {
        if let (Some(src), Some(name)) = (output.agent_context.source, &output.agent_context.name) {
            writeln!(
                stdout,
                "agent context: {name} (detected via {})",
                src.env_var()
            )?;
        }
    } else if output.agent_context.disabled_via_env {
        writeln!(
            stdout,
            "agent context: detection disabled (AKUA_NO_AGENT_DETECT)"
        )?;
    }
    Ok(())
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
        let ctx = Context::resolve(&UniversalArgs::default(), agent_claude());
        let mut buf = Vec::new();
        let code = run(&ctx, &mut buf).expect("run");
        assert_eq!(code, ExitCode::Success);

        let out = String::from_utf8(buf).expect("utf-8");
        let parsed: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
        assert_eq!(parsed["agent_context"]["detected"], true);
        assert_eq!(parsed["agent_context"]["source"], "claude_code");
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
        assert!(!out.contains("agent context:"));
    }

    #[test]
    fn text_output_in_agent_context_names_the_detection_source() {
        let args = UniversalArgs {
            no_json: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, agent_claude());
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(
            out.contains("agent context: 1 (detected via CLAUDECODE)"),
            "{out}"
        );
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
