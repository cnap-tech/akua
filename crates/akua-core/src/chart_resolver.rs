//! Resolve `charts.*` dependencies declared in `akua.toml` into typed
//! [`ResolvedChart`] values.
//!
//! A `Package.k` that writes
//!
//! ```kcl
//! import charts.nginx
//! ```
//!
//! is asking akua's loader to materialize a per-render KCL package named
//! `charts` whose `nginx.k` points at the on-disk chart directory the
//! `nginx` dep in `akua.toml` resolves to. That path + a content-addressed
//! digest of the chart tree is exactly what this module computes.
//!
//! Phase 2a resolved local-path deps. Phase 2b adds `replace` directives
//! + OCI/git remote fetches. Remote resolution without a `replace`
//! override still returns [`ChartResolveError::UnsupportedSource`] until
//! the OCI pull path lands (tracked in docs/roadmap.md Phase 2b slice B).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::mod_file::{AkuaManifest, Dependency, DependencySource};

/// A single resolved chart dep — a materialized on-disk directory plus
/// a content-addressed digest of the tree (filenames + contents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChart {
    /// Local alias in `akua.toml` / the `import charts.<name>` stem.
    pub name: String,

    /// Canonicalized absolute path on disk. Safe to hand to
    /// `helm-engine-wasm::render_dir` directly.
    pub abs_path: PathBuf,

    /// `sha256:<hex>` of the chart tree. Stable across machines when
    /// file contents + names are identical.
    pub sha256: String,

    /// Where the chart canonically comes from. Used by the lockfile
    /// writer to record the source of record even when a `replace`
    /// directive is pulling it off local disk.
    pub source: ResolvedSource,
}

/// The source of a resolved chart in a form suitable for lockfile
/// serialization. Distinct from `mod_file::DependencySource` because
/// it carries the concrete identifier (path / oci ref / git URL) and
/// the replace-override, not just the discriminant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSource {
    /// Bare path dep — `nginx = { path = "./vendor/nginx" }`. Lockfile
    /// writes `source = "path+file://<declared>"` with `version = "local"`.
    Path { declared: String },

    /// OCI-fetched dep. `blob_digest` is what the registry served us
    /// (sha256 of the helm chart layer); lockfile stores it under
    /// `digest`, subsequent pulls verify against it.
    Oci {
        oci: String,
        version: String,
        blob_digest: String,
    },

    /// OCI-sourced dep with a local fork override applied via
    /// `replace = { path = "..." }`. Lockfile still records the
    /// canonical `oci://…@version` — removing the replace just works.
    OciReplaced {
        oci: String,
        version: String,
        replace_path: String,
    },

    /// Git-sourced dep, fetched via `git_fetcher`. `commit_sha` is
    /// the resolved full 40-hex git SHA-1; lockfile stores it under
    /// `digest` with a `git:` prefix so it doesn't collide with OCI's
    /// `sha256:` scheme.
    Git {
        git: String,
        tag_or_rev: String,
        commit_sha: String,
    },

    /// Git-sourced dep with a `replace` override. Same semantics as
    /// `OciReplaced`.
    GitReplaced {
        git: String,
        /// Either the tag (preferred) or commit SHA.
        tag_or_rev: String,
        replace_path: String,
    },
}

impl ResolvedSource {
    /// Project into the triple `merge_into_lock` writes into every
    /// `LockedPackage`: canonical source string, version (or `"local"`
    /// for path deps), replace marker when a fork is active. Isolates
    /// the "how does this variant map to the lockfile" knowledge here
    /// instead of leaking it across every call site.
    pub fn to_locked_fields(&self) -> (String, String, Option<crate::lock_file::Replaced>) {
        use crate::lock_file::Replaced;
        match self {
            ResolvedSource::Path { declared } => (
                format!("path+file://{declared}"),
                "local".to_string(),
                None,
            ),
            ResolvedSource::Oci { oci, version, .. } => (oci.clone(), version.clone(), None),
            ResolvedSource::OciReplaced {
                oci,
                version,
                replace_path,
            } => (
                oci.clone(),
                version.clone(),
                Some(Replaced {
                    path: replace_path.clone(),
                }),
            ),
            ResolvedSource::Git {
                git,
                tag_or_rev,
                ..
            } => (
                format!("git+{git}@{tag_or_rev}"),
                tag_or_rev.clone(),
                None,
            ),
            ResolvedSource::GitReplaced {
                git,
                tag_or_rev,
                replace_path,
            } => (
                format!("git+{git}@{tag_or_rev}"),
                tag_or_rev.clone(),
                Some(Replaced {
                    path: replace_path.clone(),
                }),
            ),
        }
    }
}

