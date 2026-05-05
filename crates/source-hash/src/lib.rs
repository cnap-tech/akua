//! Stable content hash of source trees.
//!
//! Used to verify that the embedded `akua-render-worker.wasm` was built
//! from the same akua-core sources that `akua-cli` is currently being
//! compiled against. Mismatch = host/worker drift = release-quality bug.
//!
//! The producer (`task build:render-worker`) invokes the binary to write
//! the hash to a file alongside the .wasm. The consumer (akua-cli's
//! build.rs) imports this lib and recomputes; mismatch flips
//! `cargo:warning=` → `panic!` in release-like profiles.
//!
//! ## Protocol
//!
//! 1. Walk each `root` recursively. Skip dot-prefixed dirs (`.git`,
//!    `.cargo`). Keep only `*.rs` and `*.toml` files.
//! 2. Sort the resulting paths by their string under `workspace_root`
//!    (so the same source tree on different machines hashes the same).
//! 3. For each file, append `<sha256-of-content-hex>  <relpath>\n` to
//!    a buffer.
//! 4. SHA-256 the buffer; emit the hex digest.
//!
//! Mirrors `sha256sum file1 file2 ... | sha256sum` semantics so a
//! human can spot-check a result against the shell tools.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Compute the stable content hash of every `*.rs` / `*.toml` file
/// under any of `roots`. `workspace_root` strips the leading prefix
/// from each recorded path so the hash is portable across machines.
pub fn compute(roots: &[PathBuf], workspace_root: &Path) -> String {
    let mut files: Vec<PathBuf> = Vec::new();
    for root in roots {
        if root.is_file() {
            files.push(root.clone());
        } else if root.is_dir() {
            collect_files(root, &mut files);
        }
        // Missing roots are silently skipped — the producer + consumer
        // see the same set, so the hash is still consistent. Surfacing
        // the mismatch is build.rs's job.
    }

    // Sort by relpath string so the order is stable across machines
    // (different absolute prefixes shouldn't perturb the hash).
    let mut entries: Vec<(String, PathBuf)> = files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(workspace_root)
                .map(|r| r.to_path_buf())
                .unwrap_or_else(|_| p.clone());
            (rel.to_string_lossy().into_owned(), p)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut outer = Sha256::new();
    for (relpath, abspath) in &entries {
        let bytes = std::fs::read(abspath)
            .unwrap_or_else(|e| panic!("source-hash: read {}: {e}", abspath.display()));
        let inner_hex = hex_digest(&Sha256::digest(&bytes));
        outer.update(inner_hex.as_bytes());
        outer.update(b"  ");
        outer.update(relpath.as_bytes());
        outer.update(b"\n");
    }
    hex_digest(&outer.finalize())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip dot-prefixed (`.git`, `.cargo/`, editor swap files).
        if name_str.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rs") | Some("toml")
        ) {
            out.push(path);
        }
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(hex_char(b >> 4));
        s.push(hex_char(b & 0x0f));
    }
    s
}

fn hex_char(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + nibble - 10) as char,
        _ => unreachable!("nibble value out of range"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty input set hashes to the empty-buffer SHA-256 — a
    /// deterministic constant, surfaced here so the protocol's
    /// edge case is documented in code, not folklore.
    #[test]
    fn empty_input_hashes_to_canonical_empty_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let h = compute(&[], dir.path());
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Same files, different order in `roots` → same hash. Lockstep
    /// invariant: producer + consumer can list paths in any order.
    #[test]
    fn hash_is_order_invariant_across_roots() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.rs"), "// alpha\n").unwrap();
        std::fs::write(b.join("y.rs"), "// beta\n").unwrap();

        let h1 = compute(&[a.clone(), b.clone()], dir.path());
        let h2 = compute(&[b, a], dir.path());
        assert_eq!(h1, h2);
    }

    /// Editing a single byte in any tracked file changes the hash.
    /// The whole point of the producer/consumer protocol.
    #[test]
    fn content_change_flips_hash() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("file.rs");
        std::fs::write(&f, b"fn main() {}\n").unwrap();
        let h1 = compute(&[dir.path().to_path_buf()], dir.path());
        std::fs::write(&f, b"fn main() {} // edit\n").unwrap();
        let h2 = compute(&[dir.path().to_path_buf()], dir.path());
        assert_ne!(h1, h2);
    }

    /// `touch` (mtime change without content change) does NOT change
    /// the hash — this is the false-positive build.rs's mtime check
    /// suffers, fixed by switching to content hashing here.
    #[test]
    fn touch_without_edit_keeps_hash_stable() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("file.rs");
        std::fs::write(&f, b"fn main() {}\n").unwrap();
        let h1 = compute(&[dir.path().to_path_buf()], dir.path());

        // Re-write the same bytes — equivalent to a `touch` that
        // happens to also rewrite mtime (true touch can't be done
        // portably from Rust without `nix`/`filetime`).
        std::fs::write(&f, b"fn main() {}\n").unwrap();
        let h2 = compute(&[dir.path().to_path_buf()], dir.path());
        assert_eq!(h1, h2);
    }

    /// Non-`.rs` / non-`.toml` files (markdown, golden YAML, dotfiles)
    /// are out of scope — adding them would mean a lockfile change
    /// invalidates the worker, which isn't what we want.
    #[test]
    fn ignores_non_source_files_and_dot_prefixed_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), b"fn lib() {}\n").unwrap();
        let h1 = compute(std::slice::from_ref(&src), dir.path());

        // Adding a markdown file shouldn't move the hash.
        std::fs::write(src.join("README.md"), b"# noise\n").unwrap();
        // Nor should a hidden dir.
        std::fs::create_dir_all(src.join(".cargo")).unwrap();
        std::fs::write(src.join(".cargo/config.toml"), b"[noise]\n").unwrap();

        let h2 = compute(std::slice::from_ref(&src), dir.path());
        assert_eq!(h1, h2);
    }
}
