//! Adversarial tests for the wasmtime render sandbox.
//!
//! CLAUDE.md's security invariant ("Sandboxed by default") is only
//! meaningful if each documented cap/guard actually fires on hostile
//! input. This suite walks the roadmap's adversarial list:
//!
//! - **memory-bomb** — allocations beyond `ResourceLimits.memory_bytes`
//!   trap, surfaced as `WorkerError::Trap`.
//! - **epoch-exhaustion** — runaway computation exceeds
//!   `ResourceLimits.epoch_deadline` and traps.
//! - **path-escape (absolute)** — plugin handlers reject absolute
//!   filesystem paths; message round-trips as `PluginPanic`.
//! - **path-escape (parent-relative)** — `../../` style escapes
//!   rejected at the guard, not the filesystem.
//! - **symlink-escape** — a symlink inside the Package dir that
//!   canonicalizes outside it still rejects.
//! - **import-escape (absolute)** — Packages can't `import` from an
//!   absolute path; KCL's external-pkg resolver refuses.
//!
//! Fuel-exhaustion is intentionally omitted: the shared wasmtime
//! Engine does not enable `Config::consume_fuel` for v0.1.0 (see
//! `render_worker.rs:47-53` for the rationale). Wall-clock via
//! epoch is the active CPU cap.
//!
//! Every test that calls a plugin handler registers a one-shot
//! `adversarial.<name>` handler on the host so we exercise the
//! guard + bridge pairing without dragging in helm/kustomize. Those
//! are covered separately by `sandbox_nested_wasmtime.rs`.

use std::path::Path;

use akua_cli::render_worker::{
    RenderHost, ResourceLimits, WorkerError, WorkerRequest, WorkerResponse,
};

fn enter_scope(package_dir: &Path) -> akua_core::kcl_plugin::RenderScope {
    akua_core::kcl_plugin::RenderScope::enter(&package_dir.join("package.k"))
}

fn run(source: &str, limits: ResourceLimits) -> Result<WorkerResponse, WorkerError> {
    let host = RenderHost::shared()?;
    host.invoke(
        &WorkerRequest::Render {
            package_filename: "package.k".into(),
            source: source.to_string(),
            inputs: None,
            charts_pkg_path: None,
            kcl_pkgs: std::collections::BTreeMap::new(),
        },
        limits,
    )
}

/// Install an `adversarial.resolve(path)` plugin that forwards to
/// [`resolve_in_package`], so path-guard tests can exercise the
/// guard + bridge pairing without registering identical handlers
/// per test. `register` is idempotent (last-write-wins per name)
/// and thread-safe — fine to call from every path-guard test.
fn register_resolve_plugin() {
    akua_core::kcl_plugin::register("adversarial.resolve", |args, _kwargs| {
        let path = args
            .get(0)
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing path arg".to_string())?;
        akua_core::kcl_plugin::resolve_in_package(Path::new(path))
            .map(|p| serde_json::Value::String(p.display().to_string()))
            .map_err(|e| e.to_string())
    });
}

#[test]
fn memory_cap_enforced_below_minimum_instance_size() {
    // Below the guest module's declared memory minimum (~40 pages =
    // 2.5 MiB) wasmtime refuses instantiation via the StoreLimiter.
    // That's the static half of the cap — proves the ceiling holds
    // at the earliest possible point.
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());

    let limits = ResourceLimits {
        memory_bytes: 1024 * 1024,
        ..ResourceLimits::default()
    };
    let err = run("resources = []\n", limits).expect_err("1 MiB cap must reject instantiation");
    assert!(
        matches!(err, WorkerError::Wasmtime(_)),
        "expected Wasmtime(...) at instantiate, got: {err:?}"
    );
}

#[test]
fn memory_cap_traps_runtime_growth_past_limit() {
    // Above the module minimum but below what the evaluator needs
    // to build a large data structure. Tests the runtime half of
    // the cap — `StoreLimiter` vetoes `memory.grow` calls after
    // instantiation.
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());

    let limits = ResourceLimits {
        memory_bytes: 8 * 1024 * 1024,
        ..ResourceLimits::default()
    };
    // ~10M u64s on the KCL heap ≈ well over 8 MiB once wrappers
    // are accounted for. Forces `memory.grow` past the cap.
    let source = "\
        _xs = [i for i in range(10000000)]\n\
        resources = []\n\
    ";
    let err = run(source, limits).expect_err("runtime growth must trap");
    assert!(
        matches!(err, WorkerError::Trap(_) | WorkerError::WorkerStderr(_)),
        "expected Trap/WorkerStderr for runtime memory cap, got: {err:?}"
    );
}