/// The resolver's output. Canonical order (alphabetical by dep name)
/// so downstream users — `akua.lock` writers, `charts/` KCL module
/// generators — get deterministic iteration for free. Newtype (rather
/// than a bare `BTreeMap` alias) leaves room for future metadata —
/// resolution timestamps, manifest digest, replace-provenance — to
/// attach without breaking callers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedCharts {
    pub entries: BTreeMap<String, ResolvedChart>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChartResolveError {
    #[error("chart `{name}`: path-dep target `{}` does not exist", path.display())]
    NotFound { name: String, path: PathBuf },

    #[error("chart `{name}`: path-dep target `{}` is not a directory", path.display())]
    NotADirectory { name: String, path: PathBuf },

    #[error("chart `{name}`: i/o at `{}`: {source}", path.display())]
    Io {
        name: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Source isn't supported by this resolver path. Caller either
    /// passed [`ResolverOptions::offline = true`] or the source is
    /// still roadmap-pending (git).
    #[error("chart `{name}`: {} source not resolvable in the current mode — {reason}", source_kind_label(*kind))]
    UnsupportedSource {
        name: String,
        kind: DependencySource,
        /// Context for the user: which gate blocked resolution
        /// (offline mode, Phase 2b slice C git support, etc).
        reason: &'static str,
    },

    /// An `oci_fetcher::fetch` call failed. Kept as `OciFetchError` so
    /// the CLI layer can differentiate HTTP vs digest-mismatch.
    #[cfg(feature = "oci-fetch")]
    #[error("chart `{name}`: OCI fetch failed: {source}")]
    OciFetch {
        name: String,
        #[source]
        source: crate::oci_fetcher::OciFetchError,
    },

    /// A `git_fetcher::fetch` call failed.
    #[cfg(feature = "git-fetch")]
    #[error("chart `{name}`: git fetch failed: {source}")]
    GitFetch {
        name: String,
        #[source]
        source: crate::git_fetcher::GitFetchError,
    },
}

/// Resolve every dep in `manifest` against `workspace_root` and return
/// a [`ResolvedCharts`] suitable for threading into
/// `package_k::render_with_charts`.
///
/// `workspace_root` is the directory `akua.toml` sits in. Relative
/// path deps resolve against it.
/// Options controlling how much network the resolver is allowed to do
/// + where it caches fetched artifacts.
#[derive(Debug, Clone)]
pub struct ResolverOptions {
    /// When `true`, OCI deps return [`ChartResolveError::UnsupportedSource`]
    /// rather than attempting a network fetch. `akua verify` / tests
    /// turn this on to guarantee determinism without a network.
    pub offline: bool,

    /// Where OCI blobs live on disk. None → resolver uses the default
    /// cache (`$XDG_CACHE_HOME/akua/oci` or `$HOME/.cache/akua/oci`).
    pub cache_root: Option<PathBuf>,

    /// Lockfile-pinned digests keyed by dep name. When present, the
    /// fetcher verifies the pulled blob matches; a mismatch fails the
    /// resolve hard. Populated from `akua.lock` by the CLI.
    pub expected_digests: BTreeMap<String, String>,

    /// PEM-encoded cosign public key. When `Some`, every OCI dep
    /// pulls its `.sig` sidecar and verifies the signature before
    /// the chart is unpacked. Populated from
    /// `akua.toml [signing] cosign_public_key` by the CLI.
    pub cosign_public_key_pem: Option<String>,
}

impl Default for ResolverOptions {
    fn default() -> Self {
        Self {
            offline: false,
            cache_root: None,
            expected_digests: BTreeMap::new(),
            cosign_public_key_pem: None,
        }
    }
}

/// Offline resolve — path + replace only, OCI/git surface as
/// [`ChartResolveError::UnsupportedSource`]. The simplest callers
/// (tests, pure-local workflows) use this.
pub fn resolve(
    manifest: &AkuaManifest,
    workspace_root: &Path,
) -> Result<ResolvedCharts, ChartResolveError> {
    resolve_with_options(
        manifest,
        workspace_root,
        &ResolverOptions {
            offline: true,
            ..Default::default()
        },
    )
}

/// Full resolver — handles path, replace, and OCI (Phase 2b slice B).
/// Git deps still return [`ChartResolveError::UnsupportedSource`] —
/// Phase 2b slice C.
pub fn resolve_with_options(
    manifest: &AkuaManifest,
    workspace_root: &Path,
    opts: &ResolverOptions,
) -> Result<ResolvedCharts, ChartResolveError> {
    let mut entries = BTreeMap::new();
    for (name, dep) in &manifest.dependencies {
        // A `replace` directive on oci/git deps overrides resolution:
        // source-of-record stays the canonical oci/git (tracked in
        // lockfile for audit) but files come from the local fork.
        // Bare path deps don't use `replace` (rejected upstream by
        // manifest validation).
        if let Some(replace) = &dep.replace {
            let chart = resolve_path(
                name,
                &replace.path,
                workspace_root,
                resolved_source_for_replace(name, dep, &replace.path)?,
            )?;
            entries.insert(name.clone(), chart);
            continue;
        }
        // Match directly on the source fields rather than re-querying
        // `dep.source()` — pre-validated manifests are already guaranteed
        // to have exactly one set, and matching inline makes the
        // "path requires `path`" invariant unambiguous.
        match (&dep.path, &dep.oci, &dep.git) {
            (Some(path), None, None) => {
                let src = ResolvedSource::Path {
                    declared: path.clone(),
                };
                entries.insert(name.clone(), resolve_path(name, path, workspace_root, src)?);
            }
            (None, Some(oci), None) => {
                entries.insert(name.clone(), resolve_oci(name, dep, oci, opts)?);
            }
            (None, None, Some(git)) => {
                entries.insert(name.clone(), resolve_git(name, dep, git, opts)?);
            }
            _ => unreachable!("manifest validation rejects ambiguous / empty sources"),
        }
    }
    Ok(ResolvedCharts { entries })
}

/// Resolve an OCI-sourced dep: fetch (or retrieve from cache) via
/// [`crate::oci_fetcher`], capture the blob digest into
/// [`ResolvedSource::Oci`].
#[cfg(feature = "oci-fetch")]
fn resolve_oci(
    name: &str,
    dep: &Dependency,
    oci: &str,
    opts: &ResolverOptions,
) -> Result<ResolvedChart, ChartResolveError> {
    let version = dep
        .version
        .as_deref()
        .expect("manifest validation requires `version` on oci deps");
    let cache_root = opts
        .cache_root
        .clone()
        .unwrap_or_else(default_oci_cache_root);
    let expected = opts.expected_digests.get(name).map(String::as_str);

    let fetched = if opts.offline {
        // Air-gapped path: the resolver must not touch the network.
        // Require a lockfile-pinned digest + a populated cache
        // entry. Anything else is an operator error (run `akua add`
        // on a networked machine first).
        //
        // Note: offline mode bypasses cosign verification because
        // the `.sig` sidecar lives in the same registry and isn't
        // cached. If signing is required AND the cache was primed
        // online, cosign already approved the bytes — they can't
        // change between then and now without the digest check
        // catching it. So the invariant holds.
        let digest = expected.ok_or_else(|| ChartResolveError::UnsupportedSource {
            name: name.to_string(),
            kind: DependencySource::Oci,
            reason: "offline mode needs a lockfile-pinned digest — run `akua add` first",
        })?;
        crate::oci_fetcher::fetch_from_cache(&cache_root, digest).ok_or_else(|| {
            ChartResolveError::UnsupportedSource {
                name: name.to_string(),
                kind: DependencySource::Oci,
                reason: "offline mode and the OCI cache doesn't have this dep — run `akua add` online first",
            }
        })?
    } else {
        // Online path: consult creds + optional cosign key.
        let creds = crate::oci_auth::CredsStore::load().map_err(|source| {
            ChartResolveError::OciFetch {
                name: name.to_string(),
                source: crate::oci_fetcher::OciFetchError::AuthConfig {
                    detail: source.to_string(),
                },
            }
        })?;
        let fetch_opts = crate::oci_fetcher::FetchOpts {
            expected_digest: expected,
            creds: &creds,
            cosign_public_key_pem: opts.cosign_public_key_pem.as_deref(),
        };
        crate::oci_fetcher::fetch_with_opts(oci, version, &cache_root, &fetch_opts).map_err(
            |source| ChartResolveError::OciFetch {
                name: name.to_string(),
                source,
            },
        )?
    };

    // The content-addressed cache dir has a stable structure, so the
    // tree digest = the blob digest (same bytes just unpacked). Reuse
    // the blob digest rather than re-hashing the tree.
    Ok(ResolvedChart {
        name: name.to_string(),
        abs_path: fetched.chart_dir,
        sha256: fetched.blob_digest.clone(),
        source: ResolvedSource::Oci {
            oci: oci.to_string(),
            version: version.to_string(),
            blob_digest: fetched.blob_digest,
        },
    })
}

#[cfg(not(feature = "oci-fetch"))]
fn resolve_oci(
    name: &str,
    _dep: &Dependency,
    _oci: &str,
    _opts: &ResolverOptions,
) -> Result<ResolvedChart, ChartResolveError> {
    Err(ChartResolveError::UnsupportedSource {
        name: name.to_string(),
        kind: DependencySource::Oci,
        reason: "oci-fetch feature disabled at compile time",
    })
}

/// Resolve a git-sourced dep: clone + checkout via `git_fetcher`,
/// digest (commit SHA) written into [`ResolvedSource::Git`].
#[cfg(feature = "git-fetch")]
fn resolve_git(
    name: &str,
    dep: &Dependency,
    git: &str,
    opts: &ResolverOptions,
) -> Result<ResolvedChart, ChartResolveError> {
    use crate::git_fetcher::{self, RefSpec};

    // tag wins over rev when both are set (matches the replace-path
    // flow). Manifest validation guarantees at least one is present.
    let ref_spec = if let Some(tag) = dep.tag.as_deref() {
        RefSpec::Tag(tag.to_string())
    } else if let Some(rev) = dep.rev.as_deref() {
        RefSpec::Rev(rev.to_string())
    } else {
        unreachable!("manifest validation rejects git deps without tag or rev");
    };
    let tag_or_rev = ref_spec.label().to_string();

    let cache_root = opts
        .cache_root
        .clone()
        .unwrap_or_else(default_git_cache_root);

    // The lockfile stores the commit SHA under `digest` with a `git:`
    // prefix. Strip it back to raw hex for the fetcher; the prefix
    // is a lock-layer concern.
    let expected_commit = opts
        .expected_digests
        .get(name)
        .and_then(|d| d.strip_prefix(crate::lock_file::GIT_DIGEST_PREFIX).map(str::to_string));

    let fetched = if opts.offline {
        let expected = expected_commit.as_deref().ok_or_else(|| {
            ChartResolveError::UnsupportedSource {
                name: name.to_string(),
                kind: DependencySource::Git,
                reason: "offline mode needs a lockfile-pinned commit — run `akua add` first",
            }
        })?;
        git_fetcher::fetch_from_cache(&cache_root, expected).ok_or_else(|| {
            ChartResolveError::UnsupportedSource {
                name: name.to_string(),
                kind: DependencySource::Git,
                reason: "offline mode and the git cache doesn't have this commit — run `akua add` online first",
            }
        })?
    } else {
        git_fetcher::fetch(git, &ref_spec, &cache_root, expected_commit.as_deref()).map_err(
            |source| ChartResolveError::GitFetch {
                name: name.to_string(),
                source,
            },
        )?
    };

    // For git deps the lockfile digest is the commit SHA (git-native
    // content-address). Prefixed `git:` so it doesn't collide with
    // OCI `sha256:` in the digest-prefix validator.
    let digest = format!(
        "{}{}",
        crate::lock_file::GIT_DIGEST_PREFIX,
        fetched.commit_sha
    );
    Ok(ResolvedChart {
        name: name.to_string(),
        abs_path: fetched.chart_dir,
        sha256: digest,
        source: ResolvedSource::Git {
            git: git.to_string(),
            tag_or_rev,
            commit_sha: fetched.commit_sha,
        },
    })
}

#[cfg(not(feature = "git-fetch"))]
fn resolve_git(
    name: &str,
    _dep: &Dependency,
    _git: &str,
    _opts: &ResolverOptions,
) -> Result<ResolvedChart, ChartResolveError> {
    Err(ChartResolveError::UnsupportedSource {
        name: name.to_string(),
        kind: DependencySource::Git,
        reason: "git-fetch feature disabled at compile time",
    })
}

/// Default cache root: `$XDG_CACHE_HOME/akua/oci` with a fallback to
/// `$HOME/.cache/akua/oci`, and finally `./.akua/cache/oci` when both
/// env vars are absent (CI sandboxes, nix builds).
#[cfg(feature = "oci-fetch")]
fn default_oci_cache_root() -> PathBuf {
    default_cache_root("oci")
}

/// Default git cache root. Same resolver as OCI, different subdir
/// so the two caches don't mix.
#[cfg(feature = "git-fetch")]
fn default_git_cache_root() -> PathBuf {
    default_cache_root("git")
}

#[cfg(any(feature = "oci-fetch", feature = "git-fetch"))]
fn default_cache_root(subdir: &str) -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("akua").join(subdir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".cache/akua").join(subdir);
        }
    }
    PathBuf::from(".akua/cache").join(subdir)
}

