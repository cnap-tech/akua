//! Integration tests: parse every `akua.toml` + `akua.lock` in `examples/`.
//!
//! These validate that the spec docs + the example files + the Rust parser
//! all agree. If any example file drifts from the format the parser expects,
//! CI catches it here.

use akua_core::{AkuaLock, AkuaManifest};
use std::fs;
use std::path::{Path, PathBuf};

/// Walk `examples/` at the workspace root and yield every (akua.toml,
/// akua.lock) pair.
fn examples_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at `crates/akua-core`; go up two levels
    // to the workspace root.
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples")
}

fn collect_example_dirs() -> Vec<PathBuf> {
    let root = examples_root();
    let mut out = Vec::new();
    for entry in fs::read_dir(&root).expect("read examples/") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() && path.join("akua.toml").exists() {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn every_example_manifest_parses() {
    let dirs = collect_example_dirs();
    assert!(!dirs.is_empty(), "no examples found under examples/");

    for dir in &dirs {
        let toml_path = dir.join("akua.toml");
        let content = fs::read_to_string(&toml_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
        AkuaManifest::parse(&content)
            .unwrap_or_else(|e| panic!("parse {}: {e}", toml_path.display()));
    }
}

#[test]
fn every_example_lockfile_parses() {
    let dirs = collect_example_dirs();
    for dir in &dirs {
        let lock_path = dir.join("akua.lock");
        if !lock_path.exists() {
            continue; // lock may be absent for brand-new packages; allowed
        }
        let content = fs::read_to_string(&lock_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));
        AkuaLock::parse(&content).unwrap_or_else(|e| panic!("parse {}: {e}", lock_path.display()));
    }
}

#[test]
fn manifest_round_trips_for_every_example() {
    for dir in collect_example_dirs() {
        let toml_path = dir.join("akua.toml");
        let content = fs::read_to_string(&toml_path).expect("read");
        let parsed = AkuaManifest::parse(&content).expect("parse");
        let serialized = parsed.to_toml().expect("serialize");
        let reparsed = AkuaManifest::parse(&serialized).expect("reparse");
        assert_eq!(
            parsed,
            reparsed,
            "round-trip drift in {}",
            toml_path.display()
        );
    }
}

#[test]
fn lockfile_round_trips_for_every_example() {
    for dir in collect_example_dirs() {
        let lock_path = dir.join("akua.lock");
        if !lock_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&lock_path).expect("read");
        let parsed = AkuaLock::parse(&content).expect("parse");
        let serialized = parsed.to_toml().expect("serialize");
        let reparsed = AkuaLock::parse(&serialized).expect("reparse");
        assert_eq!(
            parsed,
            reparsed,
            "round-trip drift in {}",
            lock_path.display()
        );
    }
}

#[test]
fn every_manifest_declared_dep_is_locked() {
    for dir in collect_example_dirs() {
        let toml_path = dir.join("akua.toml");
        let lock_path = dir.join("akua.lock");
        if !lock_path.exists() {
            continue;
        }

        let manifest =
            AkuaManifest::parse(&fs::read_to_string(&toml_path).unwrap()).expect("parse manifest");
        let lock = AkuaLock::parse(&fs::read_to_string(&lock_path).unwrap()).expect("parse lock");

        let locked_names: std::collections::HashSet<&str> =
            lock.packages.iter().map(|p| p.name.as_str()).collect();

        for dep_name in manifest.dependencies.keys() {
            assert!(
                locked_names.contains(dep_name.as_str()),
                "dep `{dep_name}` declared in {} is not present in {}",
                toml_path.display(),
                lock_path.display(),
            );
        }
    }
}
