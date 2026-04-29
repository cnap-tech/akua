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
//! Upstream `kcl-runtime/src/stdlib/plugin.rs` holds
//! `PLUGIN_HANDLER_FN_PTR` across the user-supplied callback. A
//! plugin that re-entered KCL deadlocked on the same thread —
//! `std::sync::Mutex` isn't reentrant. akua carries a one-line patch
//! at `cnap-tech/kcl#akua-wasm32` (commit `d584c0bc`) that copies the
//! fn pointer out of the lock before invoking it, freeing the
//! reentrant call. Without that patch this design hangs.
//!
//! Cycle detection uses the thread-local render stack
//! [`crate::kcl_plugin::RenderScope`]: `pkg.render` of a path
//! already on the stack returns [`crate::package_k::PackageKError::Cycle`]
//! before the inner load.

use std::path::{Path, PathBuf};

use crate::{kcl_plugin, PackageK};

pub const PLUGIN_NAME: &str = "pkg.render";

const OPT_PACKAGE: &str = "package";
const OPT_PATH: &str = "path";
const OPT_INPUTS: &str = "inputs";

/// Prefix every error with the plugin name + the target path so
/// nested failures read as a stack (`pkg.render(a.k): pkg.render(b.k): …`)
/// rather than a repeated tag.
fn err_at(target: &Path, msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}({}): {msg}", target.display())
}

fn err(msg: impl std::fmt::Display) -> String {
    format!("{PLUGIN_NAME}: {msg}")
}

pub fn install() {
    kcl_plugin::register(PLUGIN_NAME, |args, _kwargs| {
        let opts = kcl_plugin::extract_options_arg(args, PLUGIN_NAME, "pkg.Render")?;
        let inputs_json = opts
            .get(OPT_INPUTS)
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // `PackageK::render` takes the same `serde_yaml::Value` shape that
        // `ctx.input()` flows through. `serde_yaml::to_value` walks the
        // serde_json::Value via serde directly — no string intermediate.
        let inputs_yaml: serde_yaml::Value =
            serde_yaml::to_value(&inputs_json).map_err(|e| err(format!("inputs: {e}")))?;

        // Resolve target package: prefer `package = "<alias>"` (typed,
        // declared in akua.toml); fall back to `path = "..."` for the
        // legacy form. CLAUDE.md "no filesystem paths in user-authored
        // KCL" — `path` is deprecated; lint surfaces a fix-it. One of
        // the two must be present.
        let target = if let Some(alias) = opts.get(OPT_PACKAGE).and_then(serde_json::Value::as_str)
        {
            resolve_by_alias(alias)?
        } else if let Some(path_str) = opts.get(OPT_PATH).and_then(serde_json::Value::as_str) {
            let raw = PathBuf::from(path_str);
            let resolved = kcl_plugin::resolve_in_package(&raw).map_err(|e| err(e.to_string()))?;
            resolve_package_file(&resolved)
        } else {
            return Err(err(format!(
                "options must set either `{OPT_PACKAGE} = \"<dep-alias>\"` or `{OPT_PATH} = \"<dir>\"`"
            )));
        };

        // Pre-render checks — cycle, depth cap, wall-clock — in one
        // pass over the render stack. Cycle rejects re-entry of a
        // Package already on the chain; depth + deadline cover the
        // remaining runaway shapes (unbounded fan-out through fresh
        // Packages; host-side eval spinning past the wasm epoch
        // deadline).
        let pre = kcl_plugin::pre_check(&target);
        if pre.cycle {
            return Err(err_at(
                &target,
                "cycle detected — already on the render stack",
            ));
        }
        if pre.depth >= pre.budget.max_depth {
            return Err(err_at(
                &target,
                format!(
                    "render depth limit ({}) exceeded — likely composition runaway",
                    pre.budget.max_depth
                ),
            ));
        }
        if let Some(deadline) = pre.budget.deadline {
            if std::time::Instant::now() >= deadline {
                return Err(err_at(
                    &target,
                    "wall-clock budget exhausted in nested render",
                ));
            }
        }

        // Load + render the inner Package. The recursion is bounded
        // by RenderScope (push on enter, pop on drop): even when the
        // inner Package itself calls pkg.render, the stack stays
        // balanced and the cycle check fires correctly.
        let pkg = PackageK::load(&target).map_err(|e| err_at(&target, e))?;
        let rendered = pkg.render(&inputs_yaml).map_err(|e| err_at(&target, e))?;

        // Convert back to serde_json — KCL's plugin contract returns
        // JSON, and the caller's `_up = pkg.render(...)` binding is
        // a real list of real dicts after this returns.
        let json_resources: Vec<serde_json::Value> = rendered
            .resources
            .into_iter()
            .map(|y| serde_json::to_value(y).map_err(|e| err_at(&target, e)))
            .collect::<Result<_, _>>()?;
        Ok(serde_json::Value::Array(json_resources))
    });
}