/// Build the `ResolvedSource::*Replaced` variant from a dep that has
/// both a canonical source (`oci` / `git`) and a `replace.path`.
/// Rejects replace on bare path deps — that shape has no canonical
/// source to preserve.
fn resolved_source_for_replace(
    name: &str,
    dep: &Dependency,
    replace_path: &str,
) -> Result<ResolvedSource, ChartResolveError> {
    match (&dep.oci, &dep.git, &dep.path) {
        (Some(oci), None, None) => Ok(ResolvedSource::OciReplaced {
            oci: oci.clone(),
            version: dep
                .version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            replace_path: replace_path.to_string(),
        }),
        (None, Some(git), None) => {
            let tag_or_rev = dep
                .tag
                .clone()
                .or_else(|| dep.rev.clone())
                .unwrap_or_else(|| "HEAD".to_string());
            Ok(ResolvedSource::GitReplaced {
                git: git.clone(),
                tag_or_rev,
                replace_path: replace_path.to_string(),
            })
        }
        _ => unreachable!(
            "replace on a path-only or multi-source dep should have been rejected by manifest \
             validation (dep `{name}`)"
        ),
    }
}

/// Upsert every [`ResolvedChart`] into `lock` as a [`LockedPackage`].
/// Preserves prior `signature` / `attestation` when the entry already
/// existed so a merge doesn't silently drop cosign metadata a
/// follow-up `akua publish` has since populated.
pub fn merge_into_lock(lock: &mut crate::lock_file::AkuaLock, resolved: &ResolvedCharts) {
    use crate::lock_file::LockedPackage;
    for chart in resolved.entries.values() {
        let (source, version, replaced) = chart.source.to_locked_fields();
        let prior = lock.packages.iter().find(|p| p.name == chart.name);

        lock.upsert(LockedPackage {
            name: chart.name.clone(),
            version,
            source,
            digest: chart.sha256.clone(),
            // Cosign signatures / SLSA attestations are populated by
            // `akua publish` — preserve whatever the prior entry had
            // on an update.
            signature: prior.and_then(|p| p.signature.clone()),
            attestation: prior.and_then(|p| p.attestation.clone()),
            dependencies: prior.map(|p| p.dependencies.clone()).unwrap_or_default(),
            replaced,
            yanked: prior.and_then(|p| p.yanked),
            kyverno_source_digest: prior.and_then(|p| p.kyverno_source_digest.clone()),
            converter_version: prior.and_then(|p| p.converter_version.clone()),
        });
    }
    lock.sort();
}

