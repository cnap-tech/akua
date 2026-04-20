//! New verb implementations for the post-pivot CLI surface.
//!
//! Each verb is one file. Every verb takes a [`crate::contract::Context`]
//! and returns an `akua_core::cli_contract::ExitCode`. Structured errors
//! are written via [`crate::contract::emit_error`].

pub mod version;
pub mod whoami;
