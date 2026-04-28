//! Verification: does the current nested-wasmtime architecture actually
//! work? KCL inside the render-worker Store (Engine A) calls
//! `helm.template(...)`. That callout goes through the plugin bridge
//! into akua-core's registered helm handler, which invokes
//! `helm_engine_wasm::render_dir(...)` — a separate wasmtime
//! Engine (Engine B) via the `engine-host-wasm` crate.
//!
//! Two wasmtime Engines, two Stores, both live on the same OS thread
//! at once. Wasmtime's own docs recommend one Engine + many Stores
//! (see `docs/spikes/wasmtime-multi-engine.md` — research in
//! progress); this test exists to establish the empirical baseline
//! before we refactor. If this passes, the two-Engine path functions
//! today even if we plan to unify later.
//!
//! Marked `#[ignore]` by default — it's relatively expensive (spins
//! up helm inside the second wasmtime) and needs the pre-built
//! helm engine-wasm artefact. Run explicitly with:
//!
//!     task build:render-worker
//!     task build:helm-engine-wasm
//!     cargo test -p akua-cli --test sandbox_nested_wasmtime -- --include-ignored

#![cfg(all(feature = "cosign-verify", feature = "dev-watch"))]

use std::path::PathBuf;

use akua_cli::render_worker::{RenderHost, ResourceLimits, WorkerRequest, WorkerResponse};

/// Chart dir path inside the repo — used as `helm.template` input.
fn chart_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("examples/00-helm-hello/chart")
}

#[test]
#[ignore = "sanity: does helm render directly (no bridge/worker)?"]
fn helm_render_direct_no_sandbox() {
    let chart = chart_dir();
    assert!(chart.is_dir());

    let release = helm_engine_wasm::Release {
        name: "hello".into(),
        namespace: "default".into(),
        revision: 1,
        service: "Helm".into(),
    };
    let values = "greeting: direct\n";
    let manifests =
        helm_engine_wasm::render_dir(&chart, "hello", values, &release).expect("helm direct");
    assert!(!manifests.is_empty(), "helm produced no manifests");
    eprintln!("[direct-helm] produced {} manifests", manifests.len());
}

/// Canary — currently FAILS because KCL's parser at
/// `kcl-parser/src/lib.rs:668` uses `Path::exists()` to probe
/// `/charts/kcl.mod` on the preopen, and Rust's wasip1 std doesn't
/// resolve absolute paths through preopens for metadata calls.
/// Preopen wiring + protocol are correct; the failure surfaces as
/// `pkgroot not found: "charts.nginx"` from KCL itself. Drop the
/// `#[ignore]` once the wasip1 absolute-path-through-preopen gap is
/// resolved upstream.
#[test]
fn charts_import_resolves_inside_sandbox_via_preopen() {
    // Narrow scope: verify that an absolute preopen path (`/charts`)
    // resolves `import charts.<name>` inside the wasmtime sandbox.
    // Decoupled from `materialize_charts` on purpose — the generated
    // `<name>.k` imports `akua.helm`, which needs the akua stdlib
    // preopened too (tracked separately). The mechanics of preopen
    // resolution are what this test proves.

    let temp = tempfile::Builder::new()
        .prefix("akua-charts-preopen-")
        .tempdir()
        .expect("charts tempdir");

    // Minimal `charts` pkg: kcl.mod + a single-file `nginx.k` exposing
    // a `marker` binding we can assert on.
    std::fs::write(
        temp.path().join("kcl.mod"),
        "[package]\nname = \"charts\"\nedition = \"0.0.1\"\nversion = \"0.0.1\"\n",
    )
    .expect("write kcl.mod");
    let marker = "preopen-reached-nginx";
    std::fs::write(
        temp.path().join("nginx.k"),
        format!("marker: str = \"{marker}\"\n"),
    )
    .expect("write nginx.k");

    let Ok(host) = RenderHost::new() else {
        eprintln!("skipping: worker .wasm not built");
        return;
    };

    let src = "import charts.nginx\n\
               chart_marker = nginx.marker\n"
        .to_string();

    let resp = host
        .invoke_with_charts(
            &WorkerRequest::Render {
                package_filename: "package.k".into(),
                source: src,
                inputs: None,
                charts_pkg_path: Some("/charts".into()),
                kcl_pkgs: std::collections::BTreeMap::new(),
            },
            ResourceLimits {
                epoch_deadline: 300,
                ..ResourceLimits::default()
            },
            temp.path(),
        )
        .expect("invoke_with_charts");
    match resp {
        WorkerResponse::Render {
            status,
            yaml,
            message,
            ..
        } => {
            assert_eq!(status, "ok", "diagnostic: {message}");
            assert!(
                yaml.contains(marker),
                "marker should round-trip through preopened import: {yaml}"
            );
        }
        _ => panic!("expected Render"),
    }
}

