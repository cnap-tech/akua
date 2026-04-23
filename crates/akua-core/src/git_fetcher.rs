//! Fetch Helm charts hosted in git repos into a local cache.
//!
//! A `git = "https://github.com/foo/bar"` dep in `akua.toml` is
//! resolved by:
//!
//! 1. Cloning the remote bare into `<cache>/repos/<sanitized-url>.git`
//!    on first sight; subsequent resolves are a no-op when the cached
//!    bare repo already has the requested ref.
//! 2. Resolving the pinned `tag` or `rev` to a concrete commit SHA.
//! 3. Materializing that commit into `<cache>/checkouts/<sha>/` via a
//!    linked worktree. Content-addressed so two deps at the same
//!    commit share disk.
//!
//! No shell-out — everything goes through [`gix`], a pure-Rust git
//! implementation. Sandbox posture in CLAUDE.md + security-model.md
//! stays intact.
//!
//! ## Scope
//!
//! - Public HTTPS + `file://` URLs. Public github / gitlab / gitea
//!   anonymous clones all work; `file://` covers tests and air-
//!   gapped mirrors.
//! - `ssh://` URLs intentionally not supported yet — needs an
//!   SSH agent integration beyond this slice. Users on that path
//!   can mirror to a local path or set up a self-hosted HTTPS
//!   endpoint.
//! - Private HTTPS auth (GitHub PAT etc.) — follow-up. The
//!   `oci_auth` credential store is reusable; wiring it into gix's
//!   transport is the work.

use std::path::{Path, PathBuf};

/// Reference specification — exactly one of tag or commit SHA.
/// Builds from the declarative `tag` / `rev` fields on a `Dependency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefSpec {
    Tag(String),
    Rev(String),
}

impl RefSpec {
    /// Human-readable label — the tag name or full SHA, whichever
    /// discriminates this spec. Used in error text + lockfile
    /// version fields.
    pub fn label(&self) -> &str {
        match self {
            RefSpec::Tag(s) | RefSpec::Rev(s) => s,
        }
    }
}

/// Result of a successful git fetch. `chart_dir` is the directory
/// the resolver hands to `helm-engine-wasm::render_dir` — same
/// contract as `OciFetcher::FetchedChart`.
#[derive(Debug, Clone)]
pub struct FetchedRepo {
    /// Absolute path to the checkout root (typically contains the
    /// chart directly). When the repo layout has the chart in a
    /// subdir, the caller post-joins the subdir path.
    pub chart_dir: PathBuf,

    /// Resolved commit SHA-1 (40-hex). This is the git-native
    /// identifier; the lockfile records it as the `digest` field
    /// prefixed `git:` so existing `sha256:` checks don't mistake
    /// it for a blob hash.
    pub commit_sha: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GitFetchError {
    #[error("invalid git URL `{0}`")]
    BadUrl(String),

    #[error("cloning `{url}`: {detail}")]
    Clone { url: String, detail: String },

    #[error("ref `{ref_name}` not found in `{url}`")]
    RefNotFound { url: String, ref_name: String },

    #[error("resolving `{ref_name}` in `{url}`: {detail}")]
    ResolveRef {
        url: String,
        ref_name: String,
        detail: String,
    },

    #[error("checking out `{sha}` into `{}`: {detail}", path.display())]
    Checkout {
        sha: String,
        path: PathBuf,
        detail: String,
    },

    #[error("i/o at `{}`: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("pulled commit `{actual}` doesn't match lockfile-pinned `{expected}` for `{url}@{ref_name}`")]
    LockCommitMismatch {
        url: String,
        ref_name: String,
        actual: String,
        expected: String,
    },
}

/// Fetch-and-checkout for a git-sourced chart dep.
///
/// `expected_commit` is the lockfile-pinned SHA (sans `git:` prefix)
/// when present. On mismatch we fail hard rather than silently
/// swallowing an upstream tag that was force-pushed to a different
/// commit.
pub fn fetch(
    url: &str,
    ref_spec: &RefSpec,
    cache_root: &Path,
    expected_commit: Option<&str>,
) -> Result<FetchedRepo, GitFetchError> {
    let bare_path = bare_repo_path(cache_root, url);

    // Fast path: bare repo already cloned → just resolve the ref.
    // Slow path: clone fresh.
    let bare = if bare_path.join("HEAD").is_file() {
        open_bare(&bare_path, url)?
    } else {
        clone_bare(url, &bare_path)?
    };

    let commit = resolve_ref(&bare, url, ref_spec)?;

    if let Some(expected) = expected_commit {
        if commit != expected {
            return Err(GitFetchError::LockCommitMismatch {
                url: url.to_string(),
                ref_name: ref_spec.label().to_string(),
                actual: commit,
                expected: expected.to_string(),
            });
        }
    }

    let checkout_dir = cache_root.join("checkouts").join(&commit);
    if !checkout_dir.join(".git-checkout-done").exists() {
        materialize_checkout(&bare, &commit, &checkout_dir, ref_spec)?;
    }

    Ok(FetchedRepo {
        chart_dir: checkout_dir,
        commit_sha: commit,
    })
}

/// Look up a cached checkout without touching the network. Returns
/// `Some` only when `<cache_root>/checkouts/<expected_commit>/` has
/// the `.git-checkout-done` sentinel — meaning a prior `fetch` call
/// completed fully. Partial state from an interrupted run is treated
/// as a cache miss.
pub fn fetch_from_cache(cache_root: &Path, expected_commit: &str) -> Option<FetchedRepo> {
    let checkout = cache_root.join("checkouts").join(expected_commit);
    if checkout.join(".git-checkout-done").exists() {
        Some(FetchedRepo {
            chart_dir: checkout,
            commit_sha: expected_commit.to_string(),
        })
    } else {
        None
    }
}

fn bare_repo_path(cache_root: &Path, url: &str) -> PathBuf {
    // Sanitize the URL into a filesystem-safe path that stays unique
    // across hosts. Not a sha — readable dirs help ops debug.
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(c),
            _ => out.push('_'),
        }
    }
    cache_root.join("repos").join(format!("{out}.git"))
}

