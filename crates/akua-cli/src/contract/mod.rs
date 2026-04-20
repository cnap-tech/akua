//! CLI-level glue between `akua-core::cli_contract` primitives and clap.
//!
//! Spec: [`docs/cli-contract.md`](../../../../docs/cli-contract.md).
//!
//! This module exists because `cli_contract` in the library is
//! intentionally clap-free — it carries the pure types (ExitCode,
//! AgentContext, StructuredError) that any consumer needs. This module
//! takes those types and wires them into:
//!
//! - [`UniversalArgs`] — the clap `Args` struct every verb embeds via
//!   `#[command(flatten)]`. One definition, one source of truth for the
//!   contract's universal flags.
//! - [`Context`] — the per-invocation execution bag: resolved output
//!   mode (after agent auto-detection + explicit overrides), the
//!   AgentContext for introspection, a stderr writer for structured
//!   errors, and the timeout / idempotency key.
//! - [`EmitError`] — helper that writes a `StructuredError` to stderr
//!   in the format cli-contract §1.2 specifies (JSON-lines when
//!   `--json`, human-readable otherwise).

pub mod args;
pub mod context;
pub mod emit;

pub use args::UniversalArgs;
pub use context::{Context, OutputMode};
pub use emit::emit_error;
