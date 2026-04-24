//! New verb implementations for the post-pivot CLI surface.
//!
//! Each verb is one file. Every verb takes a [`crate::contract::Context`]
//! and returns an `akua_core::cli_contract::ExitCode`. Structured errors
//! are written via [`crate::contract::emit_error`].

pub mod add;
pub mod auth;
pub mod cache;
pub mod check;
#[cfg(feature = "dev-watch")]
pub mod dev;
pub mod diff;
pub mod fmt;
pub mod init;
pub mod inspect;
pub mod lint;
pub mod lock;
pub mod pack;
pub mod publish;
pub mod pull;
pub mod push;
pub mod remove;
pub mod render;
pub mod repl;
#[cfg(feature = "cosign-verify")]
pub mod sign;
pub mod test;
pub mod tree;
pub mod update;
pub mod vendor;
pub mod verify;
#[cfg(feature = "cosign-verify")]
pub mod verify_tarball;
pub mod version;
pub mod whoami;