fn clone_bare(url: &str, dest: &Path) -> Result<gix::Repository, GitFetchError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|source| GitFetchError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut prep = gix::prepare_clone_bare(url, dest).map_err(|e| GitFetchError::Clone {
        url: url.to_string(),
        detail: e.to_string(),
    })?;
    let (repo, _) = prep
        .fetch_only(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)
        .map_err(|e| GitFetchError::Clone {
            url: url.to_string(),
            detail: e.to_string(),
        })?;
    Ok(repo)
}

fn open_bare(path: &Path, url: &str) -> Result<gix::Repository, GitFetchError> {
    gix::open(path).map_err(|e| GitFetchError::Clone {
        url: url.to_string(),
        detail: format!("opening cached bare `{}`: {e}", path.display()),
    })
}

fn resolve_ref(
    repo: &gix::Repository,
    url: &str,
    ref_spec: &RefSpec,
) -> Result<String, GitFetchError> {
    match ref_spec {
        RefSpec::Rev(hex) => {
            // `rev` is a raw SHA-1 hex from akua.toml (`rev = "..."`).
            // Require a full 40-hex — abbreviations would need
            // revision-resolution, which is a heavier gix feature.
            // Package manifests pin exact SHAs anyway.
            let oid = gix::ObjectId::from_hex(hex.as_bytes()).map_err(|e| {
                GitFetchError::ResolveRef {
                    url: url.to_string(),
                    ref_name: hex.clone(),
                    detail: format!("rev must be a 40-char sha1 hex: {e}"),
                }
            })?;
            // Verify the object actually exists in the clone.
            repo.find_object(oid)
                .map_err(|_| GitFetchError::RefNotFound {
                    url: url.to_string(),
                    ref_name: hex.clone(),
                })?;
            Ok(oid.to_string())
        }
        RefSpec::Tag(tag) => {
            // Tags land under `refs/tags/<name>` in the bare clone.
            let fqn = format!("refs/tags/{tag}");
            let reference = repo.find_reference(&fqn).map_err(|_| {
                GitFetchError::RefNotFound {
                    url: url.to_string(),
                    ref_name: tag.clone(),
                }
            })?;
            // Peel to a commit — annotated tags point at a tag
            // object which itself points at the commit.
            let commit = reference.into_fully_peeled_id().map_err(|e| {
                GitFetchError::ResolveRef {
                    url: url.to_string(),
                    ref_name: tag.clone(),
                    detail: e.to_string(),
                }
            })?;
            Ok(commit.to_string())
        }
    }
}

