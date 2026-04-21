//! `kustomize.build` plugin — hosted by the embedded `kustomize-engine-wasm`.
//!
//! Parallel to `crate::helm`: validates the overlay path inside the
//! Package via [`kcl_plugin::resolve_in_package`], hands the overlay
//! dir to the embedded WASM engine, parses the rendered multi-doc
//! YAML into resources.

use std::path::PathBuf;

use serde_json::Value;

use crate::kcl_plugin;

pub const PLUGIN_NAME: &str = "kustomize.build";

pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let opts = kcl_plugin::extract_options_arg(args, PLUGIN_NAME, "kustomize.Build")?;
        let path = opts
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| err("options.path must be a string"))?;

        let resolved = kcl_plugin::resolve_in_package(&PathBuf::from(path))
            .map_err(|e| err(format!("resolving overlay path: {e}")))?;

        let yaml = kustomize_engine_wasm::render_dir(&resolved)
            .map_err(|e| err(format!("kustomize engine: {e}")))?;

        let docs = crate::yaml_multidoc::parse(yaml.as_bytes(), PLUGIN_NAME)?;
        Ok(Value::Array(docs))
    });
}

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}
