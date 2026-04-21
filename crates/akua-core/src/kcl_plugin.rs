//! Bridge between akua-authored engine callables (helm.template,
//! kustomize.build, rgd.instantiate, pkg.render) and KCL's in-process
//! plugin mechanism.
//!
//! ## Architecture
//!
//! KCL's evaluator invokes plugins via a single C-ABI function
//! pointer. When `KclServiceImpl.plugin_agent: u64` is non-zero, the
//! evaluator interprets it as the address of a function with the
//! signature
//!
//! ```ignore
//! extern "C-unwind" fn(
//!     method: *const c_char,         // "kcl_plugin.<module>.<fn>"
//!     args_json: *const c_char,      // "[positional, args]"
//!     kwargs_json: *const c_char,    // "{key: value}"
//! ) -> *const c_char                 // JSON-serialized return value
//! ```
//!
//! and calls it once per plugin invocation in the KCL program. We
//! register exactly one such function — [`dispatch`] — and route each
//! call to a handler looked up from a global [`PluginRegistry`].
//!
//! Handlers are typed `fn(args, kwargs) -> Result<Value, String>`
//! where everything flows via `serde_json::Value`. This keeps the FFI
//! boundary minimal: JSON in, JSON out, no KCL-specific types leak to
//! Rust callers.
//!
//! ## Memory ownership
//!
//! The `*const c_char` returned to KCL is allocated via
//! [`CString::into_raw`] and leaked — KCL reads it, converts to a KCL
//! value, and never frees. Matches upstream KCL's Python-plugin path.
//! Bound per process: `O(plugin_calls × payload_size)`. A one-shot
//! `akua render` invocation is fine; long-lived `akua dev` watchers
//! should re-exec periodically or use a subprocess render.

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::panic::AssertUnwindSafe;
use std::sync::{OnceLock, RwLock};

use serde_json::Value;

/// Typed handler signature. `args` is a JSON array of positional
/// arguments; `kwargs` is a JSON object. Return shape is whatever
/// makes sense for the plugin — KCL decodes the JSON back into its
/// own value tree.
pub type PluginHandler =
    Box<dyn Fn(&Value, &Value) -> Result<Value, String> + Send + Sync + 'static>;

/// Process-global plugin registry. One per process — KCL's FFI
/// accepts a single function pointer so we multiplex inside it.
fn registry() -> &'static RwLock<HashMap<String, PluginHandler>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, PluginHandler>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a handler under `method` (e.g. `"helm.template"` — without
/// the `kcl_plugin.` prefix). Later registrations overwrite earlier
/// ones for the same name; tests rely on this.
pub fn register(
    method: impl Into<String>,
    handler: impl Fn(&Value, &Value) -> Result<Value, String> + Send + Sync + 'static,
) {
    registry()
        .write()
        .expect("plugin registry poisoned")
        .insert(method.into(), Box::new(handler));
}

/// Remove a handler. Returns true if one was present.
pub fn unregister(method: &str) -> bool {
    registry()
        .write()
        .expect("plugin registry poisoned")
        .remove(method)
        .is_some()
}

/// Install every built-in engine callable whose crate feature is
/// enabled. Runs exactly once per process — guarded by an internal
/// `OnceLock` so repeat calls (e.g. every `package_k::render`
/// invocation in an `akua dev` watch loop) don't contend the global
/// registry's write-lock.
///
/// Currently registers:
///
/// - `helm.template` — when `engine-helm-shell` is on.
///
/// Future (kustomize.build, rgd.instantiate, pkg.render) will plug
/// in here as their feature flags and engines land.
pub fn install_builtin_plugins() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        #[cfg(feature = "engine-helm-shell")]
        crate::helm::install();
    });
}

/// The address of [`dispatch`] as a `u64`, suitable for the
/// `KclServiceImpl.plugin_agent` field. `0` disables plugins; any
/// non-zero value is treated as a function pointer by the KCL
/// evaluator.
pub fn plugin_agent_ptr() -> u64 {
    dispatch as *const () as usize as u64
}

