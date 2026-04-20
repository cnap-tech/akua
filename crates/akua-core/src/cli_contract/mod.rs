//! CLI contract primitives — the types every `akua` verb shares.
//!
//! Spec: [`docs/cli-contract.md`](../../../../docs/cli-contract.md).
//!
//! - [`ExitCode`] — the seven typed exit codes (§2).
//! - [`AgentContext`] — env-based detection of agent sessions (§1.5).
//! - [`StructuredError`] — the machine-readable error shape (§1.2).
//!
//! These are library types, independent of clap and CLI plumbing, so
//! the CLI binary, integration tests, and any library consumer share
//! one implementation.

pub mod agent;
pub mod codes;
pub mod error;
pub mod exit;

pub use agent::{AgentContext, AgentSource};
pub use error::StructuredError;
pub use exit::ExitCode;
