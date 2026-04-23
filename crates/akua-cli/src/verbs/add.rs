//! `akua add` — insert a dependency into `akua.toml`.
//!
//! Pure manifest-edit verb: parses the existing `akua.toml`, inserts a
//! [`Dependency`] keyed by `name`, writes the canonical TOML back. No
//! OCI fetch, no lockfile mutation — those arrive when the resolver
//! pipeline lands.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::lock_file::{AkuaLock, LockLoadError};
use akua_core::mod_file::{Dependency, ManifestError};
use akua_core::{
    chart_resolver, chart_resolver::ResolverOptions, AkuaManifest, ChartResolveError,
    ManifestLoadError,
};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct AddArgs<'a> {
    pub workspace: &'a Path,

    /// Local alias for the dep — the key in `[dependencies]` and the
    /// name `import` statements use.
    pub name: &'a str,

    pub source: AddSource<'a>,

    pub version: Option<&'a str>,

    pub tag: Option<&'a str>,

    pub rev: Option<&'a str>,

    /// Overwrite an existing entry under `name` instead of erroring.
    pub force: bool,
}

#[derive(Debug, Clone)]
pub enum AddSource<'a> {
    Oci(&'a str),
    Git(&'a str),
    Path(&'a str),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AddOutput {
    pub name: String,
    pub source: &'static str,

    /// The actual ref recorded in `akua.toml` (`oci://…`, the git URL,
    /// or the local path string).
    pub source_ref: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Whether an existing entry was replaced (only true with `--force`).
    pub replaced: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AddError {
    #[error(transparent)]
    Load(#[from] ManifestLoadError),

    #[error("dep `{name}` already declared in akua.toml; pass --force to replace")]
    Exists { name: String },

    #[error(transparent)]
    Validate(#[from] ManifestError),

    #[error("i/o error writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("resolving chart dep: {0}")]
    Resolve(#[from] ChartResolveError),

    #[error(transparent)]
    LockSave(#[from] LockLoadError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl AddError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            AddError::Load(e) => {
                let base = e.to_structured();
                if matches!(e, ManifestLoadError::Missing { .. }) {
                    base.with_suggestion("run `akua init` first")
                } else {
                    base
                }
            }
            AddError::Exists { .. } => {
                StructuredError::new(codes::E_ADD_DEP_EXISTS, self.to_string())
                    .with_suggestion("pass --force to replace the existing entry")
                    .with_default_docs()
            }
            AddError::Validate(e) => {
                StructuredError::new(codes::E_ADD_INVALID_DEP, e.to_string()).with_default_docs()
            }
            AddError::Io { path, source } => StructuredError::new(codes::E_IO, source.to_string())
                .with_path(path.display().to_string())
                .with_default_docs(),
            AddError::Resolve(inner) => {
                StructuredError::new(codes::E_CHART_RESOLVE, inner.to_string())
                    .with_default_docs()
            }
            AddError::LockSave(e) => e.to_structured(),
            AddError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            AddError::Load(e) if e.is_system() => ExitCode::SystemError,
            AddError::LockSave(e) if e.is_system() => ExitCode::SystemError,
            AddError::Io { .. } | AddError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

/// Did the resolver fail because the registry served a digest that
/// disagrees with `akua.lock`? That's a supply-chain-integrity signal
/// that must never be silently dropped.
#[cfg(feature = "oci-fetch")]
fn is_digest_drift(e: &ChartResolveError) -> bool {
    matches!(
        e,
        ChartResolveError::OciFetch {
            source: akua_core::oci_fetcher::OciFetchError::LockDigestMismatch { .. },
            ..
        }
    )
}

#[cfg(not(feature = "oci-fetch"))]
fn is_digest_drift(_e: &ChartResolveError) -> bool {
    false
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &AddArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, AddError> {
    let mut manifest = AkuaManifest::load(args.workspace)?;

    let already = manifest.dependencies.contains_key(args.name);
    if already && !args.force {
        return Err(AddError::Exists {
            name: args.name.to_string(),
        });
    }

    let dep = build_dependency(&args.source, args.version, args.tag, args.rev);
    dep.validate(args.name)?;

    manifest
        .dependencies
        .insert(args.name.to_string(), dep.clone());

    let serialized = manifest.to_toml()?;
    let manifest_path = args.workspace.join("akua.toml");
    std::fs::write(&manifest_path, serialized).map_err(|e| AddError::Io {
        path: manifest_path,
        source: e,
    })?;

    // `akua add` is the verb where OCI pulls + lockfile updates are
    // authorized — this is the Cargo/Go-modules "go get" semantic.
    // Path deps resolve locally, OCI deps pull over the network,
    // replace-overridden deps source from the local fork. Prior
    // lockfile digests flow into the resolver as `expected_digests`,
    // so a tag moving under us fails loudly instead of silently
    // re-pinning.
    let prior_lock = match AkuaLock::load(args.workspace) {
        Ok(l) => l,
        Err(LockLoadError::Missing { .. }) => AkuaLock::empty(),
        Err(e) => return Err(AddError::LockSave(e)),
    };
    let expected_digests = prior_lock
        .packages
        .iter()
        .filter(|p| p.is_oci())
        .map(|p| (p.name.clone(), p.digest.clone()))
        .collect();
    let opts = ResolverOptions {
        offline: false,
        cache_root: None,
        expected_digests,
        cosign_public_key_pem: None, // akua add surfaces cosign via render / verify
    };
    // Best-effort: a dep pointing at a path that doesn't exist yet
    // (common in "add now, write chart later" flows) shouldn't block
    // the manifest edit. Hard OCI failures propagate so the user sees
    // why the registry didn't accept the pull.
    match chart_resolver::resolve_with_options(&manifest, args.workspace, &opts) {
        Ok(resolved) => {
            let mut lock = prior_lock;
            chart_resolver::merge_into_lock(&mut lock, &resolved);
            lock.save(args.workspace)?;
        }
        // Security-critical: a lockfile digest mismatch means the
        // registry served different bytes than `akua.lock` pinned.
        // Silently continuing would mask a supply-chain incident.
        // Propagate up so the operator sees it at the verb that's
        // actually editing the lockfile.
        Err(e) if is_digest_drift(&e) => return Err(AddError::Resolve(e)),
        Err(_soft) => {
            // Soft-fail: manifest edit above stands. A missing path,
            // a pre-existing chart dir, an OCI auth 403 for a chart
            // we can't reach — none of these should undo the user's
            // declarative intent. `akua render` re-runs the resolver
            // strictly when output actually has to be produced.
        }
    }

    let output = AddOutput {
        name: args.name.to_string(),
        source: source_label(&args.source),
        source_ref: source_ref(&args.source).to_string(),
        version: args.version.map(str::to_string),
        replaced: already,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(AddError::StdoutWrite)?;

    Ok(ExitCode::Success)
}

fn build_dependency(
    source: &AddSource<'_>,
    version: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Dependency {
    let mut dep = Dependency {
        oci: None,
        git: None,
        path: None,
        version: version.map(str::to_string),
        tag: tag.map(str::to_string),
        rev: rev.map(str::to_string),
        replace: None,
    };
    match source {
        AddSource::Oci(s) => dep.oci = Some((*s).to_string()),
        AddSource::Git(s) => dep.git = Some((*s).to_string()),
        AddSource::Path(s) => dep.path = Some((*s).to_string()),
    }
    dep
}

fn source_label(source: &AddSource<'_>) -> &'static str {
    match source {
        AddSource::Oci(_) => "oci",
        AddSource::Git(_) => "git",
        AddSource::Path(_) => "path",
    }
}

fn source_ref<'a>(source: &'a AddSource<'a>) -> &'a str {
    match source {
        AddSource::Oci(s) | AddSource::Git(s) | AddSource::Path(s) => s,
    }
}

fn write_text<W: Write>(writer: &mut W, output: &AddOutput) -> std::io::Result<()> {
    let verb = if output.replaced { "replaced" } else { "added" };
    let version = output
        .version
        .as_deref()
        .map(|v| format!("@{v}"))
        .unwrap_or_default();
    writeln!(
        writer,
        "{verb} {}{} ({}: {})",
        output.name, version, output.source, output.source_ref
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const STARTER: &str = r#"
[package]
name    = "test"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
"#;

    fn workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("akua.toml"), STARTER).unwrap();
        tmp
    }

    fn args<'a>(ws: &'a Path, name: &'a str, source: AddSource<'a>) -> AddArgs<'a> {
        AddArgs {
            workspace: ws,
            name,
            source,
            version: None,
            tag: None,
            rev: None,
            force: false,
        }
    }

    #[test]
    fn adds_oci_dep_with_version() {
        let ws = workspace();
        let a = AddArgs {
            version: Some("1.2.3"),
            ..args(ws.path(), "cnpg", AddSource::Oci("oci://ghcr.io/x/y"))
        };
        run(&Context::human(), &a, &mut Vec::new()).expect("run");

        let after = AkuaManifest::load(ws.path()).expect("load");
        let dep = after.dependencies.get("cnpg").expect("present");
        assert_eq!(dep.oci.as_deref(), Some("oci://ghcr.io/x/y"));
        assert_eq!(dep.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn adds_git_dep_with_tag() {
        let ws = workspace();
        let a = AddArgs {
            tag: Some("v0.5.0"),
            ..args(ws.path(), "tooling", AddSource::Git("https://github.com/x/y"))
        };
        run(&Context::human(), &a, &mut Vec::new()).expect("run");

        let dep = AkuaManifest::load(ws.path()).unwrap().dependencies.remove("tooling").unwrap();
        assert_eq!(dep.git.as_deref(), Some("https://github.com/x/y"));
        assert_eq!(dep.tag.as_deref(), Some("v0.5.0"));
        assert!(dep.oci.is_none());
    }

    #[test]
    fn adds_path_dep() {
        let ws = workspace();
        let a = args(ws.path(), "local", AddSource::Path("../sibling"));
        run(&Context::human(), &a, &mut Vec::new()).expect("run");

        let dep = AkuaManifest::load(ws.path()).unwrap().dependencies.remove("local").unwrap();
        assert_eq!(dep.path.as_deref(), Some("../sibling"));
    }

    #[test]
    fn refuses_to_replace_without_force() {
        let ws = workspace();
        let first = AddArgs {
            version: Some("1.0.0"),
            ..args(ws.path(), "cnpg", AddSource::Oci("oci://a"))
        };
        run(&Context::human(), &first, &mut Vec::new()).expect("run");

        let second = AddArgs {
            version: Some("2.0.0"),
            ..args(ws.path(), "cnpg", AddSource::Oci("oci://b"))
        };
        let err = run(&Context::human(), &second, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, AddError::Exists { .. }));
        assert_eq!(err.to_structured().code, codes::E_ADD_DEP_EXISTS);
    }

    #[test]
    fn force_replaces_existing_entry_and_flags_replaced_in_output() {
        let ws = workspace();
        let first = AddArgs {
            version: Some("1.0.0"),
            ..args(ws.path(), "cnpg", AddSource::Oci("oci://a"))
        };
        run(&Context::human(), &first, &mut Vec::new()).expect("first");

        let second = AddArgs {
            version: Some("2.0.0"),
            force: true,
            ..args(ws.path(), "cnpg", AddSource::Oci("oci://b"))
        };
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &second, &mut stdout).expect("second");

        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["replaced"], true);
        let dep = AkuaManifest::load(ws.path()).unwrap().dependencies.remove("cnpg").unwrap();
        assert_eq!(dep.oci.as_deref(), Some("oci://b"));
        assert_eq!(dep.version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn missing_manifest_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let a = args(tmp.path(), "x", AddSource::Oci("oci://a"));
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_MANIFEST_MISSING);
    }

    #[test]
    fn invalid_dep_oci_without_version_surfaces_typed_error() {
        // The Dependency validator requires a version on OCI deps.
        let ws = workspace();
        let a = args(ws.path(), "broken", AddSource::Oci("oci://a"));
        let err = run(&Context::human(), &a, &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_ADD_INVALID_DEP);
    }

    #[test]
    fn round_trip_through_parser_preserves_existing_deps() {
        let ws = workspace();
        let a = AddArgs {
            version: Some("1.0.0"),
            ..args(ws.path(), "first", AddSource::Oci("oci://x"))
        };
        run(&Context::human(), &a, &mut Vec::new()).expect("run");

        let b = AddArgs {
            version: Some("2.0.0"),
            ..args(ws.path(), "second", AddSource::Oci("oci://y"))
        };
        run(&Context::human(), &b, &mut Vec::new()).expect("run");

        let after = AkuaManifest::load(ws.path()).unwrap();
        assert!(after.dependencies.contains_key("first"));
        assert!(after.dependencies.contains_key("second"));
    }
}
