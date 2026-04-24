//! `akua repl` — interactive KCL shell.
//!
//! Accumulates every submitted line into a growing `.k` source
//! buffer, re-evaluates on each submit via the wasmtime-hosted
//! render worker (see `verbs::render::eval_source_in_worker`),
//! prints new top-level bindings. Users can explore schemas, build up
//! data structures, and load files from disk (`.load <path>`) without
//! setting up a full workspace.
//!
//! Deliberately minimal for this slice:
//!
//! - Plain-line editor — `std::io::stdin().read_line`. Users who want
//!   history + arrow-keys can wrap with `rlwrap akua repl`.
//! - No Rego layer. Lands when the policy engine phase is designed.
//! - No engine callables (`helm.template`, `pkg.render`, etc.) —
//!   those belong inside a workspace the repl doesn't materialize.
//!
//! Meta-commands (start with `.`):
//! - `.load <path>` — append a file's contents to the session
//! - `.reset`       — clear accumulated state
//! - `.show`        — print the current accumulated buffer
//! - `.exit`        — quit (Ctrl-D also works)
//!
//! JSON output is not meaningful for an interactive verb; `--json`
//! falls back to a one-line "repl doesn't emit JSON" banner + text
//! mode. Agents invoking `akua repl` programmatically should use
//! `akua render` or `akua inspect` instead.
//!
//! ## KCL quirks
//!
//! - KCL's upstream `rustc_span` asserts filenames don't end with `>`,
//!   so the repl tags every eval as `repl.k` rather than the natural
//!   `<repl:N>`.
//! - Single-letter identifiers like `y` / `n` collide with YAML's
//!   bool scalars and get emitted quoted (`'y': 2`). Tests use
//!   longer names (`alpha`, `beta`) to keep substring assertions
//!   simple; this has no impact on user-typed sessions since the
//!   quoted form is still valid YAML.

use std::io::{BufRead, Write};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::PackageKError;

use crate::contract::Context;

pub struct ReplArgs;

#[derive(Debug, thiserror::Error)]
pub enum ReplError {
    #[error("stdin read failed: {0}")]
    StdinRead(#[source] std::io::Error),

    #[error("stdout write failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl ReplError {
    pub fn to_structured(&self) -> StructuredError {
        StructuredError::new(codes::E_IO, self.to_string()).with_default_docs()
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::SystemError
    }
}

pub fn run<R: BufRead, W: Write>(
    ctx: &Context,
    _args: &ReplArgs,
    stdin: &mut R,
    stdout: &mut W,
) -> Result<ExitCode, ReplError> {
    if matches!(ctx.output, crate::contract::OutputMode::Json) {
        writeln!(
            stdout,
            "{{\"note\":\"akua repl is interactive; JSON output is not supported — falling back to text\"}}"
        )
        .map_err(ReplError::StdoutWrite)?;
    } else {
        writeln!(
            stdout,
            "akua repl — KCL interactive (type `.exit` or Ctrl-D to quit, `.help` for commands)"
        )
        .map_err(ReplError::StdoutWrite)?;
    }

    let mut session = Session::new();

    loop {
        stdout
            .write_all(session.prompt().as_bytes())
            .map_err(ReplError::StdoutWrite)?;
        stdout.flush().map_err(ReplError::StdoutWrite)?;

        let mut line = String::new();
        let n = stdin.read_line(&mut line).map_err(ReplError::StdinRead)?;
        if n == 0 {
            // EOF — exit cleanly so the repl composes with piped
            // scripts (`echo "x = 42" | akua repl` → one eval + exit).
            writeln!(stdout).map_err(ReplError::StdoutWrite)?;
            return Ok(ExitCode::Success);
        }
        let input = line.trim_end_matches(['\n', '\r']).to_string();

        match session.handle(&input) {
            SessionOutcome::Continue => {}
            SessionOutcome::Quit => return Ok(ExitCode::Success),
            SessionOutcome::Render(text) => {
                stdout
                    .write_all(text.as_bytes())
                    .map_err(ReplError::StdoutWrite)?;
                if !text.ends_with('\n') {
                    writeln!(stdout).map_err(ReplError::StdoutWrite)?;
                }
            }
        }
    }
}

/// Accumulated repl state. Separated from `run` so tests drive the
/// eval loop without an stdin loop.
struct Session {
    buffer: String,
}

enum SessionOutcome {
    Continue,
    Quit,
    Render(String),
}

impl Session {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    fn prompt(&self) -> &'static str {
        "> "
    }

