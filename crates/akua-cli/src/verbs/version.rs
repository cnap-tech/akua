//! `akua version` — CLI version and build info.
//!
//! Exit code always `Success`. Output shape stable across versions;
//! only field values differ.

use std::io::Write;

use akua_core::{cli_contract::ExitCode, contract_type};
use serde::{Deserialize, Serialize};

use crate::contract::{Context, OutputMode};

contract_type! {
/// Version response shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionOutput {
    /// Semver version of the `akua` binary.
    pub version: String,
}
}

impl Default for VersionOutput {
    fn default() -> Self {
        VersionOutput {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

pub fn run<W: Write>(ctx: &Context, stdout: &mut W) -> std::io::Result<ExitCode> {
    let output = VersionOutput::default();
    match ctx.output {
        OutputMode::Json => {
            let json =
                serde_json::to_string(&output).expect("VersionOutput serialization is infallible");
            writeln!(stdout, "{json}")?;
        }
        OutputMode::Text => {
            writeln!(stdout, "akua {}", output.version)?;
        }
    }
    Ok(ExitCode::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;
    use akua_core::cli_contract::AgentContext;

    #[test]
    fn text_output_is_human_readable() {
        let ctx = Context::human();
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        assert!(out.starts_with("akua "), "got: {out:?}");
        // Trailing newline.
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn json_output_is_parseable() {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        let ctx = Context::resolve(&args, AgentContext::none());
        let mut buf = Vec::new();
        run(&ctx, &mut buf).expect("run");
        let out = String::from_utf8(buf).expect("utf-8");
        let parsed: VersionOutput = serde_json::from_str(out.trim()).expect("parse");
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
    }
}
