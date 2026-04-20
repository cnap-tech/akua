//! CLI-level glue: clap `Args`, the resolved-output `Context`, and the
//! JSON/text emission helpers. Wires the clap-free primitives from
//! `akua_core::cli_contract` into the binary.
//!
//! Spec: [`docs/cli-contract.md`](../../../../docs/cli-contract.md).
//!
//! - [`UniversalArgs`] — flags every verb accepts (via
//!   `#[command(flatten)]`).
//! - [`Context`] — per-invocation execution bag: output mode (after
//!   agent detection + explicit overrides), timeout, idempotency key,
//!   UI suppression booleans.
//! - [`emit_output`] / [`emit_error`] — the stdout/stderr writers that
//!   honor `ctx.output`.

pub mod args;
pub mod context;
pub mod emit;

pub use args::UniversalArgs;
pub use context::{Context, OutputMode};
pub use emit::{emit_error, emit_output};
