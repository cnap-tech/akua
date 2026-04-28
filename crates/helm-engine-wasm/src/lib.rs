//! Embedded Helm v4 template engine.
//!
//! Thin shim over `engine-host-wasm`: holds the helm `.cwasm` bytes +
//! the helm-specific export-name spec, exposes `render` / `render_dir`.
//! Go source lives in `../go-src/`; see top-of-tree docs and
//! `crates/engine-host-wasm/src/lib.rs` for the sandbox posture +
//! session-reuse rationale.
//!
//! ## Why 20 MB wasm
//!
//! With the client-go strip fork applied (`fork/apply.sh`). Stock Helm
//! v4's `pkg/engine.New(*rest.Config)` pulls `k8s.io/client-go`
//! transitively — ~55 MB dead weight for a renderer that talks to no
//! cluster. Fork patches the `rest.Config` path out.

use std::path::Path;

use engine_host_wasm::{EngineSpec, SessionSlot};
use serde::{Deserialize, Serialize};

/// Embedded engine bytes — AOT-compiled `.cwasm` (default) or source
/// `.wasm` (with `precompile` feature OFF, for the `@akua-dev/sdk` npm
/// distribution where binary size matters more than cold-start
/// latency). `IS_PRECOMPILED` tags which API path on
/// [`engine_host_wasm::Session`] to take.
#[cfg(feature = "precompile")]
const HELM_ENGINE_BYTES_EMBEDDED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/helm-engine.cwasm"));
#[cfg(not(feature = "precompile"))]
const HELM_ENGINE_BYTES_EMBEDDED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/helm-engine.wasm"));
const IS_PRECOMPILED: bool = cfg!(feature = "precompile");

/// Filename the engine bytes live under when loaded from
/// [`AKUA_NATIVE_ENGINES_DIR`].
const ENGINE_FILENAME: &str = if cfg!(feature = "precompile") {
    "helm-engine.cwasm"
} else {
    "helm-engine.wasm"
};

/// Resolve the engine bytes once per process. With the env var set,
/// `<dir>/helm-engine.{cwasm|wasm}` is read at first call and cached;
/// otherwise the embedded bytes serve. Lets the `@akua-dev/native`
/// loader hand the napi addon a path into `@akua-dev/native-engines`
/// (tracked at #473) without changing the API.
fn engine_bytes() -> &'static [u8] {
    use std::sync::OnceLock;
    static OVERRIDE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let slot = OVERRIDE.get_or_init(|| {
        let dir = std::env::var_os(ENV_NATIVE_ENGINES_DIR)?;
        let path = std::path::Path::new(&dir).join(ENGINE_FILENAME);
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            // Empty file or read failure → fall back to embedded so
            // a misconfigured env var doesn't take down rendering.
            // Loud warning would be nicer; library code can't print
            // without leaking through to user output.
            _ => None,
        }
    });
    slot.as_deref().unwrap_or(HELM_ENGINE_BYTES_EMBEDDED)
}

/// Env var the napi loader writes when `@akua-dev/native-engines` is
/// installed alongside the per-platform addon, pointing at that
/// package's directory. Public so tests + the loader can set it
/// without copy-pasting the literal.
pub const ENV_NATIVE_ENGINES_DIR: &str = "AKUA_NATIVE_ENGINES_DIR";

const SPEC: EngineSpec = EngineSpec {
    name: "helm-engine",
    prefix: "helm",
    entry: "helm_render",
};

