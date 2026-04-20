//! CLI contract primitives — the types every `akua` verb shares.
//!
//! Spec: [`docs/cli-contract.md`](../../../../docs/cli-contract.md).
//!
//! This module exports three things every verb needs, independent of
//! clap/CLI plumbing:
//!
//! - [`ExitCode`] — the seven typed exit codes (§2).
//! - [`AgentContext`] — env-based detection of agent sessions (§1.5).
//! - [`StructuredError`] — the machine-readable error shape (§1.2).
//!
//! Keeping these in the library (`akua-core`) rather than the binary
//! (`akua-cli`) means they're reusable by `@akua/sdk` via WASM bindings,
//! by integration tests, and by any future embedded consumer.

pub mod agent;
pub mod error;
pub mod exit;

pub use agent::{AgentContext, AgentSource};
pub use error::StructuredError;
pub use exit::ExitCode;
