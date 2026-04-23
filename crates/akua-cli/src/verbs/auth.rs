//! `akua auth` — manage credentials in `$XDG_CONFIG_HOME/akua/auth.toml`.
//!
//! Subverbs:
//! - `akua auth list` — enumerate every configured registry across
//!   both akua/auth.toml and ~/.docker/config.json. Never prints the
//!   secret — only `{registry, source, auth_kind}`.
//! - `akua auth add --registry <host> --username <u>` — reads the
//!   password from stdin (operator-safe; mirrors
//!   `docker login --password-stdin`). Writes to akua/auth.toml.
//! - `akua auth add --registry <host> --token` — reads a bearer
//!   token from stdin. Used for GHCR classic PATs, DockerHub PATs.
//! - `akua auth remove --registry <host>` — drop the entry.
//!
//! Why not interactive prompt + password masking? Tested-code-only,
//! scriptable, and matches the docker idiom. No TTY dependency.

use std::io::{Read, Write};
use std::path::PathBuf;

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::oci_auth::{self, AuthLoadError, Credentials, RegistrySummary};
use serde::Serialize;

use crate::contract::{emit_output, Context};

/// Reader abstraction so tests inject synthetic stdin without
/// touching the process's real stdin.
pub trait SecretReader {
    fn read_secret(&mut self) -> std::io::Result<String>;
}

/// Default reader: drain the process's stdin to EOF, trim trailing
/// newline. Matches `docker login --password-stdin` semantics.
pub struct StdinReader;

impl SecretReader for StdinReader {
    fn read_secret(&mut self) -> std::io::Result<String> {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(trim_secret(&buf))
    }
}

fn trim_secret(s: &str) -> String {
    // Strip a single trailing newline (\n or \r\n) — shells pipe
    // `echo | akua auth add ...` commonly. Don't strip whitespace
    // that may be part of the token (tokens are typically
    // base64url and can't contain whitespace anyway).
    let trimmed = s.strip_suffix('\n').unwrap_or(s);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    trimmed.to_string()
}

#[derive(Debug, Clone)]
pub enum AuthAction {
    List,
    Add(AuthAddInput),
    Remove { registry: String },
}

#[derive(Debug, Clone)]
pub enum AuthAddInput {
    Basic { registry: String, username: String },
    Bearer { registry: String },
}