fn materialize_checkout(
    repo: &gix::Repository,
    commit_sha: &str,
    dest: &Path,
    ref_spec: &RefSpec,
) -> Result<(), GitFetchError> {
    std::fs::create_dir_all(dest).map_err(|source| GitFetchError::Io {
        path: dest.to_path_buf(),
        source,
    })?;

    // Strategy: read the commit's tree and write it to disk via
    // gix-worktree-state. This avoids creating a full working index
    // and linked worktree, which is overkill for "give me the files
    // at this commit in a clean directory."
    let commit = repo
        .find_object(
            gix::ObjectId::from_hex(commit_sha.as_bytes()).map_err(|e| GitFetchError::Checkout {
                sha: commit_sha.to_string(),
                path: dest.to_path_buf(),
                detail: format!("hex parse: {e}"),
            })?,
        )
        .map_err(|e| GitFetchError::Checkout {
            sha: commit_sha.to_string(),
            path: dest.to_path_buf(),
            detail: e.to_string(),
        })?;
    let tree_id = commit
        .try_to_commit_ref()
        .map_err(|e| GitFetchError::Checkout {
            sha: commit_sha.to_string(),
            path: dest.to_path_buf(),
            detail: e.to_string(),
        })?
        .tree();

    write_tree(repo, tree_id, dest).map_err(|detail| GitFetchError::Checkout {
        sha: commit_sha.to_string(),
        path: dest.to_path_buf(),
        detail,
    })?;

    // Sentinel file so the cache-hit fast-path can tell "checkout
    // finished" from "partial checkout after a crash."
    std::fs::write(
        dest.join(".git-checkout-done"),
        format!(
            "akua git_fetcher v1\ncommit={commit_sha}\nref={}\n",
            ref_spec.label()
        ),
    )
    .map_err(|source| GitFetchError::Io {
        path: dest.to_path_buf(),
        source,
    })?;

    Ok(())
}

