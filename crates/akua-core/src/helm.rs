//! `helm.template` engine callable via shell-out to the `helm` binary.
//!
//! This is the **transitional MVP** helm engine: it shells out to
//! whatever `helm` lives on `PATH`. That breaks the CLAUDE.md
//! "embedded by default" invariant and is explicitly gated behind
//! the `engine-helm-shell` feature. A WASM-embedded successor will
//! register under the same plugin name and transparently replace
//! this path.
//!
//! ## Plugin contract
//!
//! KCL side:
//!
//! ```kcl
//! import kcl_plugin.helm
//! _manifests = helm.template(chart_path, values, release_name, release_namespace)
//! ```
//!
//! - `chart_path: str` — filesystem path to a chart directory or `.tgz`.
//! - `values: {str:}` — arbitrary values tree, serialized to YAML.
//! - `release_name: str` — optional; default `"release"`.
//! - `release_namespace: str` — optional; default `"default"`.
//!
//! Returns `[dict]` — the list of Kubernetes resources helm produced.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::kcl_plugin;

/// Register `helm.template` with the global plugin dispatcher. Call
/// once at process start when the `engine-helm-shell` feature is on.
/// Idempotent — re-registering replaces the prior handler.
pub fn install() {
    kcl_plugin::register("helm.template", |args, _kwargs| {
        let arr = args
            .as_array()
            .ok_or_else(|| "helm.template: expected positional args as JSON array".to_string())?;
        let chart_path = arr
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| "helm.template: arg 0 (chart_path) must be a string".to_string())?;
        // A null/missing `values` becomes `{}` rather than serialized
        // `null` — chart templates that deref `.Values.x` without a
        // default would otherwise error on `<nil>.x`.
        let values = match arr.get(1) {
            Some(Value::Null) | None => Value::Object(Default::default()),
            Some(v) => v.clone(),
        };
        let release_name = arr.get(2).and_then(Value::as_str).unwrap_or("release");
        let release_ns = arr.get(3).and_then(Value::as_str).unwrap_or("default");

        validate_release_name(release_name)?;
        validate_namespace(release_ns)?;

        let rendered = template(
            &PathBuf::from(chart_path),
            &values,
            release_name,
            release_ns,
        )?;
        Ok(Value::Array(rendered))
    });
}

/// Helm's own release-name rule: lowercase alphanumeric + `-`, max 53
/// chars, must start with alphanumeric. Reject anything else before
/// the string reaches the subprocess command line — prevents a
/// `release_name = "--post-renderer=..."` from being interpreted as a
/// helm flag.
fn validate_release_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 53 {
        return Err(format!(
            "helm.template: release name `{name}` must be 1..=53 chars"
        ));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "helm.template: release name `{name}` must start with a lowercase letter or digit"
        ));
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
            return Err(format!(
                "helm.template: release name `{name}` may contain only [a-z0-9-]"
            ));
        }
    }
    Ok(())
}

/// Kubernetes DNS-1123 label rule: 1..=63 lowercase alphanumeric +
/// `-`, must start + end with alphanumeric.
fn validate_namespace(ns: &str) -> Result<(), String> {
    if ns.is_empty() || ns.len() > 63 {
        return Err(format!(
            "helm.template: namespace `{ns}` must be 1..=63 chars"
        ));
    }
    let chars: Vec<char> = ns.chars().collect();
    let ok_edge =
        |c: char| -> bool { c.is_ascii_lowercase() || c.is_ascii_digit() };
    if !ok_edge(chars[0]) || !ok_edge(*chars.last().unwrap()) {
        return Err(format!(
            "helm.template: namespace `{ns}` must start and end with a lowercase letter or digit"
        ));
    }
    for c in &chars {
        if !ok_edge(*c) && *c != '-' {
            return Err(format!(
                "helm.template: namespace `{ns}` may contain only [a-z0-9-]"
            ));
        }
    }
    Ok(())
}

/// Core rendering: write values to stdin, run `helm template`, parse
/// the multi-doc YAML output into a list of `serde_json::Value`. Pure
/// subprocess I/O.
pub fn template(
    chart_path: &std::path::Path,
    values: &Value,
    release_name: &str,
    release_namespace: &str,
) -> Result<Vec<Value>, String> {
    let values_yaml = serde_yaml::to_string(values)
        .map_err(|e| format!("serializing helm values: {e}"))?;

    let mut cmd = Command::new("helm");
    cmd.arg("template")
        .arg(release_name)
        .arg(chart_path)
        .arg("--namespace")
        .arg(release_namespace)
        .arg("--values")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "helm.template: `helm` not found on PATH (engine-helm-shell requires it)".to_string()
        } else {
            format!("helm.template: spawn failed: {e}")
        }
    })?;

    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(values_yaml.as_bytes())
        .map_err(|e| format!("helm.template: writing values to stdin: {e}"))?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("helm.template: waiting for helm: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("helm.template: `helm template` failed: {}", stderr.trim()));
    }

    parse_multi_doc(&output.stdout)
}