/// The single `extern "C-unwind"` dispatcher KCL calls. Parses the
/// C-string arguments, looks up the handler, serializes the return.
///
/// On any error — unknown method, invalid JSON, handler failure — we
/// return a JSON object of shape `{"__kcl_PanicInfo__": "<message>"}`
/// which KCL's plugin-invoke glue treats as a runtime panic, bubbling
/// up through `api.exec_program` as a normal evaluation error.
/// # Safety
///
/// `method` must be a non-null null-terminated C string. `args_json`
/// and `kwargs_json` may be null (we treat null as an empty JSON
/// container). Pointers must remain valid for the duration of the
/// call. KCL's evaluator satisfies all of this from its own
/// allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn dispatch(
    method: *const c_char,
    args_json: *const c_char,
    kwargs_json: *const c_char,
) -> *const c_char {
    // Every path out of `dispatch` crosses the FFI boundary back into
    // KCL. A panic here would unwind through frames KCL doesn't
    // guarantee are unwind-safe (allocator state, plugin-handler
    // lock, evaluator bookkeeping). Catch everything and convert to
    // the `__kcl_PanicInfo__` envelope KCL already treats as a
    // runtime panic — same UX, no cross-runtime unwinding.
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let method = unsafe { c_str_required(method) };
        let args_s = unsafe { c_str_or_default(args_json, "[]") };
        let kwargs_s = unsafe { c_str_or_default(kwargs_json, "{}") };
        invoke(&method, &args_s, &kwargs_s)
    }));

    let payload = match result {
        Ok(Ok(value)) => value.to_string(),
        Ok(Err(msg)) => panic_envelope(&msg),
        Err(panic_payload) => panic_envelope(&panic_message(panic_payload)),
    };

    // Leaked on purpose — KCL consumes + never frees. Upstream
    // behaviour.
    CString::new(payload)
        .expect("payload contains interior NUL byte")
        .into_raw()
}

fn panic_envelope(msg: &str) -> String {
    serde_json::json!({ "__kcl_PanicInfo__": msg }).to_string()
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "plugin handler panicked".to_string())
}

fn invoke(method: &str, args_json: &str, kwargs_json: &str) -> Result<Value, String> {
    // KCL sends `"kcl_plugin.helm.template"` as the method name.
    // Users register `"helm.template"` — strip the prefix to match.
    let short = method.strip_prefix("kcl_plugin.").unwrap_or(method);

    let args: Value =
        serde_json::from_str(args_json).map_err(|e| format!("args not valid JSON: {e}"))?;
    let kwargs: Value =
        serde_json::from_str(kwargs_json).map_err(|e| format!("kwargs not valid JSON: {e}"))?;

    let guard = registry().read().expect("plugin registry poisoned");
    let handler = guard
        .get(short)
        .ok_or_else(|| format!("no plugin registered under `{short}`"))?;
    handler(&args, &kwargs)
}