#[derive(Debug, Clone)]
pub struct AuthArgs {
    pub action: AuthAction,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AuthOutput {
    List(AuthListBody),
    Add(AuthAddBody),
    Remove(AuthRemoveBody),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AuthListBody {
    pub path: Option<PathBuf>,
    pub registries: Vec<RegistrySummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AuthAddBody {
    pub path: PathBuf,
    pub registry: String,
    pub auth_kind: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AuthRemoveBody {
    pub path: PathBuf,
    pub registry: String,
    pub removed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthVerbError {
    #[error("no akua auth config path — set $XDG_CONFIG_HOME or $HOME")]
    NoConfigPath,

    #[error(transparent)]
    Auth(#[from] AuthLoadError),

    #[error("reading secret from stdin: {0}")]
    SecretRead(#[source] std::io::Error),

    #[error("empty secret — `akua auth add` requires a non-empty value on stdin")]
    EmptySecret,

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl AuthVerbError {
    pub fn to_structured(&self) -> StructuredError {
        StructuredError::new(codes::E_IO, self.to_string()).with_default_docs()
    }
    pub fn exit_code(&self) -> ExitCode {
        match self {
            AuthVerbError::StdoutWrite(_) => ExitCode::SystemError,
            AuthVerbError::NoConfigPath | AuthVerbError::EmptySecret => ExitCode::UserError,
            AuthVerbError::Auth(_) | AuthVerbError::SecretRead(_) => ExitCode::SystemError,
        }
    }
}

pub fn run<W: Write, R: SecretReader>(
    ctx: &Context,
    args: &AuthArgs,
    stdout: &mut W,
    secret_in: &mut R,
) -> Result<ExitCode, AuthVerbError> {
    let output = match &args.action {
        AuthAction::List => {
            let registries = oci_auth::list_sources()?;
            AuthOutput::List(AuthListBody {
                path: oci_auth::akua_auth_path(),
                registries,
            })
        }
        AuthAction::Add(input) => {
            let path = oci_auth::akua_auth_path().ok_or(AuthVerbError::NoConfigPath)?;
            let secret = secret_in
                .read_secret()
                .map_err(AuthVerbError::SecretRead)?;
            if secret.is_empty() {
                return Err(AuthVerbError::EmptySecret);
            }
            let (registry, creds, auth_kind) = match input {
                AuthAddInput::Basic { registry, username } => (
                    registry.clone(),
                    Credentials::Basic {
                        username: username.clone(),
                        password: secret,
                    },
                    "basic",
                ),
                AuthAddInput::Bearer { registry } => (
                    registry.clone(),
                    Credentials::Bearer { token: secret },
                    "bearer",
                ),
            };
            oci_auth::upsert_entry(&path, &registry, &creds)?;
            AuthOutput::Add(AuthAddBody {
                path,
                registry,
                auth_kind,
            })
        }
        AuthAction::Remove { registry } => {
            let path = oci_auth::akua_auth_path().ok_or(AuthVerbError::NoConfigPath)?;
            let removed = oci_auth::remove_entry(&path, registry)?;
            AuthOutput::Remove(AuthRemoveBody {
                path,
                registry: registry.clone(),
                removed,
            })
        }
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(AuthVerbError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, output: &AuthOutput) -> std::io::Result<()> {
    match output {
        AuthOutput::List(body) => {
            if body.registries.is_empty() {
                writeln!(w, "no registries configured")?;
            } else {
                for r in &body.registries {
                    writeln!(
                        w,
                        "  {:<40}  {:<6}  {}",
                        r.registry, r.auth_kind, r.source
                    )?;
                }
            }
            if let Some(path) = &body.path {
                writeln!(w, "config: {}", path.display())?;
            }
            Ok(())
        }
        AuthOutput::Add(body) => writeln!(
            w,
            "wrote {} auth for {} → {}",
            body.auth_kind,
            body.registry,
            body.path.display()
        ),
        AuthOutput::Remove(body) => {
            if body.removed {
                writeln!(w, "removed {} from {}", body.registry, body.path.display())
            } else {
                writeln!(
                    w,
                    "no entry for {} in {}",
                    body.registry,
                    body.path.display()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;

    struct MemReader {
        secret: String,
    }
    impl SecretReader for MemReader {
        fn read_secret(&mut self) -> std::io::Result<String> {
            Ok(self.secret.clone())
        }
    }

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    fn env_scope_to(tmp: &std::path::Path) {
        // Isolate write paths to the temp dir.
        std::env::set_var("XDG_CONFIG_HOME", tmp);
        std::env::set_var("DOCKER_CONFIG", tmp.join("docker-root"));
        std::env::remove_var("HOME");
    }

    #[test]
    fn add_basic_writes_to_akua_auth_toml() {
        let _lock = lock_serial();
        let tmp = tempfile::tempdir().unwrap();
        env_scope_to(tmp.path());

        let mut stdout = Vec::new();
        let mut reader = MemReader {
            secret: "hunter2".into(),
        };
        let args = AuthArgs {
            action: AuthAction::Add(AuthAddInput::Basic {
                registry: "ghcr.io".into(),
                username: "alice".into(),
            }),
        };
        let code = run(&ctx_json(), &args, &mut stdout, &mut reader).unwrap();
        assert_eq!(code, ExitCode::Success);

        let path = oci_auth::akua_auth_path().unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("ghcr.io"), "{body}");
        assert!(body.contains("alice"), "{body}");
        assert!(body.contains("hunter2"), "{body}");
        cleanup_env();
    }

    #[test]
    fn add_bearer_writes_token_only() {
        let _lock = lock_serial();
        let tmp = tempfile::tempdir().unwrap();
        env_scope_to(tmp.path());

        let mut stdout = Vec::new();
        let mut reader = MemReader {
            secret: "ghp_deadbeef".into(),
        };
        let args = AuthArgs {
            action: AuthAction::Add(AuthAddInput::Bearer {
                registry: "ghcr.io".into(),
            }),
        };
        run(&ctx_json(), &args, &mut stdout, &mut reader).unwrap();

        let body =
            std::fs::read_to_string(oci_auth::akua_auth_path().unwrap()).unwrap();
        assert!(body.contains("token"), "{body}");
        assert!(body.contains("ghp_deadbeef"), "{body}");
        assert!(!body.contains("password"), "{body}");
        cleanup_env();
    }

    #[test]
    fn add_empty_secret_errors_with_user_exit() {
        let _lock = lock_serial();
        let tmp = tempfile::tempdir().unwrap();
        env_scope_to(tmp.path());

        let mut stdout = Vec::new();
        let mut reader = MemReader { secret: "".into() };
        let err = run(
            &ctx_json(),
            &AuthArgs {
                action: AuthAction::Add(AuthAddInput::Bearer {
                    registry: "ghcr.io".into(),
                }),
            },
            &mut stdout,
            &mut reader,
        )
        .unwrap_err();
        assert!(matches!(err, AuthVerbError::EmptySecret));
        assert_eq!(err.exit_code(), ExitCode::UserError);
        cleanup_env();
    }

    #[test]
    fn remove_reports_whether_entry_existed() {
        let _lock = lock_serial();
        let tmp = tempfile::tempdir().unwrap();
        env_scope_to(tmp.path());

        // Seed.
        let path = oci_auth::akua_auth_path().unwrap();
        oci_auth::upsert_entry(
            &path,
            "ghcr.io",
            &Credentials::Bearer { token: "x".into() },
        )
        .unwrap();

        let mut stdout = Vec::new();
        let mut reader = MemReader { secret: "".into() };
        run(
            &ctx_json(),
            &AuthArgs {
                action: AuthAction::Remove {
                    registry: "ghcr.io".into(),
                },
            },
            &mut stdout,
            &mut reader,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["action"], "remove");
        assert_eq!(parsed["removed"], true);

        // Second run → now absent.
        let mut stdout2 = Vec::new();
        run(
            &ctx_json(),
            &AuthArgs {
                action: AuthAction::Remove {
                    registry: "ghcr.io".into(),
                },
            },
            &mut stdout2,
            &mut reader,
        )
        .unwrap();
        let parsed2: serde_json::Value = serde_json::from_slice(&stdout2).unwrap();
        assert_eq!(parsed2["removed"], false);
        cleanup_env();
    }

    #[test]
    fn list_shows_added_entries_with_source_tag() {
        let _lock = lock_serial();
        let tmp = tempfile::tempdir().unwrap();
        env_scope_to(tmp.path());

        let path = oci_auth::akua_auth_path().unwrap();
        let secret = "ghp_unique_token_material_xyz123";
        oci_auth::upsert_entry(
            &path,
            "ghcr.io",
            &Credentials::Bearer {
                token: secret.into(),
            },
        )
        .unwrap();

        let mut stdout = Vec::new();
        let mut reader = MemReader { secret: "".into() };
        run(
            &ctx_json(),
            &AuthArgs {
                action: AuthAction::List,
            },
            &mut stdout,
            &mut reader,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["action"], "list");
        let regs = parsed["registries"].as_array().unwrap();
        let entry = regs
            .iter()
            .find(|e| e["registry"] == "ghcr.io")
            .expect("ghcr.io");
        assert_eq!(entry["source"], "akua");
        assert_eq!(entry["auth_kind"], "bearer");
        // Secret material must never be echoed by list.
        let out = String::from_utf8(stdout).unwrap();
        assert!(!out.contains(secret), "secret leaked: {out}");
        cleanup_env();
    }

    #[test]
    fn trim_secret_strips_single_trailing_newline() {
        assert_eq!(trim_secret("hunter2\n"), "hunter2");
        assert_eq!(trim_secret("hunter2\r\n"), "hunter2");
        assert_eq!(trim_secret("hunter2"), "hunter2");
        // Do NOT trim multiple newlines — only the last, a single one.
        assert_eq!(trim_secret("a\n\n"), "a\n");
    }

    fn cleanup_env() {
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("DOCKER_CONFIG");
    }

    // Tests that manipulate process-wide env vars must run serially,
    // otherwise one test's setup races another's cleanup. Poison is
    // ignored — a prior panic doesn't invalidate the lock for this
    // test run; we just need mutual exclusion.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|p| p.into_inner())
    }
}
