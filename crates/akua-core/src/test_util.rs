//! Shared test helpers. Only compiled with `cfg(test)`.

use std::ffi::OsString;

/// RAII guard for mutating a process env var in a test. Sets (or
/// removes) the var on construction; restores the pre-existing value on
/// drop, even if the test panics.
///
/// Rationale: `std::env::set_var` is `unsafe` since Rust 2024 (it
/// races with any other thread reading the environment). Sprinkling
/// `unsafe { set_var }` / `unsafe { remove_var }` pairs across 15+
/// tests is both verbose and leak-prone — if an assertion panics
/// between set and remove, the env stays mutated for the rest of the
/// test run. A guard with `Drop` fixes both issues.
///
/// Callers must still serialize tests that touch the *same* variable
/// via a mutex — env is process-global, and `cargo test` runs tests
/// concurrently by default.
pub(crate) struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    /// Set `key=value` for the lifetime of the returned guard.
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: documented in the type comment — caller serializes with
        // a mutex so no other thread is reading this var concurrently.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    /// Remove `key` for the lifetime of the returned guard.
    pub(crate) fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: see `set`.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: see `set`.
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_guard_restores_original_on_drop() {
        // Use a key unlikely to collide with anything else in the run.
        const KEY: &str = "AKUA_TEST_SCOPED_ENV_SET";
        // SAFETY: test-only var, no reader races expected.
        unsafe {
            std::env::set_var(KEY, "original");
        }
        {
            let _g = ScopedEnvVar::set(KEY, "overridden");
            assert_eq!(std::env::var(KEY).unwrap(), "overridden");
        }
        assert_eq!(std::env::var(KEY).unwrap(), "original");
        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[test]
    fn set_guard_removes_on_drop_when_previously_unset() {
        const KEY: &str = "AKUA_TEST_SCOPED_ENV_UNSET";
        unsafe {
            std::env::remove_var(KEY);
        }
        {
            let _g = ScopedEnvVar::set(KEY, "x");
            assert_eq!(std::env::var(KEY).unwrap(), "x");
        }
        assert!(std::env::var_os(KEY).is_none());
    }

    #[test]
    fn remove_guard_restores_value_on_drop() {
        const KEY: &str = "AKUA_TEST_SCOPED_ENV_REMOVE";
        unsafe {
            std::env::set_var(KEY, "alive");
        }
        {
            let _g = ScopedEnvVar::remove(KEY);
            assert!(std::env::var_os(KEY).is_none());
        }
        assert_eq!(std::env::var(KEY).unwrap(), "alive");
        unsafe {
            std::env::remove_var(KEY);
        }
    }
}