/// Panic when `ptr` is null — KCL always sends a method name, so a
/// null here is a bridge bug we want surfaced loudly (the enclosing
/// `catch_unwind` turns the panic into a normal KCL runtime error).
unsafe fn c_str_required(ptr: *const c_char) -> String {
    assert!(!ptr.is_null(), "plugin dispatcher received null method name");
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// `args` / `kwargs` null falls back to the empty-container JSON that
/// KCL normally sends. Keeps downstream `serde_json::from_str` happy
/// regardless of whether KCL sent `[]`/`{}` explicitly or skipped the
/// field entirely.
unsafe fn c_str_or_default(ptr: *const c_char, fallback: &str) -> String {
    if ptr.is_null() {
        return fallback.to_string();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Register a plugin, call `dispatch` directly with C-compatible
    /// strings, verify round-trip JSON. Validates the FFI glue
    /// independent of KCL.
    #[test]
    fn dispatch_routes_to_registered_handler() {
        register("test_fixture.echo", |args, _| Ok(args.clone()));

        let method = CString::new("kcl_plugin.test_fixture.echo").unwrap();
        let args = CString::new(r#"["hello", 42]"#).unwrap();
        let kwargs = CString::new("{}").unwrap();

        let out = unsafe { dispatch(method.as_ptr(), args.as_ptr(), kwargs.as_ptr()) };
        let parsed: Value = parse_leaked_cstring(out);
        assert_eq!(parsed, serde_json::json!(["hello", 42]));

        unregister("test_fixture.echo");
    }

    #[test]
    fn unknown_method_surfaces_as_kcl_panic_info() {
        let method = CString::new("kcl_plugin.test_fixture.nope").unwrap();
        let args = CString::new("[]").unwrap();
        let kwargs = CString::new("{}").unwrap();

        let out = unsafe { dispatch(method.as_ptr(), args.as_ptr(), kwargs.as_ptr()) };
        let parsed: Value = parse_leaked_cstring(out);
        // KCL looks for this exact key to detect plugin-side panics.
        assert!(parsed["__kcl_PanicInfo__"]
            .as_str()
            .unwrap()
            .contains("no plugin registered"));
    }

    #[test]
    fn handler_error_surfaces_as_kcl_panic_info() {
        register("test_fixture.fail", |_, _| {
            Err("deliberate test failure".into())
        });
        let method = CString::new("kcl_plugin.test_fixture.fail").unwrap();
        let args = CString::new("[]").unwrap();
        let kwargs = CString::new("{}").unwrap();
        let out = unsafe { dispatch(method.as_ptr(), args.as_ptr(), kwargs.as_ptr()) };
        let parsed: Value = parse_leaked_cstring(out);
        assert_eq!(parsed["__kcl_PanicInfo__"], "deliberate test failure");
        unregister("test_fixture.fail");
    }

    #[test]
    fn panicking_handler_is_caught_and_surfaced_as_kcl_panic_info() {
        register("test_fixture.boom", |_, _| panic!("kaboom"));
        let method = CString::new("kcl_plugin.test_fixture.boom").unwrap();
        let args = CString::new("[]").unwrap();
        let kwargs = CString::new("{}").unwrap();
        let out = unsafe { dispatch(method.as_ptr(), args.as_ptr(), kwargs.as_ptr()) };
        let parsed: Value = parse_leaked_cstring(out);
        assert_eq!(parsed["__kcl_PanicInfo__"], "kaboom");
        unregister("test_fixture.boom");
    }

    #[test]
    fn plugin_agent_ptr_is_nonzero_and_stable() {
        let a = plugin_agent_ptr();
        let b = plugin_agent_ptr();
        assert_ne!(a, 0, "ptr must be non-zero so KCL takes it seriously");
        assert_eq!(a, b, "dispatch is a single fn — ptr should be stable");
    }

    /// Reclaim a `*const c_char` that `dispatch` leaked for KCL. Used
    /// only by these unit tests — KCL never reclaims in production.
    fn parse_leaked_cstring(ptr: *const c_char) -> Value {
        assert!(!ptr.is_null(), "dispatch returned null");
        // SAFETY: `ptr` came from `CString::into_raw`, so retaking
        // ownership with `from_raw` is correct — mirrors the malloc /
        // free ownership transfer idiom.
        let owned = unsafe { CString::from_raw(ptr as *mut c_char) };
        serde_json::from_slice(owned.as_bytes()).expect("dispatch output is JSON")
    }

    /// End-to-end: render a real Package.k that imports a plugin, call
    /// the plugin, observe the return value in the rendered output.
    /// Proves the `plugin_agent` → `dispatch` → handler → KCL-value
    /// round-trip works through the high-level API.
    #[test]
    fn package_k_can_call_a_registered_plugin() {
        use crate::PackageK;
        use serde_yaml::Value as YamlValue;
        use tempfile::TempDir;

        register("greet.hello", |args, _| {
            let name = args
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("world");
            Ok(serde_json::json!(format!("hello, {name}")))
        });

        let src = r#"
import kcl_plugin.greet

input = option("input") or {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "plugin-demo"
    data.greeting: greet.hello("akua")
}]

outputs = [{ kind: "RawManifests", target: "./" }]
"#;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("package.k");
        std::fs::write(&path, src).unwrap();

        let pkg = PackageK::load(&path).expect("load");
        let rendered = pkg.render(&YamlValue::Mapping(Default::default())).expect("render");
        assert_eq!(rendered.resources.len(), 1);
        let greeting = rendered.resources[0]
            .get("data")
            .and_then(|d| d.get("greeting"))
            .and_then(|g| g.as_str())
            .expect("greeting field");
        assert_eq!(greeting, "hello, akua");

        unregister("greet.hello");
    }
}
