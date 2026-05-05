//! Library surface for the `akua` CLI: verb implementations and the
//! contract types (universal flags, output mode, structured-error
//! emission) every verb shares.

// Verbs return errors wrapping `ChartResolveError` (~128 bytes); boxing
// every variant adds noise for zero gain on a cold error path.
#![allow(clippy::result_large_err)]

pub mod contract;
pub mod observability;
pub mod render_worker;
pub mod verbs;

#[cfg(test)]
mod test_helpers;
