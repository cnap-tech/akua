//! `akua cache` — list, clear, or locate the content-addressed caches
//! that `akua add` + `akua render` populate on disk.
//!
//! Subverbs:
//! - `akua cache list` — enumerate OCI blobs + git repos/checkouts
//!   under `$XDG_CACHE_HOME/akua/{oci,git}` with sizes.
//! - `akua cache clear [--oci | --git]` — reclaim disk. Default wipes
//!   both; flags narrow it. Safe on absent caches (no-op).
//! - `akua cache path` — print the resolved cache roots. Useful for
//!   scripting `du -sh` / mount-point pinning on CI runners.
//!
//! Why this exists: ephemeral CI runners + self-hosted agents share
//! disk across tenants. "How big is the akua cache?" and "nuke the
//! cache" need deterministic tooling — not `rm -rf` guessing at the
//! layout. This verb is the tooling.

use std::io::Write;
use std::path::PathBuf;

use akua_core::cache_inventory::{self, CacheEntry, CacheInventory, ClearScope};
use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub enum CacheAction {
    List,
    Clear { scope: ClearScope },
    Path,
}

#[derive(Debug, Clone)]
pub struct CacheArgs {
    pub action: CacheAction,
}

/// Stable JSON shape per subverb. Discriminated by `action` so
/// consumers can parse a single shape and branch.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CacheOutput {
    List(CacheInventory),
    Clear(ClearOutputBody),
    Path(PathOutputBody),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClearOutputBody {
    pub scope: &'static str,
    pub oci_root: PathBuf,
    pub git_root: PathBuf,
    pub removed: usize,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PathOutputBody {
    pub oci_root: PathBuf,
    pub git_root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheVerbError {
    #[error(transparent)]
    Cache(#[from] cache_inventory::CacheError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl CacheVerbError {
    pub fn to_structured(&self) -> StructuredError {
        StructuredError::new(codes::E_IO, self.to_string()).with_default_docs()
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            CacheVerbError::StdoutWrite(_) => ExitCode::SystemError,
            CacheVerbError::Cache(_) => ExitCode::SystemError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &CacheArgs,
    stdout: &mut W,
) -> Result<ExitCode, CacheVerbError> {
    let oci_root = cache_inventory::default_cache_root("oci");
    let git_root = cache_inventory::default_cache_root("git");

    let output = match args.action {
        CacheAction::List => {
            let inv = cache_inventory::list()?;
            CacheOutput::List(inv)
        }
        CacheAction::Clear { scope } => {
            let report = cache_inventory::clear(scope)?;
            CacheOutput::Clear(ClearOutputBody {
                scope: scope.as_str(),
                oci_root: oci_root.clone(),
                git_root: git_root.clone(),
                removed: report.removed,
                freed_bytes: report.freed_bytes,
            })
        }
        CacheAction::Path => CacheOutput::Path(PathOutputBody {
            oci_root: oci_root.clone(),
            git_root: git_root.clone(),
        }),
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(CacheVerbError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn write_text<W: Write>(w: &mut W, output: &CacheOutput) -> std::io::Result<()> {
    match output {
        CacheOutput::List(inv) => write_list(w, inv),
        CacheOutput::Clear(body) => {
            writeln!(
                w,
                "cleared {} cache: {} entries, {} freed",
                body.scope,
                body.removed,
                human_bytes(body.freed_bytes)
            )
        }
        CacheOutput::Path(body) => {
            writeln!(w, "oci: {}", body.oci_root.display())?;
            writeln!(w, "git: {}", body.git_root.display())
        }
    }
}

fn write_list<W: Write>(w: &mut W, inv: &CacheInventory) -> std::io::Result<()> {
    if inv.entries.is_empty() {
        writeln!(w, "no cache entries")?;
        writeln!(w, "oci: {}", inv.oci_root.display())?;
        writeln!(w, "git: {}", inv.git_root.display())?;
        return Ok(());
    }
    writeln!(
        w,
        "{} entries, {} total",
        inv.entries.len(),
        human_bytes(inv.total_bytes)
    )?;
    for e in &inv.entries {
        write_entry(w, e)?;
    }
    Ok(())
}

fn write_entry<W: Write>(w: &mut W, e: &CacheEntry) -> std::io::Result<()> {
    writeln!(
        w,
        "  [{kind}] {id}  {size}  {path}",
        kind = e.kind,
        id = e.id,
        size = human_bytes(e.size_bytes),
        path = e.path.display(),
    )
}

/// Compact binary-prefix size rendering — `1.2 MiB`, `540 B`. No
/// external dep; this is the only place we need it. Good enough for
/// operator-readable output.
fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", n)
    } else {
        format!("{value:.1} {}", UNITS[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::args::UniversalArgs;

    fn ctx_json() -> Context {
        let args = UniversalArgs {
            json: true,
            ..UniversalArgs::default()
        };
        Context::resolve(&args, akua_core::cli_contract::AgentContext::none())
    }

    #[test]
    fn path_subverb_emits_both_roots() {
        let ctx = ctx_json();
        let mut buf = Vec::new();
        let code = run(
            &ctx,
            &CacheArgs {
                action: CacheAction::Path,
            },
            &mut buf,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        let parsed: serde_json::Value = serde_json::from_slice(&buf).expect("json");
        assert_eq!(parsed["action"], "path");
        assert!(parsed["oci_root"].as_str().unwrap().ends_with("oci"));
        assert!(parsed["git_root"].as_str().unwrap().ends_with("git"));
    }

    #[test]
    fn list_subverb_produces_stable_json_shape() {
        let ctx = ctx_json();
        let mut buf = Vec::new();
        run(
            &ctx,
            &CacheArgs {
                action: CacheAction::List,
            },
            &mut buf,
        )
        .expect("run");
        let parsed: serde_json::Value = serde_json::from_slice(&buf).expect("json");
        assert_eq!(parsed["action"], "list");
        assert!(parsed["oci_root"].is_string());
        assert!(parsed["git_root"].is_string());
        assert!(parsed["entries"].is_array());
        assert!(parsed["total_bytes"].is_number());
    }

    #[test]
    fn human_bytes_picks_smallest_unit() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }
}
