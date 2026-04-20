//! `akua-cli` library surface — exposes the post-pivot verb + contract
//! modules for in-tree tests and (eventually) the `akua` binary's main
//! dispatch.
//!
//! The v0.3 `main.rs` still carries the legacy `init` / `preview` /
//! `build` / `render` / `publish` / `attest` / `inspect` verbs against
//! the `package.yaml` shape. Those will be retired verb-by-verb as the
//! Phase A rewrite progresses. The new surface lives here.

pub mod contract;
pub mod verbs;
