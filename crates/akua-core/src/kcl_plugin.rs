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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
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

/// Pull the single Options schema instance every akua plugin expects
/// out of `args[0]` — returns it as a JSON map, ready for field
/// lookups. Shared error wording: `"{plugin}: arg 0 must be a
/// {schema} options object"`.
///
/// Every `akua.*` stdlib wrapper calls its plugin as
/// `_plugin.foo(opts)` where `opts` is a schema instance. KCL
/// serializes that as a single-element JSON array containing one
/// object — this helper unwraps it uniformly.
pub fn extract_options_arg<'a>(
    args: &'a Value,
    plugin_name: &str,
    schema_name: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    args.as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{plugin_name}: arg 0 must be a {schema_name} options object"))
}

/// Remove a handler. Returns true if one was present.
pub fn unregister(method: &str) -> bool {
    registry()
        .write()
        .expect("plugin registry poisoned")
        .remove(method)
        .is_some()
}

/// Install every built-in engine callable. Runs exactly once per
/// process — guarded by an internal `OnceLock` so repeat calls (e.g.
/// every `package_k::render` invocation in an `akua dev` watch loop)
/// don't contend the global registry's write-lock.
///
/// Currently registers:
///
/// - `pkg.render` — pure Rust, always available.
/// - `helm.template` — typed-error stub until `helm-engine-wasm` lands
///   (see docs/roadmap.md Phase 1). Returns `E_ENGINE_NOT_AVAILABLE`.
/// - `kustomize.build` — typed-error stub until `kustomize-engine-wasm`
///   lands (see docs/roadmap.md Phase 3).
///
/// Per CLAUDE.md, these will never be served by a shell-out to an
/// external binary. The only supported backend is a wasmtime-hosted
/// wasip1 engine compiled from the upstream Go source.
pub fn install_builtin_plugins() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        crate::pkg_render::install();
        #[cfg(feature = "engine-helm")]
        crate::helm::install();
        #[cfg(not(feature = "engine-helm"))]
        register_engine_stub("helm.template", "helm-engine-wasm (Phase 1)");
        #[cfg(feature = "engine-kustomize")]
        crate::kustomize::install();
        #[cfg(not(feature = "engine-kustomize"))]
        register_engine_stub("kustomize.build", "kustomize-engine-wasm (Phase 3)");
    });
}

/// Register a typed-error stub for an engine callable whose WASM
/// backend hasn't shipped yet. Packages that call the plugin get a
/// clear error pointing at the roadmap, not a silent pass-through.
fn register_engine_stub(plugin_name: &'static str, roadmap_ref: &'static str) {
    register(plugin_name, move |_args, _kwargs| {
        Err(format!(
            "{plugin_name}: engine not available — waiting on {roadmap_ref}. \
             akua ships only sandboxed wasmtime-hosted engines; shell-out is prohibited. \
             See docs/security-model.md + docs/roadmap.md."
        ))
    });
}

/// The address of [`dispatch`] as a `u64`, suitable for the
/// `KclServiceImpl.plugin_agent` field. `0` disables plugins; any
/// non-zero value is treated as a function pointer by the KCL
/// evaluator.
pub fn plugin_agent_ptr() -> u64 {
    dispatch as *const () as usize as u64
}

/// One render frame on the thread-local stack: the Package being
/// rendered + any additional absolute roots plugin paths are allowed
/// to resolve under. The second half is populated by
/// `package_k::render_with_charts` with the resolved chart directories
/// so `helm.template(nginx.path, ...)` (an absolute path originating
/// from the akua resolver) isn't rejected by the path-escape guard.
#[derive(Debug)]
struct RenderFrame {
    package: PathBuf,
    allowed_roots: Vec<PathBuf>,
    /// `--strict` semantics. When true, plugin paths must come from
    /// a typed `charts.*` import (i.e. resolve under one of the
    /// `allowed_roots`) — relative and raw-string paths under the
    /// Package dir are rejected. Forces authors to declare every
    /// chart in `akua.toml`, giving `akua.lock` full coverage.
    strict: bool,
}

