//! `pkg.render(opts) -> [resource]` — recursive Package composition.
//!
//! # Architecture: synchronous engine plugin
//!
//! Mirrors the call shape of [`crate::helm`] and [`crate::kustomize`]:
//! the plugin handler runs the inner Package's `render()`
//! synchronously and returns the resulting list of resources to the
//! KCL caller. List-comprehension patches, filter expressions, and
//! anything else KCL does to a `[{str:}]` work natively because the
//! return is a real list, not a placeholder.
//!
//! ## Why this needs the patched KCL fork
//!
//! Upstream `kcl-runtime/src/stdlib/plugin.rs` historically held
//! `PLUGIN_HANDLER_FN_PTR` across the user-supplied callback. A
//! plugin that re-entered KCL deadlocked on the same thread —
//! `std::sync::Mutex` isn't reentrant. akua carries a one-line patch
//! at `cnap-tech/kcl#akua-wasm32` (commit `d584c0bc`) that copies the
//! fn pointer out of the lock before invoking it, freeing the
//! reentrant call. Without that patch this design hangs; the older
//! sentinel-deferred-expansion approach was a workaround that the
//! one-line fix retired.
//!
//! Cycle detection still uses the thread-local render stack
//! [`crate::kcl_plugin::RenderScope`]: `pkg.render` of a path
//! already on the stack returns [`crate::package_k::PackageKError::Cycle`]
//! before the inner load.
//!
//! See `cnap-tech/akua#479` for the rollout context.

use std::path::{Path, PathBuf};

use crate::{kcl_plugin, PackageK};

pub const PLUGIN_NAME: &str = "pkg.render";

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}

/// Register the synchronous `pkg.render` plugin. Replaces the
/// previous sentinel mechanism wholesale.
pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let opts = kcl_plugin::extract_options_arg(args, PLUGIN_NAME, "pkg.Render")?;
        let path_str = opts
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| err("options.path must be a string"))?;
        let inputs_json = opts
            .get("inputs")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Convert the JSON inputs into a serde_yaml::Value because
        // `PackageK::render` takes the same shape `ctx.input()` flows
        // through (yaml-typed). serde_yaml round-trips JSON cleanly —
        // every JSON value has a yaml equivalent, no Tag variants
        // appear here.
        let inputs_yaml = json_to_yaml(&inputs_json).map_err(err)?;

        // Sandbox guard: resolve the path against the current
        // RenderScope's package directory + reject `..` / symlink
        // escapes. Same guard helm.template / kustomize.build use.
        let raw = PathBuf::from(path_str);
        let resolved = kcl_plugin::resolve_in_package(&raw).map_err(|e| err(e.to_string()))?;
        let target = resolve_package_file(&resolved);

        // Cycle detection — bail before loading the file. The render
        // stack tracks every Package currently on the call chain;
        // hitting an already-rendering path means we're about to
        // recurse forever.
        if kcl_plugin::is_rendering(&target) {
            return Err(err(format!(
                "cycle detected — `{}` is already on the render stack",
                target.display()
            )));
        }

        // Load + render the inner Package. The recursion is bounded
        // by RenderScope (push on enter, pop on drop): even when the
        // inner Package itself calls pkg.render, the stack stays
        // balanced and the cycle check fires correctly.
        let pkg = PackageK::load(&target).map_err(|e| err(e.to_string()))?;
        let rendered = pkg.render(&inputs_yaml).map_err(|e| err(e.to_string()))?;

        // Convert back to serde_json — KCL's plugin contract returns
        // JSON, and the caller's `_up = pkg.render(...)` binding is
        // a real list of real dicts after this returns.
        let json_resources: Vec<serde_json::Value> = rendered
            .resources
            .into_iter()
            .map(|y| serde_json::to_value(y).map_err(|e| err(e.to_string())))
            .collect::<Result<_, _>>()?;
        Ok(serde_json::Value::Array(json_resources))
    });
}

/// Convert a `serde_json::Value` into a `serde_yaml::Value`. Every
/// JSON value has a YAML equivalent (no Tag variants, no anchors /
/// aliases on the JSON side), so the round-trip via the canonical
/// serializers is lossless.
fn json_to_yaml(v: &serde_json::Value) -> Result<serde_yaml::Value, String> {
    let s = serde_json::to_string(v).map_err(|e| e.to_string())?;
    serde_yaml::from_str(&s).map_err(|e| e.to_string())
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
    use serde_yaml::Value as YamlValue;
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
    fn outer_package_renders_inner_synchronously() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "inner.k", INNER);
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

resources = pkg.render({ path = "./inner.k", inputs = { name = "from-outer" } })"#,
        );

        let outer = PackageK::load(&outer_path).expect("load outer");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render outer");

        assert_eq!(rendered.resources.len(), 1);
        let cm = &rendered.resources[0];
        assert_eq!(cm["kind"], YamlValue::String("ConfigMap".into()));
        assert_eq!(
            cm["metadata"]["name"],
            YamlValue::String("from-outer".into())
        );
    }

    /// Closes spike-1 issue #1: the patched-sentinel case the
    /// previous deferred-expansion mechanism could only fail-loud on
    /// now works because the plugin returns a real list.
    #[test]
    fn list_comprehension_patches_apply_to_pkg_render_output() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "inner.k", INNER);
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

_up = pkg.render({ path = "./inner.k" })
resources = [r | {metadata.labels = {"patched" = "yes"}} for r in _up]"#,
        );

        let outer = PackageK::load(&outer_path).expect("load outer");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render outer");

        assert_eq!(rendered.resources.len(), 1);
        let cm = &rendered.resources[0];
        assert_eq!(
            cm["metadata"]["labels"]["patched"],
            YamlValue::String("yes".into()),
            "list-comprehension overlay should apply now that pkg.render returns a real list"
        );
    }

    /// Filtering on the result also works — the use case the
    /// sentinel mechanism couldn't express at all.
    #[test]
    fn filter_expression_works_on_pkg_render_output() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "multi.k",
            r#"
resources = [
    {apiVersion: "v1", kind: "ConfigMap", metadata.name: "keep-me"},
    {apiVersion: "v1", kind: "Secret", metadata.name: "drop-me"},
]"#,
        );
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

_all = pkg.render({ path = "./multi.k" })
resources = [r for r in _all if r.kind == "ConfigMap"]"#,
        );

        let outer = PackageK::load(&outer_path).expect("load outer");
        let rendered = outer
            .render(&YamlValue::Mapping(Default::default()))
            .expect("render outer");

        assert_eq!(rendered.resources.len(), 1);
        assert_eq!(
            rendered.resources[0]["metadata"]["name"],
            YamlValue::String("keep-me".into())
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
    fn nested_pkg_render_recurses() {
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
        // The innermost render received `name = "deep-from-middle"`
        // through two layers of pkg.render — proves the input flow.
        assert_eq!(
            rendered.resources[0]["metadata"]["name"],
            YamlValue::String("deep-from-middle".into())
        );
    }
}
