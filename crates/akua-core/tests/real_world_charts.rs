//! Regression coverage — render real charts end-to-end via the embedded
//! Helm engine + native OCI/HTTP fetcher.
//!
//! These tests pull live charts from public registries and render them.
//! They're slow + network-dependent so they're marked `#[ignore]` by default.
//!
//! Run locally:
//!
//! ```bash
//! task test:real-world-charts
//! # or:
//! cargo test -p akua-core --test real_world_charts -- --ignored --nocapture
//! ```
//!
//! CI runs them on a nightly schedule (separate workflow) so ordinary PRs
//! aren't gated on network availability.
//!
//! Adding a chart: append to `CHARTS`, run locally to confirm it works,
//! PR. If it stops rendering later, the test fails and we have a
//! regression signal.

#![cfg(feature = "helm-wasm")]

use akua_core::source::{HelmBlock, Source};
use akua_core::{build_umbrella_chart, render_umbrella_embedded, RenderOptions};

/// One test case: the minimum to construct a single-source umbrella pointing
/// at an external chart.
struct Chart {
    /// Human-friendly label used in test output.
    name: &'static str,
    /// Chart repository URL — OCI (`oci://…`) or HTTPS Helm repo.
    repo: &'static str,
    /// Chart name inside the repo. For OCI, this is the last path segment
    /// already implied by `repo`, so we duplicate here for the umbrella dep.
    chart: &'static str,
    /// Exact version to pin (keeps tests deterministic).
    version: &'static str,
}

/// Curated set of real-world charts to exercise the render pipeline.
/// Keep pinned to exact versions so the tests are reproducible.
const CHARTS: &[Chart] = &[
    // Already exercised by hello-package, but assert it stays working.
    Chart {
        name: "bitnami-nginx",
        repo: "https://charts.bitnami.com/bitnami",
        chart: "nginx",
        version: "18.1.0",
    },
    Chart {
        name: "bitnami-postgresql",
        repo: "https://charts.bitnami.com/bitnami",
        chart: "postgresql",
        version: "16.0.6",
    },
    Chart {
        name: "bitnami-redis",
        repo: "https://charts.bitnami.com/bitnami",
        chart: "redis",
        version: "20.1.0",
    },
    // OCI source — stefanprodan's podinfo, ships via GHCR. Exercises the
    // manifest-probe bearer-token flow in fetch.rs that compensates for
    // oci-client's `/v2/`-only auth probe.
    Chart {
        name: "podinfo-oci",
        repo: "oci://ghcr.io/stefanprodan/charts",
        chart: "podinfo",
        version: "6.7.1",
    },
];

fn umbrella_source(c: &Chart) -> Source {
    Source {
        name: c.name.to_string(),
        helm: Some(HelmBlock {
            repo: c.repo.to_string(),
            chart: Some(c.chart.to_string()),
            version: c.version.to_string(),
        }),
        kcl: None,
        helmfile: None,
        values: None,
    }
}

fn render_chart(c: &Chart) -> Result<String, String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let out = tmp
        .path()
        .canonicalize()
        .map_err(|e| format!("canonicalize: {e}"))?;
    let umbrella = build_umbrella_chart("akua-regression", "0.0.0", &[umbrella_source(c)])
        .map_err(|e| format!("build: {e}"))?;
    let opts = RenderOptions {
        helm_bin: "helm".into(),
        release_name: "regression".to_string(),
        namespace: "default".to_string(),
        override_values: None,
    };
    render_umbrella_embedded(&umbrella, &out, &opts).map_err(|e| format!("render: {e}"))
}

#[test]
#[ignore = "network + slow — run via `task test:real-world-charts`"]
fn render_curated_set() {
    let mut failures = Vec::new();

    for c in CHARTS {
        eprintln!("--- rendering {} {}:{} ---", c.name, c.chart, c.version);
        match render_chart(c) {
            Ok(yaml) => {
                if yaml.trim().is_empty() {
                    failures.push(format!(
                        "{}: render succeeded but produced empty output",
                        c.name
                    ));
                    continue;
                }
                // Minimum sanity: must contain at least one kind-like line.
                if !yaml.contains("kind:") {
                    failures.push(format!("{}: rendered output has no kind: line", c.name));
                    continue;
                }
                eprintln!("  OK ({} bytes)", yaml.len());
            }
            Err(e) => {
                failures.push(format!("{}: {}", c.name, e));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} of {} charts failed to render:\n{}",
            failures.len(),
            CHARTS.len(),
            failures.join("\n")
        );
    }
}

/// Per-chart test variants — isolated failures show up individually in
/// test output instead of being swallowed by the batch.
macro_rules! chart_test {
    ($fn_name:ident, $idx:expr) => {
        #[test]
        #[ignore = "network + slow — run via `task test:real-world-charts`"]
        fn $fn_name() {
            let c = &CHARTS[$idx];
            match render_chart(c) {
                Ok(yaml) => {
                    assert!(!yaml.trim().is_empty(), "empty output");
                    assert!(yaml.contains("kind:"), "no kind: line");
                }
                Err(e) => panic!("render failed: {}", e),
            }
        }
    };
}

chart_test!(render_bitnami_nginx, 0);
chart_test!(render_bitnami_postgresql, 1);
chart_test!(render_bitnami_redis, 2);
chart_test!(render_podinfo_oci, 3);