/// Parse helm's multi-document YAML output into one `Value` per
/// document. Empty docs (helm emits them between resources) are
/// dropped so the returned list is usable in `resources = [*_x]`.
fn parse_multi_doc(bytes: &[u8]) -> Result<Vec<Value>, String> {
    use serde::de::Deserialize;

    let text = std::str::from_utf8(bytes)
        .map_err(|e| format!("helm output not utf-8: {e}"))?;

    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let value = Value::deserialize(doc)
            .map_err(|e| format!("parsing helm output as YAML: {e}"))?;
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
        assert_eq!(docs[0]["metadata"]["name"], "x");
    }

    #[test]
    fn empty_input_produces_empty_list() {
        assert_eq!(parse_multi_doc(b"").unwrap(), Vec::<Value>::new());
        assert_eq!(parse_multi_doc(b"---\n").unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn invalid_utf8_surfaces_typed_error() {
        let err = parse_multi_doc(&[0xff, 0xfe, 0xfd]).unwrap_err();
        assert!(err.contains("not utf-8"));
    }

    fn write_minimal_chart(dir: &std::path::Path) {
        fs::write(
            dir.join("Chart.yaml"),
            "apiVersion: v2\nname: mychart\nversion: 0.1.0\n",
        )
        .unwrap();
        fs::create_dir(dir.join("templates")).unwrap();
        fs::write(
            dir.join("templates/cm.yaml"),
            r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}-cfg
data:
  greeting: {{ .Values.greeting | quote }}
"#,
        )
        .unwrap();
    }

    /// End-to-end against a real `helm` binary on PATH. Ignored by
    /// default so CI without helm installed stays green; run with
    /// `cargo test --features engine-helm-shell -- --ignored`.
    #[test]
    #[ignore = "requires `helm` on PATH; run with --ignored"]
    fn template_invokes_helm_and_parses_output() {
        let tmp = TempDir::new().unwrap();
        write_minimal_chart(tmp.path());

        let values = serde_json::json!({ "greeting": "hi from test" });
        let docs = template(tmp.path(), &values, "demo", "default").expect("template");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["kind"], "ConfigMap");
        assert_eq!(docs[0]["metadata"]["name"], "demo-cfg");
        assert_eq!(docs[0]["data"]["greeting"], "hi from test");
    }

    #[test]
    fn install_registers_the_plugin() {
        // Rejects via validate_release_name before helm ever runs, so
        // this test needs no helm binary. Proves the dispatcher
        // routes through our handler under the `helm.template` name.
        install();

        let args = serde_json::json!(["chart-path", {}, "--evil", "default"]);
        let kwargs = serde_json::json!({});
        let method = std::ffi::CString::new("kcl_plugin.helm.template").unwrap();
        let args_c = std::ffi::CString::new(args.to_string()).unwrap();
        let kwargs_c = std::ffi::CString::new(kwargs.to_string()).unwrap();
        let out_ptr = unsafe {
            crate::kcl_plugin::dispatch(method.as_ptr(), args_c.as_ptr(), kwargs_c.as_ptr())
        };
        let owned = unsafe { std::ffi::CString::from_raw(out_ptr as *mut std::os::raw::c_char) };
        let parsed: Value = serde_json::from_slice(owned.as_bytes()).unwrap();
        let panic_msg = parsed["__kcl_PanicInfo__"].as_str().expect("panic envelope");
        assert!(panic_msg.contains("release name"), "got: {panic_msg}");
    }

    #[test]
    fn release_name_rejects_leading_dash() {
        assert!(validate_release_name("--post-renderer=x").is_err());
        assert!(validate_release_name("-release").is_err());
    }

    #[test]
    fn release_name_accepts_helm_valid() {
        assert!(validate_release_name("my-app").is_ok());
        assert!(validate_release_name("release-1").is_ok());
        assert!(validate_release_name("0abc").is_ok());
    }

    #[test]
    fn namespace_rejects_bogus() {
        assert!(validate_namespace("").is_err());
        assert!(validate_namespace("-leading").is_err());
        assert!(validate_namespace("trailing-").is_err());
        assert!(validate_namespace("UPPER").is_err());
    }

    #[test]
    fn namespace_accepts_valid() {
        assert!(validate_namespace("default").is_ok());
        assert!(validate_namespace("kube-system").is_ok());
        assert!(validate_namespace("a").is_ok());
    }
}