/// Human-readable tag for a dep source. Used by `UnsupportedSource`'s
/// `#[error(...)]` template.
fn source_kind_label(source: DependencySource) -> &'static str {
    match source {
        DependencySource::Oci => "oci://",
        DependencySource::Git => "git",
        DependencySource::Path => "path",
    }
}

fn resolve_path(
    name: &str,
    requested: &str,
    workspace_root: &Path,
    source: ResolvedSource,
) -> Result<ResolvedChart, ChartResolveError> {
    let rel = PathBuf::from(requested);
    let joined = if rel.is_absolute() {
        rel
    } else {
        workspace_root.join(rel)
    };

    let canon = match joined.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ChartResolveError::NotFound {
                name: name.to_string(),
                path: joined,
            });
        }
        Err(e) => {
            return Err(ChartResolveError::Io {
                name: name.to_string(),
                path: joined,
                source: e,
            });
        }
    };

    let meta = canon.metadata().map_err(|e| ChartResolveError::Io {
        name: name.to_string(),
        path: canon.clone(),
        source: e,
    })?;
    if !meta.is_dir() {
        return Err(ChartResolveError::NotADirectory {
            name: name.to_string(),
            path: canon,
        });
    }

    let sha256 = hash_dir(&canon).map_err(|e| ChartResolveError::Io {
        name: name.to_string(),
        path: canon.clone(),
        source: e,
    })?;

    Ok(ResolvedChart {
        name: name.to_string(),
        abs_path: canon,
        sha256,
        source,
    })
}

