//! Embedded kustomize engine.
//!
//! Thin shim over `engine-host-wasm`: holds the kustomize `.cwasm`
//! bytes + the kustomize-specific export-name spec, exposes
//! `render_dir` / `render_tar`. Go source in `../go-src/`; shared
//! sandbox posture + session-reuse rationale in
//! `crates/engine-host-wasm/src/lib.rs`.

use std::path::Path;

use engine_host_wasm::{EngineSpec, SessionSlot};
use serde::{Deserialize, Serialize};

/// Embedded engine bytes — AOT `.cwasm` (default) or source `.wasm`
/// (with `precompile` feature OFF, for `@akua-dev/sdk`'s npm
/// distribution). See helm-engine-wasm for the same pattern.
/// With `embed-engines` OFF, the embedded slot is empty — see
/// helm-engine-wasm for the migration plan (#482).
#[cfg(all(feature = "precompile", feature = "embed-engines"))]
const KUSTOMIZE_ENGINE_BYTES_EMBEDDED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/kustomize-engine.cwasm"));
#[cfg(all(not(feature = "precompile"), feature = "embed-engines"))]
const KUSTOMIZE_ENGINE_BYTES_EMBEDDED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/kustomize-engine.wasm"));
#[cfg(not(feature = "embed-engines"))]
const KUSTOMIZE_ENGINE_BYTES_EMBEDDED: &[u8] = &[];
const IS_PRECOMPILED: bool = cfg!(feature = "precompile");

/// Filename the engine bytes live under when loaded from
/// the `AKUA_NATIVE_ENGINES_DIR` override.
const ENGINE_FILENAME: &str = if cfg!(feature = "precompile") {
    "kustomize-engine.cwasm"
} else {
    "kustomize-engine.wasm"
};

/// Resolve engine bytes once per process. See helm-engine-wasm for
/// the rationale + #473 for the migration to a separate
/// `@akua-dev/native-engines` npm package.
fn engine_bytes() -> &'static [u8] {
    use std::sync::OnceLock;
    static OVERRIDE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let slot = OVERRIDE.get_or_init(|| {
        // Single env var name across both engine crates — see
        // helm_engine_wasm::ENV_NATIVE_ENGINES_DIR. Hardcoded here
        // (not imported) to keep this crate buildable without a
        // direct dep on helm-engine-wasm.
        let dir = std::env::var_os("AKUA_NATIVE_ENGINES_DIR")?;
        let path = std::path::Path::new(&dir).join(ENGINE_FILENAME);
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            _ => None,
        }
    });
    slot.as_deref().unwrap_or(KUSTOMIZE_ENGINE_BYTES_EMBEDDED)
}

const SPEC: EngineSpec = EngineSpec {
    name: "kustomize-engine",
    prefix: "kustomize",
    entry: "kustomize_build",
};

#[derive(Debug, thiserror::Error)]
pub enum KustomizeEngineError {
    #[error(transparent)]
    Host(#[from] engine_host_wasm::EngineHostError),

    #[error("engine: {0}")]
    Engine(String),

    #[error("serializing input: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Serialize)]
struct BuildRequest<'a> {
    overlay_tar_gz_b64: String,
    entrypoint: &'a str,
}

#[derive(Debug, Deserialize)]
struct BuildResponse {
    #[serde(default)]
    yaml: String,
    #[serde(default)]
    error: String,
}

/// Render a kustomize overlay directory. Tars `overlay_dir`'s parent so
/// sibling paths like `../base` resolve correctly, hands the tarball to
/// the WASM engine, returns the rendered multi-doc YAML.
pub fn render_dir(overlay_dir: &Path) -> Result<String, KustomizeEngineError> {
    let entrypoint = overlay_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("overlay")
        .to_string();
    let parent = overlay_dir.parent().ok_or_else(|| {
        KustomizeEngineError::Engine(format!(
            "overlay dir has no parent: {}",
            overlay_dir.display()
        ))
    })?;
    let parent_name = parent
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pkg")
        .to_string();
    let tar_gz = tar_dir(parent, &parent_name)?;
    let guest_entrypoint = format!("{parent_name}/{entrypoint}");
    render_tar(&tar_gz, &guest_entrypoint)
}

/// Render from an already-tar.gz'd overlay tree. `entrypoint` is the
/// path (inside the tarball) of the directory containing
/// `kustomization.yaml`.
pub fn render_tar(tar_gz: &[u8], entrypoint: &str) -> Result<String, KustomizeEngineError> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(tar_gz);
    let req = BuildRequest {
        overlay_tar_gz_b64: b64,
        entrypoint,
    };
    let input = serde_json::to_vec(&req)?;
    let output = call_guest(&input)?;
    let resp: BuildResponse = serde_json::from_slice(&output)?;
    if !resp.error.is_empty() {
        return Err(KustomizeEngineError::Engine(resp.error));
    }
    Ok(resp.yaml)
}

fn tar_dir(dir: &Path, name_in_archive: &str) -> Result<Vec<u8>, KustomizeEngineError> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(name_in_archive, dir)?;
        tar.finish()?;
    }
    gz.flush()?;
    Ok(gz.finish()?)
}

thread_local! {
    static SESSION: SessionSlot = const { std::cell::RefCell::new(None) };
}

fn call_guest(input: &[u8]) -> Result<Vec<u8>, KustomizeEngineError> {
    SESSION.with(|slot| {
        engine_host_wasm::thread_local_call_with(slot, engine_bytes(), SPEC, input, IS_PRECOMPILED)
            .map_err(KustomizeEngineError::from)
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
            "kustomize-engine.cwasm has suspicious size: {} bytes",
            engine_bytes().len()
        );
    }

    #[test]
    fn renders_minimal_overlay() {
        if !engine_is_built() {
            eprintln!("skipping: kustomize-engine.wasm not built");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        let base = pkg.join("base");
        let overlay = pkg.join("overlay");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(
            base.join("kustomization.yaml"),
            "resources:\n  - configmap.yaml\n",
        )
        .unwrap();
        std::fs::write(
            base.join("configmap.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: hello\ndata:\n  greeting: hi\n",
        )
        .unwrap();
        std::fs::write(
            overlay.join("kustomization.yaml"),
            "resources:\n  - ../base\nnamePrefix: prod-\n",
        )
        .unwrap();

        let yaml = render_dir(&overlay).expect("render");
        assert!(yaml.contains("prod-hello"), "rendered: {yaml}");
        assert!(yaml.contains("greeting: hi"), "rendered: {yaml}");
    }
}
