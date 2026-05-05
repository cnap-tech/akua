//! Build-time: AOT-compile `akua-render-worker.wasm` → `.cwasm` so the
//! per-render `Module::deserialize` at runtime is a memcpy, not a
//! multi-second Cranelift compile.
//!
//! The `.wasm` itself is produced by `task build:render-worker`
//! (cargo build against `wasm32-wasip1` in `crates/akua-render-worker/`).
//! This build.rs expects that artifact to already exist; it does NOT
//! recurse into cargo to build the worker. That keeps the build
//! topology cycle-free — akua-cli depends on the precompiled .cwasm,
//! the worker is built with its own cargo invocation.
//!
//! If the worker .wasm isn't present we emit a sentinel empty `.cwasm`
//! and a build-time warning. The runtime host checks for the zero-length
//! marker and surfaces `E_SANDBOX_UNAVAILABLE` so users get a clear
//! "run `task build:render-worker` first" message instead of a cryptic
//! wasmtime parse error.

use std::path::PathBuf;

// Delegates to the workspace-wide config helper. Build-time and
// runtime use byte-identical Cranelift settings (single source of
// truth), with the build-time variant additionally pinning Config
// to the cargo TARGET when cross-compiling — without that, the
// macos-aarch64 runner producing an x86_64-apple-darwin binary
// embeds an aarch64 cwasm and `Module::deserialize` traps at
// runtime.
fn worker_config() -> wasmtime::Config {
    engine_host_wasm::build_script_config()
}

