//! Integration test: pull a small public OCI Helm chart and verify
//! the cache layout.
//!
//! Skipped automatically when the network is unreachable or the target
//! registry is down — we never fail a build just because GitHub is
//! flaky. Enable more aggressively by setting `AKUA_OCI_IT=1` in CI
//! environments that guarantee network.

#![cfg(feature = "oci-fetch")]

use akua_core::oci_fetcher;

/// A small, stable, public helm OCI chart. `podinfo` is ~5 KB packed
/// and has been pinned at this tag for years.
const OCI_REF: &str = "oci://ghcr.io/stefanprodan/charts/podinfo";
const VERSION: &str = "6.6.0";

/// Treat connection-level failures (DNS, TLS, timeout) as "network
/// unavailable, skip" — genuine offline CI. A `Status` error means
/// the registry *did* respond; those are either a real bug (our ref
/// drifted, media type changed) or a deserved CI red. Don't swallow
/// them.
fn skip_if_network_flake(e: &oci_fetcher::OciFetchError) -> bool {
    matches!(
        e,
        oci_fetcher::OciFetchError::Transport(
            akua_core::oci_transport::TransportError::Http { .. }
        )
    )
}

#[test]
fn fetches_public_helm_oci_chart() {
    let cache = tempfile::tempdir().unwrap();
    let fetched = match oci_fetcher::fetch(OCI_REF, VERSION, cache.path(), None) {
        Ok(f) => f,
        Err(e) if skip_if_network_flake(&e) => {
            eprintln!("skipping: transient network error: {e}");
            return;
        }
        Err(oci_fetcher::OciFetchError::Transport(
            akua_core::oci_transport::TransportError::AuthRequired { registry },
        )) => {
            panic!("unexpected auth required on public chart for `{registry}`");
        }
        Err(e) => panic!("unexpected fetch error: {e}"),
    };

    assert!(fetched.chart_dir.join("Chart.yaml").is_file());
    assert!(fetched.blob_digest.starts_with("sha256:"));
    let chart = std::fs::read_to_string(fetched.chart_dir.join("Chart.yaml")).unwrap();
    assert!(chart.contains("name: podinfo"), "Chart.yaml: {chart}");

    // Second call must be a cache hit — no network, same digest.
    let cached =
        oci_fetcher::fetch(OCI_REF, VERSION, cache.path(), Some(&fetched.blob_digest)).unwrap();
    assert_eq!(cached.chart_dir, fetched.chart_dir);
    assert_eq!(cached.blob_digest, fetched.blob_digest);
}

#[test]
fn digest_mismatch_is_rejected() {
    let cache = tempfile::tempdir().unwrap();
    let bogus = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let err = match oci_fetcher::fetch(OCI_REF, VERSION, cache.path(), Some(bogus)) {
        Ok(_) => panic!("expected digest mismatch"),
        Err(e) => e,
    };
    match err {
        oci_fetcher::OciFetchError::LockDigestMismatch { .. } => {}
        e if skip_if_network_flake(&e) => {
            eprintln!("skipping: transient network error: {e}");
        }
        other => panic!("expected LockDigestMismatch, got: {other:?}"),
    }
}