/// Accept either a directory (append `package.k`) or a direct file path.
fn resolve_package_file(resolved: &Path) -> PathBuf {
    if resolved.is_dir() {
        resolved.join("package.k")
    } else {
        resolved.to_path_buf()
    }
}

/// Look up an Akua-package dep alias against the current render frame's
/// resolved-deps map. Errors when the alias is missing, listing the
/// aliases the caller could have used so typos surface immediately.
fn resolve_by_alias(alias: &str) -> Result<PathBuf, String> {
    match kcl_plugin::resolve_pkg_alias(alias) {
        Some(dir) => Ok(resolve_package_file(&dir)),
        None => {
            let known = kcl_plugin::current_pkg_aliases();
            let hint = if known.is_empty() {
                String::from("none — declare it under `[dependencies]` in akua.toml")
            } else {
                format!("known: {}", known.join(", "))
            };
            Err(err(format!(
                "package `{alias}` is not in the current Package's dependencies ({hint})"
            )))
        }
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

    /// List-comprehension overlay applied to `pkg.render` output reaches
    /// the inner resources — the return is a real list, not a placeholder.
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
            YamlValue::String("yes".into())
        );
    }

    /// Filter expressions on `pkg.render` output preserve only the
    /// matching resources.
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

    /// Indirect cycle: A → B → A. The render stack must catch this
    /// the same way it catches A → A — RenderScope tracks every
    /// Package on the chain, not just the direct caller.
    #[test]
    fn detects_indirect_cycle_via_render_stack() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "a.k",
            r#"
import kcl_plugin.pkg

_b = pkg.render({ path = "./b.k" })
resources = _b"#,
        );
        write(
            tmp.path(),
            "b.k",
            r#"
import kcl_plugin.pkg

_a = pkg.render({ path = "./a.k" })
resources = _a"#,
        );

        let pkg = PackageK::load(&tmp.path().join("a.k")).expect("load");
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

    /// Two-level recursion: outer → middle → deep. Inputs flow
    /// through both layers, and the render stack stays balanced.
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
        assert_eq!(
            rendered.resources[0]["metadata"]["name"],
            YamlValue::String("deep-from-middle".into())
        );
    }

    /// Wall-clock budget that's already expired by the time the
    /// outer render starts trips on the first nested `pkg.render`
    /// call. Confirms the deadline propagates into the plugin
    /// handler via the inherited budget snapshot.
    #[test]
    fn budget_wall_clock_deadline_rejects_nested_render() {
        use kcl_plugin::BudgetSnapshot;
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "inner.k", INNER);
        let outer_path = write(
            tmp.path(),
            "outer.k",
            r#"
import kcl_plugin.pkg

resources = pkg.render({ path = "./inner.k" })"#,
        );

        // Deadline already in the past → first pkg.render call rejects.
        let budget = BudgetSnapshot {
            deadline: Some(Instant::now() - Duration::from_secs(1)),
            max_depth: BudgetSnapshot::DEFAULT_MAX_DEPTH,
        };
        let _outer_scope = kcl_plugin::RenderScope::enter_with_budget(&outer_path, budget);

        let outer = PackageK::load(&outer_path).expect("load outer");
        let err = outer
            .render(&YamlValue::Mapping(Default::default()))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("wall-clock budget exhausted"),
            "expected wall-clock rejection, got: {err}"
        );
    }

    /// Depth cap rejects unbounded fan-out through fresh Packages
    /// — cycle detection alone doesn't catch this because every
    /// inner Package is a different file.
    #[test]
    fn budget_depth_cap_rejects_runaway_recursion() {
        use kcl_plugin::BudgetSnapshot;

        // Build a chain where each level renders the next:
        //   level0.k → level1.k → level2.k → ... → levelN.k
        let tmp = TempDir::new().unwrap();
        let chain_len = 5;
        // Tail: a leaf with a literal resource list.
        write(
            tmp.path(),
            &format!("level{chain_len}.k"),
            "resources = [{apiVersion: \"v1\", kind: \"ConfigMap\", metadata.name: \"leaf\"}]\n",
        );
        for i in (0..chain_len).rev() {
            let next = format!("./level{}.k", i + 1);
            write(
                tmp.path(),
                &format!("level{i}.k"),
                &format!(
                    r#"
import kcl_plugin.pkg

resources = pkg.render({{ path = "{next}" }})"#
                ),
            );
        }

        // Cap depth at 3 — chain is 6 levels deep so the 3rd
        // pkg.render call must reject.
        let outer_path = tmp.path().join("level0.k");
        let budget = BudgetSnapshot {
            deadline: None,
            max_depth: 3,
        };
        let _outer_scope = kcl_plugin::RenderScope::enter_with_budget(&outer_path, budget);

        let outer = PackageK::load(&outer_path).expect("load");
        let err = outer
            .render(&YamlValue::Mapping(Default::default()))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("depth limit"),
            "expected depth-limit rejection, got: {err}"
        );
    }
}