/// Walk a git tree and materialize every blob to disk. Simple
/// recursive traversal — no deltified tree walking, so bigger repos
/// take more I/O; fine for helm-chart-scale trees (~100 files).
fn write_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    dest: &Path,
) -> Result<(), String> {
    let tree = repo
        .find_object(tree_id)
        .map_err(|e| format!("find tree `{tree_id}`: {e}"))?;
    let tree = tree
        .try_into_tree()
        .map_err(|e| format!("object `{tree_id}` is not a tree: {e}"))?;
    let tree_ref = tree.decode().map_err(|e| format!("decode tree: {e}"))?;
    for entry in tree_ref.entries.iter() {
        let name = std::str::from_utf8(entry.filename)
            .map_err(|e| format!("non-utf8 filename in `{tree_id}`: {e}"))?;
        let target = dest.join(name);
        match entry.mode.kind() {
            gix::object::tree::EntryKind::Tree => {
                std::fs::create_dir_all(&target)
                    .map_err(|e| format!("mkdir {}: {e}", target.display()))?;
                write_tree(repo, entry.oid.into(), &target)?;
            }
            gix::object::tree::EntryKind::Blob
            | gix::object::tree::EntryKind::BlobExecutable => {
                let blob = repo
                    .find_object(entry.oid)
                    .map_err(|e| format!("find blob `{}`: {e}", entry.oid))?;
                std::fs::write(&target, &blob.data)
                    .map_err(|e| format!("write {}: {e}", target.display()))?;
            }
            // Symlinks and submodules: skipped. Helm charts don't use
            // either in practice, and both create sandbox-escape
            // surfaces we don't want to grant by default.
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — `file://` URL pointing at a local bare repo. No network.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bare repo at `path` containing a single commit with
    /// the given files. Returns the commit SHA. Uses gix's own object-
    /// creation APIs to avoid depending on a `git` binary on PATH
    /// (which would violate CLAUDE.md's no-shell-out anyway).
    fn make_bare_repo(path: &Path, files: &[(&str, &str)], tag: Option<&str>) -> String {
        let repo = gix::init_bare(path).expect("init bare");

        // Build a tree from the files.
        let mut tree = gix::objs::Tree::empty();
        for (name, content) in files {
            let blob_id = repo.write_blob(content.as_bytes()).expect("write blob").into();
            tree.entries.push(gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryMode::from(gix::objs::tree::EntryKind::Blob),
                filename: (*name).into(),
                oid: blob_id,
            });
        }
        tree.entries
            .sort_by(|a, b| a.filename.as_slice().cmp(b.filename.as_slice()));
        let tree_id = repo.write_object(&tree).expect("write tree").into();

        // Commit.
        use gix::actor::SignatureRef;
        let sig = SignatureRef {
            name: "akua-test".into(),
            email: "test@akua.dev".into(),
            time: gix::date::Time::new(1700000000, 0).into(),
        };
        let commit = gix::objs::Commit {
            tree: tree_id,
            parents: Default::default(),
            author: sig.into(),
            committer: sig.into(),
            encoding: None,
            message: "initial commit".into(),
            extra_headers: Vec::new(),
        };
        let commit_id = repo.write_object(&commit).expect("write commit");
        repo.reference(
            "refs/heads/main",
            commit_id,
            gix::refs::transaction::PreviousValue::Any,
            "initial",
        )
        .expect("set main");
        // Point HEAD at main (bare repos default to master otherwise
        // on older gix versions).
        repo.reference(
            "HEAD",
            commit_id,
            gix::refs::transaction::PreviousValue::Any,
            "HEAD",
        )
        .expect("set HEAD");

        if let Some(t) = tag {
            repo.reference(
                format!("refs/tags/{t}").as_str(),
                commit_id,
                gix::refs::transaction::PreviousValue::Any,
                "tag",
            )
            .expect("set tag");
        }

        commit_id.to_string()
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    #[test]
    fn ref_spec_label_returns_tag_or_rev() {
        assert_eq!(RefSpec::Tag("v1.0".into()).label(), "v1.0");
        assert_eq!(RefSpec::Rev("abcdef1234".into()).label(), "abcdef1234");
    }

    #[test]
    fn bare_repo_path_is_deterministic_and_sanitized() {
        let root = Path::new("/cache");
        let a = bare_repo_path(root, "https://github.com/foo/bar");
        let b = bare_repo_path(root, "https://github.com/foo/bar");
        assert_eq!(a, b);
        // Slashes and colons sanitized, but readable.
        assert!(a.to_string_lossy().contains("github.com"));
    }

    #[test]
    fn fetches_by_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let commit = make_bare_repo(
            &origin,
            &[
                ("Chart.yaml", "apiVersion: v2\nname: demo\nversion: 0.1.0\n"),
                ("values.yaml", "greeting: hi\n"),
            ],
            Some("v1.0.0"),
        );

        let cache = tmp.path().join("cache");
        let url = file_url(&origin);
        let fetched = fetch(&url, &RefSpec::Tag("v1.0.0".into()), &cache, None).expect("fetch");
        assert_eq!(fetched.commit_sha, commit);
        assert!(fetched.chart_dir.join("Chart.yaml").is_file());
        assert!(fetched.chart_dir.join("values.yaml").is_file());
    }

    #[test]
    fn fetches_by_rev() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let commit = make_bare_repo(
            &origin,
            &[("Chart.yaml", "apiVersion: v2\nname: demo\nversion: 0.1.0\n")],
            None,
        );

        let cache = tmp.path().join("cache");
        let url = file_url(&origin);
        let fetched = fetch(&url, &RefSpec::Rev(commit.clone()), &cache, None).expect("fetch");
        assert_eq!(fetched.commit_sha, commit);
    }

    #[test]
    fn second_fetch_is_cache_hit() {
        // Both calls should succeed; second must not re-clone (the
        // checkout dir already exists, we flag-file it).
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        make_bare_repo(
            &origin,
            &[("Chart.yaml", "apiVersion: v2\nname: demo\nversion: 0.1.0\n")],
            Some("v1.0"),
        );
        let cache = tmp.path().join("cache");
        let url = file_url(&origin);

        let a = fetch(&url, &RefSpec::Tag("v1.0".into()), &cache, None).expect("first");
        let b = fetch(&url, &RefSpec::Tag("v1.0".into()), &cache, None).expect("second");
        assert_eq!(a.chart_dir, b.chart_dir);
    }

    #[test]
    fn lock_commit_mismatch_fails_hard() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        make_bare_repo(
            &origin,
            &[("Chart.yaml", "apiVersion: v2\nname: demo\nversion: 0.1.0\n")],
            Some("v1.0"),
        );
        let cache = tmp.path().join("cache");
        let url = file_url(&origin);
        let expected =
            "0000000000000000000000000000000000000000"; // wrong SHA
        let err = fetch(
            &url,
            &RefSpec::Tag("v1.0".into()),
            &cache,
            Some(expected),
        )
        .unwrap_err();
        assert!(matches!(err, GitFetchError::LockCommitMismatch { .. }));
    }

    #[test]
    fn tag_not_found_surfaces_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        make_bare_repo(
            &origin,
            &[("Chart.yaml", "apiVersion: v2\nname: demo\nversion: 0.1.0\n")],
            Some("v1.0"),
        );
        let cache = tmp.path().join("cache");
        let url = file_url(&origin);
        let err = fetch(&url, &RefSpec::Tag("v999".into()), &cache, None).unwrap_err();
        assert!(matches!(err, GitFetchError::RefNotFound { .. }));
    }

    #[test]
    fn fetch_from_cache_returns_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(fetch_from_cache(
            tmp.path(),
            "0000000000000000000000000000000000000000",
        )
        .is_none());
    }
}