#[test]
#[ignore = "expensive: requires helm engine-wasm + 2 wasmtime Engines"]
fn helm_template_through_plugin_bridge_across_engines() {
    // Sanity: fixture exists.
    let chart = chart_dir();
    assert!(chart.is_dir(), "missing chart fixture: {}", chart.display());
    eprintln!("[test] chart dir: {}", chart.display());
    eprintln!(
        "[test] chart.parent(): {}",
        chart.parent().unwrap().display()
    );

    // RenderScope wants the path to a `Package.k` file, not the
    // package dir — `current_package_dir()` returns `file.parent()`.
    // Point at the real example Package so the resolver gets the
    // right dir.
    let package_k = chart.parent().unwrap().join("package.k");
    let _scope = akua_core::kcl_plugin::RenderScope::enter(&package_k);

    let Ok(host) = RenderHost::new() else {
        eprintln!("skipping: worker .wasm not built (task build:render-worker)");
        return;
    };

    // KCL source invokes the helm plugin via the raw `kcl_plugin.<pkg>`
    // discovery shape so we bypass the akua.helm stdlib wrapper (the
    // worker's wasm32 build skips the akua stdlib ExternalPkg —
    // tracked separately in the spike writeup).
    // Relative path resolves under the RenderScope's package dir
    // (which is `chart.parent()` above — i.e. examples/00-helm-hello/).
    // Mirrors what the 00-helm-hello Package itself does.
    let src = "import kcl_plugin.helm\n\
         _manifests = helm.template({\"chart\": \"./chart\", \"values\": {\"greeting\": \"nested wasmtime works\"}, \"release\": \"hello\", \"namespace\": \"default\"})\n\
         resources = _manifests\n".to_string();

    // Cold helm init (instantiate helm.wasm + Go `_initialize`)
    // consistently takes 1-2s on first run. Default 3s deadline
    // leaves zero margin — bump to 30s so this test isolates the
    // nested-wasmtime question, not the deadline setting.
    let limits = ResourceLimits {
        epoch_deadline: 300,
        ..ResourceLimits::default()
    };
    let resp = host
        .invoke(
            &WorkerRequest::Render {
                package_filename: "package.k".into(),
                source: src,
                inputs: None,
                charts_pkg_path: None,
                kcl_pkgs: std::collections::BTreeMap::new(),
            },
            limits,
        )
        .expect("worker invoke");

    match resp {
        WorkerResponse::Render {
            status,
            yaml,
            message,
            ..
        } => {
            // If this comes back `fail`, message holds the diagnostic —
            // extract it for the failure report rather than asserting
            // a substring.
            assert_eq!(
                status, "ok",
                "helm through nested wasmtime failed: {message}"
            );
            // A minimal shape check — the helm chart under
            // 00-helm-hello renders a ConfigMap whose `data.greeting`
            // field echoes the input. If the bridge works end-to-end
            // across the two Engines the marker string round-trips.
            assert!(
                yaml.contains("nested wasmtime works") || yaml.contains("ConfigMap"),
                "expected helm output to contain the marker or kind ConfigMap — got:\n{yaml}"
            );
        }
        WorkerResponse::Ping { .. } => panic!("expected Render"),
    }
}