#[derive(Debug, thiserror::Error)]
pub enum HelmEngineError {
    #[error(transparent)]
    Host(#[from] engine_host_wasm::EngineHostError),

    #[error("engine: {0}")]
    Engine(String),

    #[error("serializing input: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
}

impl Default for Release {
    fn default() -> Self {
        Self {
            name: "release".to_string(),
            namespace: "default".to_string(),
            revision: 1,
            service: "Helm".to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RenderRequest<'a> {
    chart_tar_gz_b64: String,
    values_yaml: &'a str,
    release: Release,
}

#[derive(Debug, Deserialize)]
struct RenderResponse {
    #[serde(default)]
    manifests: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    error: String,
}

/// Render a chart tarball with values via the embedded Helm engine.
/// Returns `<template-path>` → rendered YAML (matches
/// `helm.sh/helm/v4/pkg/engine.Render`'s output shape).
pub fn render(
    chart_tar_gz: &[u8],
    values_yaml: &str,
    release: &Release,
) -> Result<std::collections::BTreeMap<String, String>, HelmEngineError> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(chart_tar_gz);
    let req = RenderRequest {
        chart_tar_gz_b64: b64,
        values_yaml,
        release: release.clone(),
    };
    let input = serde_json::to_vec(&req)?;

    let output = call_guest(&input)?;
    let resp: RenderResponse = serde_json::from_slice(&output)?;
    if !resp.error.is_empty() {
        return Err(HelmEngineError::Engine(resp.error));
    }
    Ok(resp.manifests)
}

/// Render from a chart directory on disk (convenience wrapper).
pub fn render_dir(
    chart_dir: &Path,
    chart_name: &str,
    values_yaml: &str,
    release: &Release,
) -> Result<std::collections::BTreeMap<String, String>, HelmEngineError> {
    let tar_gz = tar_chart_dir(chart_dir, chart_name)?;
    render(&tar_gz, values_yaml, release)
}

fn tar_chart_dir(chart_dir: &Path, chart_name: &str) -> Result<Vec<u8>, HelmEngineError> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(chart_name, chart_dir)?;
        tar.finish()?;
    }
    gz.flush()?;
    Ok(gz.finish()?)
}

thread_local! {
    static SESSION: SessionSlot = const { std::cell::RefCell::new(None) };
}

fn call_guest(input: &[u8]) -> Result<Vec<u8>, HelmEngineError> {
    SESSION.with(|slot| {
        engine_host_wasm::thread_local_call_with(slot, engine_bytes(), SPEC, input, IS_PRECOMPILED)
            .map_err(HelmEngineError::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_is_built() -> bool {
        engine_bytes().len() > 1_000_000
    }

    #[test]
    fn embedded_cwasm_bytes_present_or_placeholder() {
        assert!(
            engine_bytes().is_empty() || engine_bytes().len() > 1_000_000,
            "helm-engine.cwasm has suspicious size: {} bytes",
            engine_bytes().len()
        );
    }

    #[test]
    fn engine_filename_matches_precompile_feature() {
        // Pinning the on-disk filename so the napi loader (which writes
        // these names into the @akua-dev/native-engines package) can
        // hardcode the same string. If precompile is on, we look for
        // `<dir>/helm-engine.cwasm`; if off, `<dir>/helm-engine.wasm`.
        if cfg!(feature = "precompile") {
            assert_eq!(ENGINE_FILENAME, "helm-engine.cwasm");
        } else {
            assert_eq!(ENGINE_FILENAME, "helm-engine.wasm");
        }
    }

    #[test]
    fn env_var_name_is_stable_contract() {
        // Loader writes this env var; engine reads it. Pinned so the
        // two halves can't drift. Renaming requires updating the
        // napi loader at the same time — see #473.
        assert_eq!(ENV_NATIVE_ENGINES_DIR, "AKUA_NATIVE_ENGINES_DIR");
    }

    #[test]
    fn renders_minimal_chart() {
        if !engine_is_built() {
            eprintln!("skipping: helm-engine.wasm not built (run `task build:helm-engine-wasm`)");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let chart = tmp.path().join("mychart");
        std::fs::create_dir_all(chart.join("templates")).unwrap();
        std::fs::write(
            chart.join("Chart.yaml"),
            "apiVersion: v2\nname: mychart\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(chart.join("values.yaml"), "greeting: hi\n").unwrap();
        std::fs::write(
            chart.join("templates/cm.yaml"),
            r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}-cm
data:
  greeting: {{ .Values.greeting | quote }}
"#,
        )
        .unwrap();
        let tar_gz = tar_chart_dir(&chart, "mychart").unwrap();
        let out = render(
            &tar_gz,
            "greeting: hello\n",
            &Release {
                name: "demo".to_string(),
                namespace: "default".to_string(),
                revision: 1,
                service: "Helm".to_string(),
            },
        )
        .expect("render");
        let (_path, yaml) = out
            .iter()
            .find(|(k, _)| k.ends_with("templates/cm.yaml"))
            .expect("cm.yaml rendered");
        assert!(yaml.contains("demo-cm"), "rendered: {yaml}");
        assert!(yaml.contains("hello"), "rendered: {yaml}");
    }

    #[test]
    fn render_error_propagates_from_plugin() {
        if !engine_is_built() {
            eprintln!("skipping: helm-engine.wasm not built");
            return;
        }
        // Truncated tarball → engine returns an error.
        let result = render(&[0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0], "", &Release::default());
        match result {
            Err(HelmEngineError::Engine(_)) => {}
            other => panic!("expected Engine error, got {other:?}"),
        }
    }
}