    fn handle(&mut self, input: &str) -> SessionOutcome {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return SessionOutcome::Continue;
        }
        if let Some(meta) = trimmed.strip_prefix('.') {
            return self.meta(meta);
        }
        // Keep each submit on its own line so KCL parses statements
        // as distinct top-level items.
        let trial = format!("{}{input}\n", self.buffer);
        match crate::verbs::render::eval_source_in_worker("repl.k", &trial) {
            Ok(yaml) => {
                self.buffer = trial;
                SessionOutcome::Render(yaml)
            }
            Err(PackageKError::KclEval(msg)) => SessionOutcome::Render(format!("error: {msg}")),
            Err(e) => SessionOutcome::Render(format!("error: {e}")),
        }
    }

    fn meta(&mut self, cmd: &str) -> SessionOutcome {
        let mut parts = cmd.splitn(2, char::is_whitespace);
        let verb = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match verb {
            "exit" | "quit" => SessionOutcome::Quit,
            "reset" => {
                self.buffer.clear();
                SessionOutcome::Render("(buffer cleared)".to_string())
            }
            "show" => {
                if self.buffer.is_empty() {
                    SessionOutcome::Render("(empty buffer)".to_string())
                } else {
                    SessionOutcome::Render(self.buffer.clone())
                }
            }
            "load" => {
                if arg.is_empty() {
                    return SessionOutcome::Render("usage: .load <path>".to_string());
                }
                let body = match std::fs::read_to_string(arg) {
                    Ok(s) => s,
                    Err(e) => {
                        return SessionOutcome::Render(format!("error: {e}"));
                    }
                };
                let trial = format!("{}{body}\n", self.buffer);
                match crate::verbs::render::eval_source_in_worker("repl.k", &trial) {
                    Ok(yaml) => {
                        self.buffer = trial;
                        SessionOutcome::Render(yaml)
                    }
                    Err(e) => SessionOutcome::Render(format!("error: {e}")),
                }
            }
            "help" => SessionOutcome::Render(
                "meta commands: .load <path> | .reset | .show | .help | .exit".to_string(),
            ),
            other => SessionOutcome::Render(format!("unknown meta command `.{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;

    fn ctx_human() -> Context {
        Context::resolve(
            &UniversalArgs {
                no_json: true,
                ..UniversalArgs::default()
            },
            akua_core::cli_contract::AgentContext::none(),
        )
    }

    #[test]
    fn single_binding_echoes_yaml() {
        let mut s = Session::new();
        match s.handle("x = 42") {
            SessionOutcome::Render(text) => assert!(text.contains("x: 42"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    // Single-letter identifiers like `y`/`n` collide with YAML's
    // bool scalars and get emitted as `'y': …`. Use longer names
    // in tests so `text.contains("beta: …")` stays simple.
    #[test]
    fn accumulated_state_survives_across_submits() {
        let mut s = Session::new();
        s.handle("alpha = 1");
        match s.handle("beta = alpha + 1") {
            SessionOutcome::Render(text) => {
                assert!(text.contains("beta: 2"), "{text}");
                assert!(text.contains("alpha: 1"), "{text}");
            }
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn syntax_error_does_not_poison_the_buffer() {
        let mut s = Session::new();
        s.handle("alpha = 42");
        // Garbage input — should surface an error but leave prior
        // bindings intact for the next submit.
        match s.handle("this is not kcl") {
            SessionOutcome::Render(text) => assert!(text.starts_with("error:"), "{text}"),
            _ => panic!("expected render"),
        }
        match s.handle("beta = alpha") {
            SessionOutcome::Render(text) => assert!(text.contains("beta: 42"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn reset_clears_accumulated_state() {
        let mut s = Session::new();
        s.handle("alpha = 42");
        s.handle(".reset");
        // After reset, `alpha` is no longer defined — referencing it fails.
        match s.handle("beta = alpha") {
            SessionOutcome::Render(text) => assert!(text.starts_with("error:"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn show_returns_current_buffer() {
        let mut s = Session::new();
        s.handle("x = 1");
        match s.handle(".show") {
            SessionOutcome::Render(text) => assert!(text.contains("x = 1"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn show_on_empty_buffer_is_labeled() {
        let mut s = Session::new();
        match s.handle(".show") {
            SessionOutcome::Render(text) => assert!(text.contains("empty"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn exit_meta_returns_quit() {
        let mut s = Session::new();
        assert!(matches!(s.handle(".exit"), SessionOutcome::Quit));
        // Alias.
        let mut s = Session::new();
        assert!(matches!(s.handle(".quit"), SessionOutcome::Quit));
    }

    #[test]
    fn unknown_meta_surfaces_warning_but_continues() {
        let mut s = Session::new();
        match s.handle(".zzz") {
            SessionOutcome::Render(text) => assert!(text.contains("unknown"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn blank_input_is_a_no_op() {
        let mut s = Session::new();
        assert!(matches!(s.handle(""), SessionOutcome::Continue));
        assert!(matches!(s.handle("   "), SessionOutcome::Continue));
    }

    #[test]
    fn load_reads_file_into_buffer() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("snippet.k");
        std::fs::write(&path, b"greeting = \"hi\"\n").unwrap();
        let mut s = Session::new();
        match s.handle(&format!(".load {}", path.display())) {
            SessionOutcome::Render(text) => {
                assert!(text.contains("greeting: hi"), "{text}");
            }
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn load_without_arg_prints_usage() {
        let mut s = Session::new();
        match s.handle(".load") {
            SessionOutcome::Render(text) => assert!(text.contains("usage"), "{text}"),
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn run_loop_pipes_eof_to_clean_exit() {
        use std::io::Cursor;
        let mut stdin = Cursor::new(b"x = 7\n");
        let mut stdout: Vec<u8> = Vec::new();
        let ctx = ctx_human();
        let code = run(&ctx, &ReplArgs, &mut stdin, &mut stdout).unwrap();
        assert_eq!(code, ExitCode::Success);
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("x: 7"), "{text}");
    }

    #[test]
    fn run_loop_exits_on_dot_exit_meta() {
        use std::io::Cursor;
        let mut stdin = Cursor::new(b".exit\n");
        let mut stdout: Vec<u8> = Vec::new();
        run(&ctx_human(), &ReplArgs, &mut stdin, &mut stdout).unwrap();
        // Quit writes the greeter + prompt, no render body.
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.starts_with("akua repl"), "{text}");
    }
}
