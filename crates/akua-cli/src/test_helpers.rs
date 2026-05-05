//! Crate-internal test fixtures shared across verb test modules.
//! `#[cfg(test)]`-gated so it adds nothing to release builds.

use std::fs;
use tempfile::TempDir;

/// Minimal valid akua workspace: `akua.toml` with the supplied body
/// and an empty `package.k` at the root. `package_tar::pack_workspace`
/// requires both files to be present.
pub(crate) fn workspace_with(toml_body: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("akua.toml"), toml_body).unwrap();
    fs::write(dir.path().join("package.k"), "resources = []\n").unwrap();
    dir
}
