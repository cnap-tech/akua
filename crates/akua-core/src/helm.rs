//! `helm.template` plugin — hosted by the embedded `helm-engine-wasm`.
//!
//! The plugin handler receives an `akua.helm.Template` schema instance,
//! resolves the chart path inside the calling Package (via
//! [`kcl_plugin::resolve_in_package`]), tars the chart dir, hands
//! `(chart_tar_gz, values_yaml, release)` to the wasmtime-hosted Helm
//! engine, and returns the rendered Kubernetes resources to KCL.
//!
//! ## Sandboxing invariants
//!
//! - Chart path is validated by `resolve_in_package` — no `..` escape,
//!   no absolute paths, no symlink escape. See
//!   [`docs/security-model.md`](../../../docs/security-model.md).
//! - Helm runs inside wasmtime WASI — no network, no subprocess, no
//!   host filesystem access beyond the chart tarball we hand it.
//! - CLAUDE.md invariant: "Sandboxed by default. No shell-out, ever."

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::kcl_plugin;

pub const PLUGIN_NAME: &str = "helm.template";

pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let opts = kcl_plugin::extract_options_arg(args, PLUGIN_NAME, "helm.Template")?;

        let chart_path = opts
            .get("chart")
            .and_then(Value::as_str)
            .ok_or_else(|| err("options.chart must be a string"))?;
        let resolved_chart = kcl_plugin::resolve_in_package(&PathBuf::from(chart_path))
            .map_err(|e| err(format!("resolving chart path: {e}")))?;

        // Missing/null `values` → empty map. Chart templates that deref
        // `.Values.x` without a default would error on `<nil>.x` otherwise.
        let empty_map = serde_json::Map::new();
        let values_json = opts
            .get("values")
            .filter(|v| !v.is_null())
            .and_then(Value::as_object)
            .unwrap_or(&empty_map);
        let values_yaml =
            serde_yaml::to_string(values_json).map_err(|e| err(format!("serializing values: {e}")))?;

        let release_name = opts.get("release").and_then(Value::as_str).unwrap_or("release");
        let release_namespace = opts
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("default");

        validate_release_name(release_name)?;
        validate_namespace(release_namespace)?;

        let chart_name = chart_dir_name(&resolved_chart);
        let release = helm_engine_wasm::Release {
            name: release_name.to_string(),
            namespace: release_namespace.to_string(),
            revision: 1,
            service: "Helm".to_string(),
        };

        let manifests = helm_engine_wasm::render_dir(
            &resolved_chart,
            &chart_name,
            &values_yaml,
            &release,
        )
        .map_err(|e| err(format!("helm engine: {e}")))?;

        // Each helm manifest may contain multiple `---`-separated docs
        // (one chart file → N resources). Parse each through the shared
        // multi-doc YAML helper so empty separator docs drop cleanly
        // and errors attribute to `helm.template:` prefix.
        let mut resources = Vec::new();
        for yaml in manifests.values() {
            let docs = crate::yaml_multidoc::parse(yaml.as_bytes(), PLUGIN_NAME)?;
            resources.extend(docs);
        }
        Ok(Value::Array(resources))
    });
}

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}

/// Helm's release-name rule: 1..=53 lowercase alphanumeric + `-`, first
/// char alphanumeric. Reject before the name reaches the engine — even
/// inside the sandbox, deterministic rejection at the plugin boundary
/// gives Package authors a clear line-number error.
fn validate_release_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 53 {
        return Err(err(format!("release name `{name}` must be 1..=53 chars")));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(err(format!(
            "release name `{name}` must start with a lowercase letter or digit"
        )));
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
            return Err(err(format!(
                "release name `{name}` may contain only [a-z0-9-]"
            )));
        }
    }
    Ok(())
}

/// DNS-1123 label: 1..=63 lowercase alphanumeric + `-`, first/last
/// char alphanumeric.
fn validate_namespace(ns: &str) -> Result<(), String> {
    if ns.is_empty() || ns.len() > 63 {
        return Err(err(format!("namespace `{ns}` must be 1..=63 chars")));
    }
    let chars: Vec<char> = ns.chars().collect();
    let ok_edge = |c: char| -> bool { c.is_ascii_lowercase() || c.is_ascii_digit() };
    if !ok_edge(chars[0]) || !ok_edge(*chars.last().unwrap()) {
        return Err(err(format!(
            "namespace `{ns}` must start and end with a lowercase letter or digit"
        )));
    }
    for c in &chars {
        if !ok_edge(*c) && *c != '-' {
            return Err(err(format!("namespace `{ns}` may contain only [a-z0-9-]")));
        }
    }
    Ok(())
}

fn chart_dir_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("chart")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_name_valid() {
        assert!(validate_release_name("my-app").is_ok());
        assert!(validate_release_name("release-1").is_ok());
        assert!(validate_release_name("0abc").is_ok());
    }

    #[test]
    fn release_name_rejects_leading_dash() {
        assert!(validate_release_name("--post-renderer=x").is_err());
        assert!(validate_release_name("-release").is_err());
    }

    #[test]
    fn namespace_valid() {
        assert!(validate_namespace("default").is_ok());
        assert!(validate_namespace("kube-system").is_ok());
        assert!(validate_namespace("a").is_ok());
    }

    #[test]
    fn namespace_rejects_bogus() {
        assert!(validate_namespace("").is_err());
        assert!(validate_namespace("-leading").is_err());
        assert!(validate_namespace("trailing-").is_err());
        assert!(validate_namespace("UPPER").is_err());
    }
}