fn main() {
    let worker_wasm = workspace_root().join("target/wasm32-wasip1/release/akua-render-worker.wasm");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let cwasm_out = out_dir.join("akua-render-worker.cwasm");
    let wasm_out = out_dir.join("akua-render-worker.wasm");

    println!("cargo:rerun-if-changed={}", worker_wasm.display());
    println!("cargo:rerun-if-changed=build.rs");

    // Watch the source trees that feed into akua-render-worker.wasm so
    // touching `eval_kcl` (or anything else the worker links) re-runs
    // this build script — cargo otherwise considers the artifact
    // up-to-date and silently keeps the stale `.cwasm`. The script can't
    // *rebuild* the worker (that's a separate cargo invocation against
    // wasm32-wasip1, run by `task build:render-worker`), but at least
    // the contributor sees the cargo:warning telling them to run it.
    let root = workspace_root();
    println!(
        "cargo:rerun-if-changed={}",
        root.join("crates/akua-render-worker/src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("crates/akua-core/src").display()
    );

    if !worker_wasm.exists() {
        // Hard-fail in release profiles. The previous behaviour
        // (empty sandbox + runtime E_SANDBOX_UNAVAILABLE) shipped
        // broken binaries through CI matrices that don't run
        // `task build:render-worker` — the symptom only surfaces on
        // first `akua render` post-install. Better to fail the build.
        //
        // dev / test profiles still get the empty-sandbox fallback
        // so contributors who haven't run `task build:render-worker`
        // yet aren't blocked from compiling akua-cli for unit tests
        // that don't exercise the worker.
        let profile = std::env::var("PROFILE").unwrap_or_default();
        let release_like = profile == "release"
            || profile == "ci-release"
            || std::env::var_os("CARGO_CFG_AKUA_REQUIRE_WORKER").is_some();
        if release_like {
            panic!(
                "akua-render-worker.wasm not found at {} — release profiles must ship a worker. \
                 Run `task build:render-worker` (or set AKUA_REQUIRE_WORKER=0 to override) before \
                 `cargo build -p akua-cli --release`.",
                worker_wasm.display()
            );
        }
        println!(
            "cargo:warning=akua-render-worker.wasm not found at {} — run `task build:render-worker` first. Emitting empty sandbox module (runtime will surface E_SANDBOX_UNAVAILABLE).",
            worker_wasm.display()
        );
        std::fs::write(&cwasm_out, []).expect("write empty cwasm marker");
        std::fs::write(&wasm_out, []).expect("write empty wasm marker");
        return;
    }

    // Freshness check: verify that the embedded worker .wasm was built
    // from the same akua-core sources akua-cli is currently compiling
    // against. Mismatch = host/worker drift = release-quality bug.
    //
    // Two signals, in order:
    //   1. Content hash via `<wasm>.sources.sha256` written by
    //      `task build:render-worker`. Authoritative when present —
    //      not affected by `git checkout`'s mtime resets.
    //   2. mtime fallback if the hash file is missing (e.g. someone ran
    //      `cargo build -p akua-render-worker --target wasm32-wasip1`
    //      directly, skipping the Taskfile).
    //
    // Dev profiles get a `cargo:warning=` (don't block unit-test
    // compilation). Release profiles — `release`, `ci-release`, or
    // `AKUA_REQUIRE_WORKER` set — hard-fail with the same gating the
    // missing-worker case uses below: a shipped binary whose worker
    // drifted is the same kind of bug as a binary with no worker.
    let source_roots = vec![
        root.join("crates/akua-render-worker/src"),
        root.join("crates/akua-render-worker/Cargo.toml"),
        root.join("crates/akua-core/src"),
        root.join("crates/akua-core/Cargo.toml"),
    ];
    let staleness = check_worker_freshness(&worker_wasm, &source_roots, &root);
    if let Some(reason) = staleness {
        let profile = std::env::var("PROFILE").unwrap_or_default();
        let release_like = profile == "release"
            || profile == "ci-release"
            || std::env::var_os("CARGO_CFG_AKUA_REQUIRE_WORKER").is_some();
        if release_like {
            panic!(
                "{reason}\nRelease profiles refuse stale workers because host + worker drift is \
                 a release-quality bug. Run `task build:render-worker` to rebuild."
            );
        }
        println!("cargo:warning={reason}");
    }

    let wasm = std::fs::read(&worker_wasm)
        .unwrap_or_else(|e| panic!("read worker wasm from {}: {e}", worker_wasm.display()));
    // Stage the source `.wasm` regardless; lib.rs picks one of the
    // two via cfg(feature = "precompile-engines").
    std::fs::write(&wasm_out, &wasm).expect("stage worker wasm");

    let precompile = std::env::var_os("CARGO_FEATURE_PRECOMPILE_ENGINES").is_some();
    if precompile {
        let engine =
            wasmtime::Engine::new(&worker_config()).expect("wasmtime::Engine::new(worker_config)");
        let cwasm = engine
            .precompile_module(&wasm)
            .expect("precompile_module failed");
        std::fs::write(&cwasm_out, cwasm).expect("write cwasm");
        println!(
            "cargo:warning=akua-render-worker.cwasm: {} bytes (AOT from {})",
            std::fs::metadata(&cwasm_out).map(|m| m.len()).unwrap_or(0),
            worker_wasm.display()
        );
    } else {
        std::fs::write(&cwasm_out, []).expect("write empty cwasm slot");
        println!(
            "cargo:warning=precompile-engines OFF — embedding source akua-render-worker.wasm ({} bytes), JIT at first render",
            wasm.len()
        );
    }
}

/// Verify the worker .wasm was built from the current source tree.
/// Returns `Some(reason)` describing the drift, or `None` when the
/// worker is fresh (or can't tell — best-effort signals only).
///
/// Authoritative path: a `<wasm>.sources.sha256` file written by
/// `task build:render-worker` records the SHA-256 of source content
/// at .wasm production time. We recompute the hash here; mismatch
/// means an akua-core/akua-render-worker source changed since the
/// .wasm was built.
///
/// Fallback path (no hash file): mtime comparison. Best-effort —
/// suffers a false positive after `git checkout` resets file mtimes
/// without changing content, which is exactly why the hash file
/// exists. CI workflows always go through the Taskfile; the
/// fallback only applies if a contributor invoked
/// `cargo build -p akua-render-worker --target wasm32-wasip1`
/// directly.
fn check_worker_freshness(
    worker_wasm: &std::path::Path,
    source_roots: &[PathBuf],
    workspace_root: &std::path::Path,
) -> Option<String> {
    // Append rather than replace the extension — the file lives next
    // to `<basename>.wasm` as `<basename>.wasm.sources.sha256`. Matches
    // how `task build:render-worker` writes it.
    // Append (not replace) the extension: `<basename>.wasm.sources.sha256`.
    // OsString::push avoids `Path::with_extension`'s replace-only semantics
    // and stays correct for non-UTF-8 paths (vs. format!("{}.foo")).
    let mut hash_file = worker_wasm.as_os_str().to_owned();
    hash_file.push(".sources.sha256");
    let hash_file = std::path::PathBuf::from(hash_file);
    if hash_file.exists() {
        let recorded = std::fs::read_to_string(&hash_file).ok()?;
        let recorded = recorded.trim();
        let actual = source_hash::compute(source_roots, workspace_root);
        if actual != recorded {
            return Some(format!(
                "akua-render-worker.wasm sources hash mismatch (recorded {}, current {}) — \
                 run `task build:render-worker` to rebuild it.",
                recorded, actual
            ));
        }
        return None;
    }
    // Fallback: mtime comparison. Catches the common "edit then
    // forgot to rebuild" case but suffers false positives after
    // git checkout. The hash file path above eliminates that.
    let target_mtime = std::fs::metadata(worker_wasm).ok()?.modified().ok()?;
    for root in source_roots {
        if let Some(stale) = walk_for_newer_source(root, target_mtime) {
            return Some(format!(
                "akua-render-worker.wasm is older than {} — run `task build:render-worker` to \
                 rebuild it before re-running cargo build.",
                stale.display()
            ));
        }
    }
    None
}

fn walk_for_newer_source(
    dir: &std::path::Path,
    target_mtime: std::time::SystemTime,
) -> Option<PathBuf> {
    if dir.is_file() {
        let meta = std::fs::metadata(dir).ok()?;
        if meta.modified().ok()? > target_mtime {
            return Some(dir.to_path_buf());
        }
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = entry.file_type().ok()?;
        if ft.is_dir() {
            if let Some(found) = walk_for_newer_source(&path, target_mtime) {
                return Some(found);
            }
        } else if source_hash::is_tracked_source(&path) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime > target_mtime {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

/// akua-cli's build.rs runs with CWD = crates/akua-cli. The workspace
/// root sits two parents up.
fn workspace_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/akua-cli parent chain")
        .to_path_buf()
}