/// Content-hash a directory tree. Walks files in sorted-by-relative-path
/// order so the digest is stable across filesystems (ext4 returns
/// `readdir` order; APFS returns arbitrary order). For each file the
/// hasher absorbs `<rel_path>\0<bytes>\n` — the NUL separator rules out
/// a "file A ends where file B's name begins" collision.
///
/// Symlinks are skipped entirely: their target lives outside the chart
/// dir, which breaks both determinism and the sandbox assumption that
/// the tarball we hand the engine has no escape hatches. An actual
/// chart needing a symlink is already broken on Windows hosts.
fn hash_dir(root: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, abs) in files {
        // Use `to_string_lossy` for cross-platform parity: Windows uses
        // UTF-16 OsStr internally; Unix is bytes. A chart with
        // non-UTF8-path filenames is broken regardless — collapsing
        // here doesn't create realistic collisions.
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        // Stream the file into the hasher so multi-MB values.yaml
        // or long template trees don't balloon memory. BufReader
        // + io::copy is the standard pattern.
        let file = std::fs::File::open(&abs)?;
        let mut reader = std::io::BufReader::new(file);
        std::io::copy(&mut reader, &mut hasher)?;
        hasher.update(b"\n");
    }
    Ok(format!("sha256:{}", hex_encode(&hasher.finalize())))
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let path = entry.path();
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walker stays under root")
                .to_path_buf();
            out.push((rel, path));
        }
        // Symlinks deliberately skipped — see `hash_dir` rationale.
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest(body: &str) -> AkuaManifest {
        let src = format!(
            r#"
[package]
name    = "test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
{body}
"#
        );
        AkuaManifest::parse(&src).expect("manifest parse")
    }

    /// Write a minimal chart tree (Chart.yaml + templates/cm.yaml) at
    /// `root`. Returns `root` for fluency in the callsite.
    fn write_minimal_chart(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join("templates")).unwrap();
        std::fs::write(
            root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(
            root.join("templates/cm.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo\n",
        )
        .unwrap();
        root.to_path_buf()
    }

    #[test]
    fn resolves_local_path_dep() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);

        let resolved = resolve(&manifest, ws.path()).expect("resolve");
        assert_eq!(resolved.entries.len(), 1);
        let nginx = resolved.entries.get("nginx").expect("nginx entry");
        assert_eq!(nginx.name, "nginx");
        assert!(nginx.abs_path.ends_with("charts/nginx"));
        assert!(nginx.abs_path.is_absolute());
        assert!(
            nginx.sha256.starts_with("sha256:"),
            "digest shape: {}",
            nginx.sha256
        );
        assert_eq!(nginx.sha256.len(), "sha256:".len() + 64);
    }

    #[test]
    fn digest_is_stable_across_calls() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);

        let a = resolve(&manifest, ws.path()).unwrap();
        let b = resolve(&manifest, ws.path()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn digest_changes_when_chart_contents_change() {
        let ws = tempfile::tempdir().unwrap();
        let chart = ws.path().join("charts/nginx");
        write_minimal_chart(&chart);
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);
        let before = resolve(&manifest, ws.path()).unwrap();

        // Mutate one template.
        std::fs::write(
            chart.join("templates/cm.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo2\n",
        )
        .unwrap();

        let after = resolve(&manifest, ws.path()).unwrap();
        assert_ne!(
            before.entries.get("nginx").unwrap().sha256,
            after.entries.get("nginx").unwrap().sha256
        );
    }

    #[test]
    fn digest_stable_across_file_creation_order() {
        // Create two charts with the same final contents but different
        // creation sequences — digest should match. Guards against
        // `readdir`-order flakiness on unsorted hashing.
        let ws_a = tempfile::tempdir().unwrap();
        let ws_b = tempfile::tempdir().unwrap();
        let chart_a = ws_a.path().join("c");
        let chart_b = ws_b.path().join("c");
        std::fs::create_dir_all(chart_a.join("templates")).unwrap();
        std::fs::create_dir_all(chart_b.join("templates")).unwrap();

        // A: Chart.yaml first, then cm.yaml
        std::fs::write(chart_a.join("Chart.yaml"), "v: 1\n").unwrap();
        std::fs::write(chart_a.join("templates/cm.yaml"), "body\n").unwrap();
        // B: cm.yaml first, then Chart.yaml
        std::fs::write(chart_b.join("templates/cm.yaml"), "body\n").unwrap();
        std::fs::write(chart_b.join("Chart.yaml"), "v: 1\n").unwrap();

        let mani_a = minimal_manifest(r#"x = { path = "./c" }"#);
        let mani_b = minimal_manifest(r#"x = { path = "./c" }"#);
        let a = resolve(&mani_a, ws_a.path()).unwrap();
        let b = resolve(&mani_b, ws_b.path()).unwrap();
        assert_eq!(
            a.entries.get("x").unwrap().sha256,
            b.entries.get("x").unwrap().sha256
        );
    }

    #[test]
    fn multiple_deps_alphabetical() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/zulu"));
        write_minimal_chart(&ws.path().join("charts/alpha"));
        let manifest = minimal_manifest(
            r#"
zulu  = { path = "./charts/zulu" }
alpha = { path = "./charts/alpha" }
"#,
        );
        let resolved = resolve(&manifest, ws.path()).unwrap();
        let names: Vec<&str> = resolved.entries.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["alpha", "zulu"], "BTreeMap iteration is sorted");
    }

    #[test]
    fn missing_path_dep_produces_typed_error() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(r#"ghost = { path = "./nope" }"#);
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::NotFound { ref name, .. } if name == "ghost"),
            "got: {err:?}"
        );
    }

    #[test]
    fn path_dep_pointing_at_a_file_is_rejected() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("not-a-chart.txt"), "hi").unwrap();
        let manifest = minimal_manifest(r#"bad = { path = "./not-a-chart.txt" }"#);
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::NotADirectory { ref name, .. } if name == "bad"),
            "got: {err:?}"
        );
    }

    #[test]
    fn oci_dep_surfaces_typed_error_in_offline_mode() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://ghcr.io/foo/nginx", version = "1.0.0" }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(
                err,
                ChartResolveError::UnsupportedSource {
                    ref name,
                    kind: DependencySource::Oci,
                    ..
                } if name == "nginx"
            ),
            "got: {err:?}"
        );
        // Reason text must mention offline so agents know which knob
        // to turn.
        assert!(err.to_string().contains("offline"), "got: {err}");
    }

    #[test]
    fn git_dep_surfaces_typed_error_in_offline_mode() {
        // Default `resolve()` is offline-mode. A git dep with no
        // lockfile-pinned commit can't be satisfied without the
        // network, so the resolver refuses — distinct from the
        // `GitFetch` error path (which requires a real clone).
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(
            r#"libs = { git = "https://github.com/foo/bar", tag = "v1.0.0" }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(
                err,
                ChartResolveError::UnsupportedSource {
                    ref name,
                    kind: DependencySource::Git,
                    ..
                } if name == "libs"
            ),
            "got: {err:?}"
        );
        assert!(err.to_string().contains("offline"), "got: {err}");
    }

    #[test]
    fn resolved_source_is_path_for_bare_path_dep() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);
        let resolved = resolve(&manifest, ws.path()).unwrap();
        let nginx = resolved.entries.get("nginx").unwrap();
        assert_eq!(
            nginx.source,
            ResolvedSource::Path {
                declared: "./charts/nginx".to_string()
            }
        );
    }

    #[test]
    fn oci_dep_with_replace_resolves_from_fork_path() {
        // Dev-workflow staple: pull chart source from local fork while
        // the lockfile still pins the canonical `oci://` digest.
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx-fork"));
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://r/n", version = "1.0.0", replace = { path = "./charts/nginx-fork" } }"#,
        );
        let resolved = resolve(&manifest, ws.path()).expect("replace should resolve");
        let nginx = resolved.entries.get("nginx").unwrap();
        assert!(nginx.abs_path.ends_with("charts/nginx-fork"));
        assert!(nginx.sha256.starts_with("sha256:"));
        match &nginx.source {
            ResolvedSource::OciReplaced {
                oci,
                version,
                replace_path,
            } => {
                assert_eq!(oci, "oci://r/n");
                assert_eq!(version, "1.0.0");
                assert_eq!(replace_path, "./charts/nginx-fork");
            }
            other => panic!("expected OciReplaced, got {other:?}"),
        }
    }

    #[test]
    fn git_dep_with_replace_carries_canonical_source_through() {
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/lib-fork"));
        let manifest = minimal_manifest(
            r#"libs = { git = "https://github.com/foo/bar", tag = "v1.2.3", replace = { path = "./charts/lib-fork" } }"#,
        );
        let resolved = resolve(&manifest, ws.path()).unwrap();
        match &resolved.entries.get("libs").unwrap().source {
            ResolvedSource::GitReplaced {
                git,
                tag_or_rev,
                replace_path,
            } => {
                assert_eq!(git, "https://github.com/foo/bar");
                assert_eq!(tag_or_rev, "v1.2.3");
                assert_eq!(replace_path, "./charts/lib-fork");
            }
            other => panic!("expected GitReplaced, got {other:?}"),
        }
    }

    #[test]
    fn git_dep_replace_prefers_tag_over_rev() {
        // When both tag and rev are set, tag wins in the lockfile source.
        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("fork"));
        let manifest = minimal_manifest(
            r#"libs = { git = "https://x/y", tag = "v1.0", rev = "abc123", replace = { path = "./fork" } }"#,
        );
        let resolved = resolve(&manifest, ws.path()).unwrap();
        match &resolved.entries.get("libs").unwrap().source {
            ResolvedSource::GitReplaced { tag_or_rev, .. } => {
                assert_eq!(tag_or_rev, "v1.0", "tag takes precedence over rev");
            }
            _ => panic!("expected GitReplaced"),
        }
    }

    #[test]
    fn replace_with_missing_path_still_surfaces_typed_error() {
        // Replace directive pointing at a non-existent path is a user
        // error — the resolver must report NotFound naming the dep.
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://r/n", version = "1.0.0", replace = { path = "./nope" } }"#,
        );
        let err = resolve(&manifest, ws.path()).unwrap_err();
        assert!(
            matches!(err, ChartResolveError::NotFound { ref name, .. } if name == "nginx"),
            "got: {err:?}"
        );
    }

    #[test]
    fn merge_into_lock_populates_path_deps() {
        use crate::lock_file::AkuaLock;

        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);
        let resolved = resolve(&manifest, ws.path()).unwrap();

        let mut lock = AkuaLock::empty();
        merge_into_lock(&mut lock, &resolved);

        assert_eq!(lock.packages.len(), 1);
        let entry = &lock.packages[0];
        assert_eq!(entry.name, "nginx");
        assert_eq!(entry.version, "local");
        assert_eq!(entry.source, "path+file://./charts/nginx");
        assert!(entry.digest.starts_with("sha256:"));
        assert!(entry.replaced.is_none());
    }

    #[test]
    fn merge_into_lock_records_replaced_canonical_source() {
        use crate::lock_file::AkuaLock;

        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx-fork"));
        let manifest = minimal_manifest(
            r#"nginx = { oci = "oci://r/n", version = "1.0.0", replace = { path = "./charts/nginx-fork" } }"#,
        );
        let resolved = resolve(&manifest, ws.path()).unwrap();

        let mut lock = AkuaLock::empty();
        merge_into_lock(&mut lock, &resolved);

        let entry = &lock.packages[0];
        assert_eq!(entry.source, "oci://r/n");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(
            entry.replaced.as_ref().map(|r| r.path.as_str()),
            Some("./charts/nginx-fork")
        );
    }

    #[test]
    fn merge_into_lock_preserves_signature_on_update() {
        use crate::lock_file::{AkuaLock, LockedPackage};

        let ws = tempfile::tempdir().unwrap();
        write_minimal_chart(&ws.path().join("charts/nginx"));
        let manifest = minimal_manifest(r#"nginx = { path = "./charts/nginx" }"#);

        let mut lock = AkuaLock::empty();
        lock.packages.push(LockedPackage {
            name: "nginx".to_string(),
            version: "local".to_string(),
            source: "path+file://./charts/nginx".to_string(),
            digest: "sha256:0000".to_string(), // stale digest
            signature: Some("cosign:sigstore:prior-publish".to_string()),
            dependencies: vec![],
            attestation: None,
            replaced: None,
            yanked: None,
            kyverno_source_digest: None,
            converter_version: None,
        });

        let resolved = resolve(&manifest, ws.path()).unwrap();
        merge_into_lock(&mut lock, &resolved);

        // Digest refreshed to the live chart; signature preserved.
        assert_ne!(lock.packages[0].digest, "sha256:0000");
        assert_eq!(
            lock.packages[0].signature.as_deref(),
            Some("cosign:sigstore:prior-publish")
        );
    }

    #[test]
    fn resolver_options_carry_cosign_public_key() {
        // Just the construction-site sanity check: nothing else tests
        // this field setter yet since the actual verify path needs a
        // live registry. Unit coverage for `verify_keyed` lives in
        // `cosign` and OCI wiring lives in `oci_fetcher`.
        let opts = ResolverOptions {
            cosign_public_key_pem: Some("-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----\n".into()),
            ..Default::default()
        };
        assert!(opts.cosign_public_key_pem.is_some());
    }

    #[test]
    fn empty_deps_returns_empty() {
        let ws = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest("");
        let resolved = resolve(&manifest, ws.path()).unwrap();
        assert!(resolved.entries.is_empty());
    }
}
