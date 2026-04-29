//! Universal CLI flags per [cli-contract](../../../../docs/cli-contract.md).
//!
//! Every verb flattens these into its own clap `Args`. One source of
//! truth for `--json`, `--plan`, `--timeout`, `--idempotency-key`, and
//! the output-control triplet.

use clap::Args;

/// The universal flag set every `akua` verb accepts.
///
/// Consumed via `#[command(flatten)]` on each per-verb Args struct.
#[derive(Debug, Clone, Args, Default)]
pub struct UniversalArgs {
    /// Emit structured JSON to stdout (§1.1).
    ///
    /// Auto-enables when akua detects an agent context unless
    /// overridden by `--no-json`.
    #[arg(long, global = true, group = "output_mode", action = clap::ArgAction::SetTrue)]
    pub json: bool,

    /// Force human-readable text output.
    ///
    /// Explicit opt-out from agent-context auto-detection
    /// (cli-contract §1.5).
    #[arg(long, global = true, group = "output_mode", action = clap::ArgAction::SetTrue)]
    pub no_json: bool,

    /// Compute the plan; do not write (§4).
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub plan: bool,

    /// Wall-clock cap; exits code 6 on expiry (§5).
    ///
    /// Go-duration format: `30s`, `5m`, `1h`, `250ms`. Invalid
    /// values fail with `E_INVALID_FLAG`.
    #[arg(long, global = true, value_name = "DURATION")]
    pub timeout: Option<String>,

    /// Retry-safe key for write operations (§3).
    #[arg(long, global = true, value_name = "UUID")]
    pub idempotency_key: Option<String>,

    /// Structured log format to stderr.
    ///
    /// Defaults to `text`; auto-enables `json` in agent context.
    #[arg(long, global = true, value_name = "FORMAT", value_parser = ["text", "json"])]
    pub log: Option<String>,

    /// Log severity filter.
    #[arg(long, global = true, value_name = "LEVEL", value_parser = ["debug", "info", "warn", "error"])]
    pub log_level: Option<String>,

    /// More detail in logs (does not change stdout format).
    #[arg(short, long, global = true, action = clap::ArgAction::SetTrue)]
    pub verbose: bool,

    /// Disable color codes in human-readable output.
    ///
    /// Auto-disabled under `--json` and in agent context.
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub no_color: bool,

    /// Suppress spinners and animated progress.
    ///
    /// Auto-disabled in agent context.
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub no_progress: bool,

    /// Never block on stdin (fail with exit 1 if required).
    ///
    /// Auto-enabled in agent context.
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub no_interactive: bool,

    /// Disable agent-context auto-detection for this invocation.
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub no_agent_mode: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches, Parser};

    // Wrap UniversalArgs in a Parser so we can exercise the flags end-to-end.
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(flatten)]
        args: UniversalArgs,
    }

    fn parse(args: &[&str]) -> TestCli {
        TestCli::parse_from(std::iter::once("test").chain(args.iter().copied()))
    }

    #[test]
    fn defaults_are_all_false_or_none() {
        let cli = parse(&[]);
        assert!(!cli.args.json);
        assert!(!cli.args.no_json);
        assert!(!cli.args.plan);
        assert!(cli.args.timeout.is_none());
        assert!(cli.args.idempotency_key.is_none());
        assert!(!cli.args.verbose);
        assert!(!cli.args.no_agent_mode);
    }

    #[test]
    fn parses_json_flag() {
        let cli = parse(&["--json"]);
        assert!(cli.args.json);
    }

    #[test]
    fn parses_timeout_and_idempotency_key() {
        let cli = parse(&["--timeout=30s", "--idempotency-key=abc-123"]);
        assert_eq!(cli.args.timeout.as_deref(), Some("30s"));
        assert_eq!(cli.args.idempotency_key.as_deref(), Some("abc-123"));
    }

    #[test]
    fn json_and_no_json_are_mutually_exclusive() {
        let err =
            TestCli::try_parse_from(["test", "--json", "--no-json"]).expect_err("should conflict");
        // clap flags the group conflict in its message.
        let msg = err.to_string();
        assert!(
            msg.contains("--no-json") || msg.contains("cannot be used"),
            "expected group conflict, got: {msg}"
        );
    }

    #[test]
    fn rejects_unknown_log_format() {
        let err = TestCli::try_parse_from(["test", "--log=xml"]).expect_err("reject");
        assert!(err.to_string().contains("invalid value"));
    }

    #[test]
    fn rejects_unknown_log_level() {
        let err = TestCli::try_parse_from(["test", "--log-level=trace"]).expect_err("reject");
        assert!(err.to_string().contains("invalid value"));
    }

    #[test]
    fn every_flag_is_global() {
        // Smoke: the command compiles and all our flags show up.
        let cmd = TestCli::command();
        let names: Vec<_> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        for expected in [
            "json",
            "no_json",
            "plan",
            "timeout",
            "idempotency_key",
            "log",
            "log_level",
            "verbose",
            "no_color",
            "no_progress",
            "no_interactive",
            "no_agent_mode",
        ] {
            assert!(
                names.contains(&expected),
                "flag `{expected}` missing from UniversalArgs"
            );
        }
    }

    #[test]
    fn from_arg_matches_round_trips() {
        let cmd = TestCli::command();
        let matches = cmd
            .try_get_matches_from(["test", "--json", "--timeout=5m"])
            .expect("parse");
        let cli = TestCli::from_arg_matches(&matches).expect("from matches");
        assert!(cli.args.json);
        assert_eq!(cli.args.timeout.as_deref(), Some("5m"));
    }
}
