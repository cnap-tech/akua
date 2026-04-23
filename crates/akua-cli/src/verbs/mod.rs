//! New verb implementations for the post-pivot CLI surface.
//!
//! Each verb is one file. Every verb takes a [`crate::contract::Context`]
//! and returns an `akua_core::cli_contract::ExitCode`. Structured errors
//! are written via [`crate::contract::emit_error`].

pub mod add;
pub mod cache;
pub mod check;
#[cfg(feature = "dev-watch")]
pub mod dev;
pub mod diff;
pub mod fmt;
pub mod init;
pub mod inspect;
pub mod lint;
pub mod publish;
pub mod pull;
pub mod remove;
pub mod render;
pub mod test;
pub mod tree;
pub mod verify;
pub mod version;
pub mod whoami;
