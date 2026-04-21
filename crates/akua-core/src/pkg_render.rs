//! `pkg.render(Render) -> sentinel` — recursive Package composition.
//!
//! # Architecture: post-eval expansion
//!
//! The obvious design — recurse into `PackageK::render` inside the
//! plugin handler — deadlocks against KCL upstream, which holds the
//! global `PLUGIN_HANDLER_FN_PTR` mutex across every plugin call.
//! A nested `FastRunner::run` tries to re-acquire the same mutex
//! on the same thread; `std::sync::Mutex` isn't reentrant.
//!
//! Instead, the handler returns a **sentinel** shaped like
//!
//! ```json
//! { "akuaPkgRenderSentinel": { "path": "…", "inputs": {…} } }
//! ```
//!
//! (camelCase; KCL's plan serializer strips keys whose names start
//! with `_`, so the usual `__dunder__` convention doesn't survive
//! the round-trip.)
//!
//! The sentinel lands in the caller's `resources` list. After the
//! caller's eval_kcl completes and releases the KCL mutex,
//! [`expand_sentinels`] walks the resources, loads the referenced
//! Package.k, calls its `render()`, and splices the nested
//! resources in place. The inner render gets a fresh KCL evaluator
//! session with no contention.
//!
//! Cycle detection uses the thread-local render-scope established
//! by [`crate::kcl_plugin::RenderScope`]: if an expansion would
//! push a path already on the stack, we reject with a typed error.

use std::path::{Path, PathBuf};

use serde_yaml::Value as YamlValue;

use crate::{kcl_plugin, PackageK};

pub const PLUGIN_NAME: &str = "pkg.render";

/// Sentinel key the handler emits, detected by [`expand_sentinels`]
/// during post-render expansion.
///
/// KCL's plan serializer strips keys whose names start with `_`
/// (the language's "private" convention), which is why this key
/// lives in a camelCase namespace instead of the more conventional
/// `__akua_pkg_render__`.
pub const SENTINEL_KEY: &str = "akuaPkgRenderSentinel";

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}

/// Register the plugin. The handler is intentionally cheap — it
/// serializes a sentinel and returns. Actual rendering is deferred
/// to [`expand_sentinels`].
pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let opts = kcl_plugin::extract_options_arg(args, PLUGIN_NAME, "pkg.Render")?;
        let path_str = opts
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| err("options.path must be a string"))?;
        let inputs = opts
            .get("inputs")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Emit a list (pkg.render's KCL-side contract is "returns
        // [resource]") with one sentinel in it. expand_sentinels
        // replaces the sentinel with the rendered list.
        Ok(serde_json::json!([{
            SENTINEL_KEY: {
                "path": path_str,
                "inputs": inputs,
            }
        }]))
    });
}

/// Walk `resources`, replacing every pkg.render sentinel with the
/// rendered resources of the referenced Package. Recursive: nested
/// sentinels inside inner renders expand in turn. Cycle detection
/// via [`kcl_plugin::is_rendering`].
///
/// Called by `PackageK::render` after `eval_kcl` returns — the KCL
/// evaluator mutex is no longer held, so inner renders are free to
/// acquire it fresh.
pub fn expand_sentinels(
    resources: Vec<YamlValue>,
) -> Result<Vec<YamlValue>, crate::package_k::PackageKError> {
    let mut out = Vec::with_capacity(resources.len());
    for r in resources {
        if let Some(call) = extract_sentinel(&r) {
            let nested = render_nested(&call.path, &call.inputs)?;
            out.extend(nested);
        } else {
            out.push(r);
        }
    }
    Ok(out)
}

struct SentinelCall {
    path: String,
    inputs: YamlValue,
}

fn extract_sentinel(v: &YamlValue) -> Option<SentinelCall> {
    let inner = v.get(SENTINEL_KEY)?;
    let path = inner.get("path").and_then(YamlValue::as_str)?.to_string();
    let inputs = inner.get("inputs").cloned().unwrap_or(YamlValue::Null);
    Some(SentinelCall { path, inputs })
}

fn render_nested(
    path_str: &str,
    inputs: &YamlValue,
) -> Result<Vec<YamlValue>, crate::package_k::PackageKError> {
    let raw = PathBuf::from(path_str);
    let resolved = kcl_plugin::resolve_against_package(&raw);
    let target = resolve_package_file(&resolved);

    if kcl_plugin::is_rendering(&target) {
        return Err(crate::package_k::PackageKError::Cycle { path: target });
    }

    let pkg = PackageK::load(&target)?;
    // `render` recurses naturally — it pushes its own scope, evals,
    // and walks its own sentinels. The `_scope` RAII in `render`
    // ensures the stack stays balanced even on error.
    let rendered = pkg.render(inputs)?;
    Ok(rendered.resources)
}

/// Accept either a directory (append `package.k`) or a direct file path.
fn resolve_package_file(resolved: &Path) -> PathBuf {
    if resolved.is_dir() {
        resolved.join("package.k")
    } else {
        resolved.to_path_buf()
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

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
        path
    }

    /// Minimal inner Package — ConfigMap with name driven by input.
    const INNER: &str = r#"
schema Input:
    name: str = "inner"

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.name
}]
"#;

    #[test]
    fn outer_package_expands_inner_pkg_render_sentinel() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "inner.k", INNER);
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

_nested = pkg.render({ path = "./inner.k", inputs = { name = "from-outer" } })

resources = _nested"#,
        );

        let outer = PackageK::load(&outer_path).expect("load outer");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render outer");

        assert_eq!(rendered.resources.len(), 1, "sentinel should expand to one ConfigMap");
        let cm = &rendered.resources[0];
        assert_eq!(cm["kind"], YamlValue::String("ConfigMap".into()));
        assert_eq!(
            cm["metadata"]["name"],
            YamlValue::String("from-outer".into())
        );
    }

    #[test]
    fn detects_direct_cycle_via_render_stack() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "cyclic.k",
            r#"
import kcl_plugin.pkg

_self = pkg.render({ path = "./cyclic.k" })

resources = _self"#,
        );

        let pkg = PackageK::load(&tmp.path().join("cyclic.k")).expect("load");
        let err = pkg
            .render(&YamlValue::Mapping(Default::default()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("cycle detected"), "got: {err}");
    }

    #[test]
    fn directory_path_resolves_to_implicit_package_k() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("nested")).unwrap();
        write(&tmp.path().join("nested"), "package.k", INNER);

        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

# Directory, not a file — pkg.render appends package.k.
_rs = pkg.render({ path = "./nested" })

resources = _rs"#,
        );

        let outer = PackageK::load(&outer_path).expect("load");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render");
        assert_eq!(rendered.resources.len(), 1);
        // Default `name = "inner"` from inner schema.
        assert_eq!(
            rendered.resources[0]["metadata"]["name"],
            YamlValue::String("inner".into())
        );
    }

    #[test]
    fn nested_pkg_render_expands_recursively() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "deep.k", INNER);
        write(
            tmp.path(),
            "middle.k",
            r#"
import kcl_plugin.pkg

resources = pkg.render({ path = "./deep.k", inputs = { name = "deep-from-middle" } })"#,
        );
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

resources = pkg.render({ path = "./middle.k" })"#,
        );

        let outer = PackageK::load(&outer_path).expect("load");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render");
        assert_eq!(rendered.resources.len(), 1);
        // The innermost render used its default name; `middle`
        // didn't plumb name through. Asserting we reached the
        // innermost resource via two sentinel expansions.
        assert_eq!(
            rendered.resources[0]["kind"],
            YamlValue::String("ConfigMap".into())
        );
    }
}