thread_local! {
    /// Stack of active render frames on this thread. `PackageK::render`
    /// pushes on entry and pops on exit via [`RenderScope`]. Plugin
    /// handlers read the top to resolve user-supplied paths (helm
    /// chart dirs, nested package refs) against the Package that
    /// called them instead of against the process cwd.
    static RENDER_STACK: RefCell<Vec<RenderFrame>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that pushes a render frame on construction and pops it
/// on drop. Must be held for the duration of the KCL evaluation —
/// once dropped, the Package is no longer reachable via
/// [`current_package_dir`] or [`is_rendering`].
#[must_use = "holding the guard is how the package stays on the render stack"]
pub struct RenderScope {
    // Marker only; the push/pop bookkeeping lives on the thread-local.
    _private: (),
}

impl RenderScope {
    /// Push `package` onto the current-thread render stack with an
    /// empty allowed-roots list — plugin paths must resolve under
    /// `package.parent()`.
    pub fn enter(package: &Path) -> Self {
        Self::enter_with(package, &[], false)
    }

    /// Push `package` with resolved-chart roots + strict flag. Crate-
    /// private: only `package_k::render_with_charts` is allowed to
    /// register sandbox-escape roots — exposing this publicly would
    /// be a security surface since the plugin-path guard defers to
    /// whatever the top-of-stack frame permits.
    pub(crate) fn enter_with(package: &Path, allowed_roots: &[PathBuf], strict: bool) -> Self {
        RENDER_STACK.with(|s| {
            s.borrow_mut().push(RenderFrame {
                package: package.to_path_buf(),
                allowed_roots: allowed_roots.to_vec(),
                strict,
            });
        });
        Self { _private: () }
    }
}

impl Drop for RenderScope {
    fn drop(&mut self) {
        RENDER_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Parent directory of the top-of-stack Package.k. Plugin handlers
/// should resolve relative user-supplied paths against this so
/// `helm.template("./chart", ...)` Just Works regardless of which
/// directory the caller ran `akua render` from.
///
/// Returns `None` when no render is active (plugin called outside a
/// `PackageK::render` scope — rare, mostly tests).
pub fn current_package_dir() -> Option<PathBuf> {
    RENDER_STACK.with(|s| {
        s.borrow()
            .last()
            .and_then(|f| f.package.parent().map(Path::to_path_buf))
    })
}

/// `true` if `package` is already on the render stack. Used by
/// recursive plugins (`pkg.render`) to reject cycles before they
/// cause infinite recursion.
pub fn is_rendering(package: &Path) -> bool {
    RENDER_STACK.with(|s| s.borrow().iter().any(|f| f.package == package))
}

/// Absolute roots registered for the top-of-stack frame — the
/// resolved chart paths `render_with_charts` pushed. Empty when the
/// caller used `RenderScope::enter` directly (tests, legacy paths).
fn current_allowed_roots() -> Vec<PathBuf> {
    RENDER_STACK.with(|s| {
        s.borrow()
            .last()
            .map(|f| f.allowed_roots.clone())
            .unwrap_or_default()
    })
}

/// Whether the top-of-stack render frame is in strict mode. Plugin
/// handlers consult this to reject raw-string plugin paths (i.e.
/// paths that don't come from a resolved `charts.*` import).
fn current_strict() -> bool {
    RENDER_STACK.with(|s| s.borrow().last().map(|f| f.strict).unwrap_or(false))
}

/// Error from [`resolve_in_package`] when a plugin's user-supplied
/// path escapes the Package directory. Surfaced to Packages as a
/// parse error; to CLI callers with [`codes::E_PATH_ESCAPE`].
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("plugin path `{requested}` resolved to `{resolved}`, which escapes the Package directory `{package_dir}`")]
    Escape {
        requested: PathBuf,
        resolved: PathBuf,
        package_dir: PathBuf,
    },

    #[error("plugin path `{0}` is absolute; Package-relative paths only")]
    AbsoluteDisallowed(PathBuf),

    #[error("plugin path `{0}` isn't a typed `charts.*` import; strict mode requires every chart to be declared in `akua.toml`")]
    StrictRequiresTypedImport(PathBuf),

    #[error("no render scope — plugin called outside `PackageK::render`")]
    NoRenderScope,

    #[error("i/o resolving `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve a user-supplied plugin path against the current render
/// frame. Relative paths resolve under the Package dir; absolute paths
/// are rejected **unless** they resolve under one of the frame's
/// registered allowed roots — which is how resolved `charts.*` dep
/// paths reach `helm.template` without escaping the sandbox. Symlinks
/// are followed via `canonicalize` so a crafted `./link → /etc` fails
/// the under-dir check. Missing render scope is an error — every
/// plugin handler runs inside [`RenderScope`] by construction.
///
/// See docs/security-model.md for the full threat model.
pub fn resolve_in_package(path: &Path) -> Result<PathBuf, PathError> {
    let package_dir = current_package_dir().ok_or(PathError::NoRenderScope)?;
    let allowed_roots = current_allowed_roots();
    let strict = current_strict();

    if path.is_absolute() {
        let canon = canonicalize_best_effort(path).map_err(|e| PathError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        for root in &allowed_roots {
            if canon.starts_with(root) {
                return Ok(canon);
            }
        }
        return Err(PathError::AbsoluteDisallowed(path.to_path_buf()));
    }

    // Strict mode: relative plugin paths aren't allowed — force the
    // Package author to declare every chart in `akua.toml` and import
    // it as `charts.<name>`. The only accepted inputs are absolute
    // paths that land under a resolved-chart root (handled above).
    if strict {
        return Err(PathError::StrictRequiresTypedImport(path.to_path_buf()));
    }

    let pkg_canon = package_dir
        .canonicalize()
        .map_err(|e| PathError::Io {
            path: package_dir.clone(),
            source: e,
        })?;

    let joined = package_dir.join(path);
    // Canonicalize resolves `..` + symlinks to a real path under the
    // filesystem. If the target doesn't exist yet, walk up to the
    // nearest ancestor that does and canonicalize that — plugins are
    // allowed to pass paths to files they'll create.
    let resolved = canonicalize_best_effort(&joined).map_err(|e| PathError::Io {
        path: joined.clone(),
        source: e,
    })?;

    if !resolved.starts_with(&pkg_canon) {
        return Err(PathError::Escape {
            requested: path.to_path_buf(),
            resolved,
            package_dir: pkg_canon,
        });
    }
    Ok(resolved)
}

/// Canonicalize `p` if it exists; otherwise canonicalize the closest
/// existing ancestor and append the remaining components. Needed so
/// path-traversal validation works for plugin paths that point at
/// files the caller will create at render time.
fn canonicalize_best_effort(p: &Path) -> std::io::Result<PathBuf> {
    if let Ok(c) = std::fs::canonicalize(p) {
        return Ok(c);
    }
    let mut cur = p.to_path_buf();
    let mut tail = PathBuf::new();
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            tail = if tail.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                PathBuf::from(name).join(&tail)
            };
        }
        if let Ok(canon) = std::fs::canonicalize(parent) {
            return Ok(canon.join(tail));
        }
        cur = parent.to_path_buf();
        if cur.as_os_str().is_empty() {
            break;
        }
    }
    // Fallback: no ancestor canonicalizes (empty / broken fs) — surface
    // the first canonicalize error so the caller sees a useful message.
    std::fs::canonicalize(p)
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
    fn render_scope_exposes_package_dir_and_pops_on_drop() {
        assert!(current_package_dir().is_none());
        let pkg = std::path::Path::new("/tmp/ws/package.k");
        {
            let _scope = RenderScope::enter(pkg);
            assert_eq!(
                current_package_dir().as_deref(),
                Some(std::path::Path::new("/tmp/ws"))
            );
            assert!(is_rendering(pkg));
        }
        assert!(current_package_dir().is_none());
        assert!(!is_rendering(pkg));
    }

    #[test]
    fn render_scopes_nest() {
        let outer = std::path::Path::new("/tmp/outer/package.k");
        let inner = std::path::Path::new("/tmp/outer/nested/package.k");
        let _o = RenderScope::enter(outer);
        assert_eq!(
            current_package_dir().as_deref(),
            Some(std::path::Path::new("/tmp/outer"))
        );
        {
            let _i = RenderScope::enter(inner);
            assert_eq!(
                current_package_dir().as_deref(),
                Some(std::path::Path::new("/tmp/outer/nested"))
            );
            assert!(is_rendering(outer));
            assert!(is_rendering(inner));
        }
        // Inner popped; outer still active.
        assert_eq!(
            current_package_dir().as_deref(),
            Some(std::path::Path::new("/tmp/outer"))
        );
        assert!(!is_rendering(inner));
    }

    #[test]
    fn resolve_in_package_rejects_outside_scope_requests() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let pkg_file = tmp.path().join("package.k");
        std::fs::write(&pkg_file, "").unwrap();

        // No scope: rejected.
        let err = resolve_in_package(std::path::Path::new("chart")).unwrap_err();
        assert!(matches!(err, PathError::NoRenderScope));

        let _scope = RenderScope::enter(&pkg_file);

        // Relative under dir: OK. `canonicalize` resolves to the real
        // path; we compare by `ends_with` since macOS /tmp is a symlink.
        let chart = tmp.path().join("chart");
        std::fs::create_dir(&chart).unwrap();
        let ok = resolve_in_package(std::path::Path::new("./chart")).unwrap();
        assert!(ok.ends_with("chart"));

        // Parent traversal: rejected.
        let err = resolve_in_package(std::path::Path::new("../outside")).unwrap_err();
        assert!(matches!(err, PathError::Escape { .. }), "got: {err:?}");

        // Absolute: rejected.
        let err = resolve_in_package(std::path::Path::new("/etc/passwd")).unwrap_err();
        assert!(matches!(err, PathError::AbsoluteDisallowed(_)), "got: {err:?}");
    }

    #[test]
    fn resolve_in_package_rejects_symlink_escape() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let pkg_dir = tmp.path().join("pkg");
        std::fs::create_dir(&pkg_dir).unwrap();
        let pkg_file = pkg_dir.join("package.k");
        std::fs::write(&pkg_file, "").unwrap();

        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();

        // Plant a symlink `pkg/link → ../outside`.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, pkg_dir.join("link")).unwrap();

        let _scope = RenderScope::enter(&pkg_file);

        #[cfg(unix)]
        {
            let err = resolve_in_package(std::path::Path::new("./link")).unwrap_err();
            assert!(matches!(err, PathError::Escape { .. }), "got: {err:?}");
        }
    }

    #[test]
    fn is_rendering_detects_self_reentry() {
        let pkg = std::path::Path::new("/tmp/a/package.k");
        assert!(!is_rendering(pkg));
        let _outer = RenderScope::enter(pkg);
        assert!(is_rendering(pkg));
        {
            let _inner = RenderScope::enter(pkg);
            // Doubly pushed — cycle detection at the outer call site
            // should reject before this happens; the thread-local
            // tolerates it but `is_rendering` stays true either way.
            assert!(is_rendering(pkg));
        }
        assert!(is_rendering(pkg), "outer scope still active");
    }

    #[test]
    fn render_stack_is_thread_local() {
        let pkg = std::path::Path::new("/tmp/outer/package.k");
        let _scope = RenderScope::enter(pkg);
        assert!(is_rendering(pkg));

        // A child thread sees an empty stack — the parent's scope
        // doesn't leak across thread boundaries.
        let child_seen = std::thread::spawn(|| {
            let empty = current_package_dir().is_none();
            let not_rendering = !is_rendering(std::path::Path::new("/tmp/outer/package.k"));
            (empty, not_rendering)
        })
        .join()
        .expect("child thread");
        assert!(child_seen.0, "child saw a non-empty render stack");
        assert!(child_seen.1, "child thought /tmp/outer was rendering");
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
