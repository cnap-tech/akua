//! `kustomize.build` engine callable via shell-out to the `kustomize` binary.
//!
//! Transitional MVP mirror of [`crate::helm`] — gated behind
//! `engine-kustomize-shell`, expected to be replaced by a WASM-embedded
//! engine that registers under the same plugin name.
//!
//! ## Plugin contract
//!
//! ```kcl
//! import akua.kustomize
//! _manifests = kustomize.build("./overlays/prod")
//! ```
//!
//! - `path: str` — filesystem path to a kustomization directory
//!   (one containing `kustomization.yaml` / `.yml` / `Kustomization`).
//!   Resolved against the calling Package.k's directory via the
//!   thread-local render scope.
//!
//! Returns `[dict]` — the list of Kubernetes resources kustomize
//! produced, with empty separator docs dropped.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::kcl_plugin;

/// Plugin name KCL uses to reach the handler. Stable across the
/// shell-out and any future WASM-embedded successor.
pub const PLUGIN_NAME: &str = "kustomize.build";

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}

/// Register `kustomize.build` with the global plugin dispatcher.
/// Idempotent — re-registering replaces the prior handler.
pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let arr = args
            .as_array()
            .ok_or_else(|| err("expected positional args as JSON array"))?;
        let path = arr
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| err("arg 0 (path) must be a string"))?;

        let resolved = kcl_plugin::resolve_against_package(&PathBuf::from(path));
        let rendered = build(&resolved)?;
        Ok(Value::Array(rendered))
    });
}

/// Shell out to `kustomize build <path>` and parse the multi-doc
/// YAML output into one `Value` per resource.
pub fn build(path: &Path) -> Result<Vec<Value>, String> {
    if !has_kustomization_file(path) {
        return Err(err(format!(
            "no kustomization.yaml / kustomization.yml / Kustomization at {}",
            path.display()
        )));
    }

    let mut cmd = Command::new("kustomize");
    cmd.arg("build")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            err("`kustomize` not found on PATH (engine-kustomize-shell requires it)")
        } else {
            err(format!("spawn failed: {e}"))
        }
    })?;

    let output = child
        .wait_with_output()
        .map_err(|e| err(format!("waiting for kustomize: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(format!("`kustomize build` failed: {}", stderr.trim())));
    }

    parse_multi_doc(&output.stdout)
}

/// Kustomize recognizes any of these filenames as the entrypoint —
/// matches the upstream `kustomize` CLI's own list (see
/// `sigs.k8s.io/kustomize/kustomize/commands/build`).
fn has_kustomization_file(path: &Path) -> bool {
    for candidate in ["kustomization.yaml", "kustomization.yml", "Kustomization"] {
        if path.join(candidate).is_file() {
            return true;
        }
    }
    false
}

fn parse_multi_doc(bytes: &[u8]) -> Result<Vec<Value>, String> {
    use serde::de::Deserialize;

    let text = std::str::from_utf8(bytes).map_err(|e| err(format!("output not utf-8: {e}")))?;

    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let value = Value::deserialize(doc)
            .map_err(|e| err(format!("parsing output as YAML: {e}")))?;
        if is_empty_doc(&value) {
            continue;
        }
        out.push(value);
    }
    Ok(out)
}

fn is_empty_doc(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_multi_doc_yaml_into_resource_list() {
        let text = br#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: first
---
apiVersion: v1
kind: Service
metadata:
  name: second
"#;
        let docs = parse_multi_doc(text).expect("parse");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["kind"], "ConfigMap");
        assert_eq!(docs[1]["kind"], "Service");
    }

    #[test]
    fn drops_empty_separator_docs() {
        let text = b"---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n---\n---\n";
        let docs = parse_multi_doc(text).expect("parse");
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn has_kustomization_file_accepts_every_canonical_name() {
        for name in ["kustomization.yaml", "kustomization.yml", "Kustomization"] {
            let tmp = TempDir::new().unwrap();
            fs::write(tmp.path().join(name), "resources: []\n").unwrap();
            assert!(has_kustomization_file(tmp.path()), "missed: {name}");
        }
    }

    #[test]
    fn has_kustomization_file_rejects_empty_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(!has_kustomization_file(tmp.path()));
    }

    #[test]
    fn build_rejects_dir_without_kustomization() {
        let tmp = TempDir::new().unwrap();
        let e = build(tmp.path()).unwrap_err();
        assert!(e.contains("no kustomization"), "got: {e}");
    }

    fn write_minimal_overlay(dir: &std::path::Path) {
        fs::write(
            dir.join("kustomization.yaml"),
            "resources:\n  - cm.yaml\n",
        )
        .unwrap();
        fs::write(
            dir.join("cm.yaml"),
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: demo\ndata:\n  k: v\n",
        )
        .unwrap();
    }

    /// End-to-end against a real `kustomize` on PATH. Ignored by
    /// default — run with
    /// `cargo test --features engine-kustomize-shell -- --ignored`.
    #[test]
    #[ignore = "requires `kustomize` on PATH; run with --ignored"]
    fn build_invokes_kustomize_and_parses_output() {
        let tmp = TempDir::new().unwrap();
        write_minimal_overlay(tmp.path());

        let docs = build(tmp.path()).expect("build");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["kind"], "ConfigMap");
        assert_eq!(docs[0]["metadata"]["name"], "demo");
    }

    #[test]
    fn install_registers_the_plugin() {
        // Missing-kustomization error fires before kustomize runs,
        // so this test needs no binary on PATH.
        install();

        let tmp = TempDir::new().unwrap();
        let args = serde_json::json!([tmp.path().to_string_lossy()]);
        let kwargs = serde_json::json!({});
        let method = std::ffi::CString::new("kcl_plugin.kustomize.build").unwrap();
        let args_c = std::ffi::CString::new(args.to_string()).unwrap();
        let kwargs_c = std::ffi::CString::new(kwargs.to_string()).unwrap();
        let out_ptr = unsafe {
            crate::kcl_plugin::dispatch(method.as_ptr(), args_c.as_ptr(), kwargs_c.as_ptr())
        };
        let owned = unsafe { std::ffi::CString::from_raw(out_ptr as *mut std::os::raw::c_char) };
        let parsed: Value = serde_json::from_slice(owned.as_bytes()).unwrap();
        let panic_msg = parsed["__kcl_PanicInfo__"].as_str().expect("panic envelope");
        assert!(panic_msg.contains("no kustomization"), "got: {panic_msg}");
    }
}
