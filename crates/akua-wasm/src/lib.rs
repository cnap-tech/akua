//! # akua-wasm
//!
//! WASM bindings for `akua-core`, consumed from the browser via `@akua/core`.
//!
//! This crate will compile to `cdylib` via `wasm-pack`, producing a `.wasm`
//! module plus JS glue that `@akua/core` wraps in a typed TypeScript API.
//!
//! ## Status
//!
//! Placeholder. WASM build infrastructure lands in milestone v4.

pub fn placeholder() -> &'static str {
    "akua-wasm — WASM bindings pending"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_returns_message() {
        assert!(placeholder().contains("akua-wasm"));
    }
}