#[test]
fn epoch_cap_traps_runaway_evaluation() {
    // `epoch_deadline = 1` tick with the background ticker firing
    // every ~100ms leaves the guest effectively zero budget. Any
    // non-trivial KCL program exceeds it.
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());

    let limits = ResourceLimits {
        epoch_deadline: 1,
        ..ResourceLimits::default()
    };
    // A moderately expensive comprehension — in vanilla KCL the
    // interpreter cost is linear in the list length. Large enough
    // that 1 tick can't finish it.
    let source = "\
        _xs = [i for i in range(200000)]\n\
        resources = []\n\
    ";
    let err = run(source, limits).expect_err("epoch cap must trap");
    assert!(
        matches!(err, WorkerError::Trap(_)),
        "expected Trap for epoch cap, got: {err:?}"
    );
}

#[test]
fn absolute_plugin_path_rejected_at_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());
    register_resolve_plugin();

    let source = "\
        import kcl_plugin.adversarial\n\
        _ = adversarial.resolve(\"/etc/passwd\")\n\
        resources = []\n\
    ";
    let err = run(source, ResourceLimits::default()).expect_err("absolute path must be rejected");
    match err {
        WorkerError::PluginPanic(msg) => assert!(
            msg.contains("absolute") || msg.contains("Package-relative"),
            "expected absolute-rejected marker, got: {msg}"
        ),
        other => panic!("expected PluginPanic, got: {other:?}"),
    }
}

#[test]
fn parent_relative_plugin_path_rejected_at_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());
    register_resolve_plugin();

    let source = "\
        import kcl_plugin.adversarial\n\
        _ = adversarial.resolve(\"../../../etc\")\n\
        resources = []\n\
    ";
    let err = run(source, ResourceLimits::default()).expect_err("parent escape must be rejected");
    match err {
        WorkerError::PluginPanic(msg) => {
            assert!(msg.contains("escape"), "expected escape marker, got: {msg}")
        }
        other => panic!("expected PluginPanic, got: {other:?}"),
    }
}

#[test]
fn symlink_escape_rejected_through_canonicalize() {
    // The trap here is subtle: a path like `./link` looks
    // under-dir textually, but canonicalize resolves the symlink
    // and the guard checks the *resolved* path. Demonstrates the
    // guard isn't string-only.
    let tmp = tempfile::tempdir().unwrap();
    // Point the symlink OUTSIDE the package dir.
    let outside = tempfile::tempdir().unwrap();
    let link_path = tmp.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link_path).expect("symlink");
    #[cfg(not(unix))]
    {
        eprintln!("skipping symlink test on non-unix");
        return;
    }

    let _scope = enter_scope(tmp.path());
    register_resolve_plugin();

    let source = "\
        import kcl_plugin.adversarial\n\
        _ = adversarial.resolve(\"./link\")\n\
        resources = []\n\
    ";
    let err = run(source, ResourceLimits::default()).expect_err("symlink escape must be rejected");
    match err {
        WorkerError::PluginPanic(msg) => assert!(
            msg.contains("escape"),
            "expected escape marker from canonicalized symlink, got: {msg}"
        ),
        other => panic!("expected PluginPanic, got: {other:?}"),
    }
}

#[test]
fn absolute_import_path_rejected_by_kcl_resolver() {
    // KCL's parser / external-pkg resolver refuses to reach an
    // absolute path that isn't registered as an ExternalPkg —
    // Packages cannot leak into the host filesystem through
    // `import`. No preopens configured for `/etc`, and no
    // ExternalPkg named `etc`, so this must fail at KCL load.
    let tmp = tempfile::tempdir().unwrap();
    let _scope = enter_scope(tmp.path());

    let source = "\
        import /etc/passwd as evil\n\
        resources = []\n\
    ";
    let resp = run(source, ResourceLimits::default())
        .expect("KCL parse errors come back as Render { status: fail }");
    match resp {
        WorkerResponse::Render {
            status, message, ..
        } => {
            assert_ne!(status, "ok", "expected KCL to reject absolute import");
            assert!(
                !message.is_empty(),
                "expected a diagnostic message for absolute import"
            );
        }
        _ => panic!("expected Render"),
    }
}
