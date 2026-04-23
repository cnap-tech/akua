//! Shared workspace walker.
//!
//! `package_tar` (publish) + `test_runner` (akua test) both iterate
//! a workspace, skipping the same set of build-output / hidden /
//! package-manager directories. One walker, two predicates:
//!
//! - `should_skip_dir(name)` — true for any directory name that
//!   should never be recursed into. Load-bearing: render outputs,
//!   akua/git caches, dotfiles, language-ecosystem siblings.
//! - `keep_file(name)` — caller-supplied predicate applied per-file.
//!
//! Results are returned sorted by absolute path so downstream
//! consumers (tarball builder, test report) are byte-deterministic
//! across filesystems with different `readdir` orderings.

use std::path::{Path, PathBuf};

/// Walk `root` and return `(rel, abs)` pairs for every file the
/// per-file predicate accepts. Missing root is not an error — the
/// result is an empty vec. Symlinks are skipped: same reasoning as
/// the helm chart hash path (determinism + sandbox posture).
pub(crate) fn collect_files(
    root: &Path,
    keep_file: impl Fn(&str) -> bool,
) -> std::io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = Vec::new();
    walk(root, root, &keep_file, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk(
    root: &Path,
    dir: &Path,
    keep_file: &dyn Fn(&str) -> bool,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if should_skip_dir(&name_str) {
            continue;
        }

        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, keep_file, out)?;
        } else if ft.is_file() && keep_file(&name_str) {
            let rel = path
                .strip_prefix(root)
                .expect("walker stays under root")
                .to_path_buf();
            out.push((rel, path));
        }
        // Symlinks + other file types: deliberately skipped.
    }
    Ok(())
}

/// Directory names we never descend into. Catches build outputs,
/// akua-internal caches, VCS metadata, language-ecosystem siblings,
/// and all hidden dotfiles.
pub(crate) fn should_skip_dir(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name,
        "deploy" | "rendered" | "out" | "target" | "node_modules"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn skip_dir_rules() {
        assert!(should_skip_dir(".git"));
        assert!(should_skip_dir(".akua"));
        assert!(should_skip_dir("deploy"));
        assert!(should_skip_dir("target"));
        assert!(should_skip_dir("node_modules"));
        assert!(!should_skip_dir("src"));
        assert!(!should_skip_dir("vendor"));
    }

    #[test]
    fn collect_files_applies_predicate_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "b.k", b"b\n");
        write(tmp.path(), "a.k", b"a\n");
        write(tmp.path(), "c.txt", b"c\n");
        write(tmp.path(), "nested/d.k", b"d\n");
        write(tmp.path(), ".hidden/skip.k", b"skip\n");
        write(tmp.path(), "target/skip.k", b"skip\n");

        let kept = collect_files(tmp.path(), |n| n.ends_with(".k")).unwrap();
        let rels: Vec<String> = kept
            .iter()
            .map(|(r, _)| r.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rels,
            vec!["a.k".to_string(), "b.k".to_string(), "nested/d.k".to_string()]
        );
    }

    #[test]
    fn missing_root_returns_empty() {
        let kept = collect_files(Path::new("/no/such/dir"), |_| true).unwrap();
        assert!(kept.is_empty());
    }
}
