//! File-watch + hot-rerender loop for `akua dev`.
//!
//! Author workflow: run `akua dev` in a terminal, edit any `.k` /
//! `.yaml` / `.toml` / chart file in the workspace, see the render
//! verdict update within a debounce tick. No polling — `notify`
//! uses platform-native inotify/kqueue/FSEvents, so idle CPU cost
//! is ~zero.
//!
//! Scope (Phase 8 slice):
//!
//! - Watch the workspace recursively.
//! - Debounce rapid bursts (editors save in multiple steps: .swp,
//!   truncate-write, fsync). 200ms default matches the human "save
//!   and check the terminal" cadence.
//! - Filter: only `.k` / `.yaml` / `.yml` / `.toml` / `.json` /
//!   chart-template extensions trigger re-render. Everything else
//!   (editor tempfiles, screenshots, VCS metadata) is ignored.
//! - Per cycle: call the caller-supplied render closure, emit a
//!   single [`DevEvent`] describing the outcome.
//! - Stop: when the caller-supplied `should_stop` predicate
//!   returns `true`. Lets the CLI layer hook SIGINT without this
//!   module depending on signal crates.

use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use serde::Serialize;

use crate::walk::should_skip_dir;

/// What an iteration of the watch loop emits. Serialized as JSON
/// lines when the caller is in agent mode; human-mode callers
/// stringify it themselves.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum DevEvent {
    /// Initial render fired immediately on startup — users expect
    /// their first edit+save to show a diff, so we seed with the
    /// current state rather than waiting for a change.
    Started {
        workspace: PathBuf,
        /// Human-readable summary of the initial render.
        summary: String,
    },
    /// Debounced change → render ran. `took_ms` helps authors tell
    /// when a Package is getting slow + should be optimized.
    Rendered {
        /// Paths that triggered this cycle (post-filter).
        changed: Vec<PathBuf>,
        took_ms: u128,
        summary: String,
    },
    /// Render failed. Loop keeps running — the author is likely
    /// mid-edit.
    RenderError {
        changed: Vec<PathBuf>,
        message: String,
    },
    /// `should_stop` returned true — loop is about to exit cleanly.
    Stopped,
}

#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error("setting up watcher for `{}`: {source}", path.display())]
    Watcher {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

/// Start the watch-render loop. Blocks on the current thread until
/// `should_stop()` returns `true`. `render_once` is invoked
/// synchronously per debounced change batch; it returns a short
/// summary string that rides along in [`DevEvent::Rendered`].
///
/// `emit` sinks every event — typically the CLI verb's
/// `emit_output` wrapper. Split from the render closure so tests
/// can stub both independently.
pub fn watch_and_render<R, E, S>(
    workspace: &Path,
    debounce: Duration,
    mut render_once: R,
    mut emit: E,
    mut should_stop: S,
) -> Result<(), DevError>
where
    R: FnMut(&[PathBuf]) -> Result<String, String>,
    E: FnMut(&DevEvent),
    S: FnMut() -> bool,
{
    // Seed: kick a render before the first event so `akua dev`
    // surfaces current state instead of looking idle until the
    // author saves.
    match render_once(&[]) {
        Ok(summary) => emit(&DevEvent::Started {
            workspace: workspace.to_path_buf(),
            summary,
        }),
        Err(message) => emit(&DevEvent::RenderError {
            changed: Vec::new(),
            message,
        }),
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(debounce, move |res| {
        let _ = tx.send(res);
    })
    .map_err(|source| DevError::Watcher {
        path: workspace.to_path_buf(),
        source,
    })?;

    // Watch per kept subdir non-recursively rather than
    // `RecursiveMode::Recursive` on the workspace root. A
    // monorepo's `target/` / `node_modules/` / `.git/` can
    // exhaust `fs.inotify.max_user_watches` before startup
    // finishes — skipping them here keeps the watch count
    // proportional to the source tree size. New top-level dirs
    // created during the session (rare) aren't auto-watched;
    // authors restart `akua dev` in that case.
    let dirs = crate::walk::collect_directories(workspace).map_err(|source| DevError::Watcher {
        path: workspace.to_path_buf(),
        source: notify::Error::io(source),
    })?;
    for dir in &dirs {
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|source| DevError::Watcher {
                path: dir.clone(),
                source,
            })?;
    }

    loop {
        if should_stop() {
            emit(&DevEvent::Stopped);
            return Ok(());
        }
        // Poll the channel with a short timeout so we check
        // `should_stop` regularly without pinning CPU. The
        // debouncer's own tick is independent — it buffers bursts
        // and flushes on `debounce` elapsed, delivering one
        // batched message per quiet period.
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(events)) => {
                let changed = filter_relevant(
                    events.into_iter().map(|e| e.path),
                    workspace,
                );
                if changed.is_empty() {
                    continue;
                }
                let start = Instant::now();
                match render_once(&changed) {
                    Ok(summary) => emit(&DevEvent::Rendered {
                        changed,
                        took_ms: start.elapsed().as_millis(),
                        summary,
                    }),
                    Err(message) => emit(&DevEvent::RenderError { changed, message }),
                }
            }
            Ok(Err(err)) => {
                // Transient watcher error (rename race, macOS
                // Spotlight churn). Not fatal — surface it and
                // keep looping.
                emit(&DevEvent::RenderError {
                    changed: Vec::new(),
                    message: err.to_string(),
                });
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                emit(&DevEvent::Stopped);
                return Ok(());
            }
        }
    }
}

/// Filter raw change paths down to the set that should trigger a
/// re-render. Skips:
///
/// - Paths outside `workspace` (symlink leaks, rare).
/// - Directories inside [`should_skip_dir`] (`target/`, `.git/`,
///   `deploy/`, etc.).
/// - Files whose extension isn't one of our source types.
/// - Editor tempfiles (suffixes: `~`, `.swp`, `.swx`, `.tmp`).
///
/// Pure function — no FS I/O. Unit-testable without a watcher.
pub fn filter_relevant<I>(paths: I, workspace: &Path) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut out: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| is_relevant(p, workspace))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn is_relevant(path: &Path, workspace: &Path) -> bool {
    // Must live under the workspace after canonicalization logic
    // the caller already did (notify gives absolute paths).
    if !path.starts_with(workspace) {
        return false;
    }
    // Any ancestor component being a skip-dir → ignore. Covers
    // files nested at any depth under `target/`, `node_modules/`,
    // dotdirs, etc.
    for comp in path.components() {
        if let std::path::Component::Normal(name) = comp {
            let Some(s) = name.to_str() else { continue };
            if should_skip_dir(s) {
                return false;
            }
        }
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if is_editor_tempfile(name) {
        return false;
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext,
        "k" | "yaml" | "yml" | "json" | "toml" | "tpl" | "lock"
    )
}

fn is_editor_tempfile(name: &str) -> bool {
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name.starts_with(".#") // emacs lockfiles
        || name.starts_with("4913") // vim's backup probe file
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        PathBuf::from("/ws")
    }

    #[test]
    fn keeps_source_extensions() {
        assert!(is_relevant(&ws().join("package.k"), &ws()));
        assert!(is_relevant(&ws().join("inputs.yaml"), &ws()));
        assert!(is_relevant(&ws().join("akua.toml"), &ws()));
        assert!(is_relevant(&ws().join("akua.lock"), &ws()));
        assert!(is_relevant(
            &ws().join("vendor/nginx/templates/cm.yaml"),
            &ws()
        ));
        assert!(is_relevant(
            &ws().join("vendor/nginx/templates/_helpers.tpl"),
            &ws()
        ));
    }

    #[test]
    fn drops_non_source_extensions() {
        assert!(!is_relevant(&ws().join("README.md"), &ws()));
        assert!(!is_relevant(&ws().join("screenshot.png"), &ws()));
        assert!(!is_relevant(&ws().join("notes"), &ws()));
    }

    #[test]
    fn drops_editor_tempfiles() {
        assert!(!is_relevant(&ws().join("package.k~"), &ws()));
        assert!(!is_relevant(&ws().join(".package.k.swp"), &ws()));
        assert!(!is_relevant(&ws().join(".#package.k"), &ws()));
        assert!(!is_relevant(&ws().join("4913"), &ws()));
    }

    #[test]
    fn drops_skip_dir_children() {
        assert!(!is_relevant(&ws().join("target/debug/out.k"), &ws()));
        assert!(!is_relevant(&ws().join("node_modules/x/y.yaml"), &ws()));
        assert!(!is_relevant(&ws().join(".git/HEAD"), &ws()));
        assert!(!is_relevant(&ws().join("deploy/000.yaml"), &ws()));
        assert!(!is_relevant(&ws().join("rendered/x.yaml"), &ws()));
    }

    #[test]
    fn drops_paths_outside_workspace() {
        assert!(!is_relevant(&PathBuf::from("/other/package.k"), &ws()));
    }

    #[test]
    fn filter_relevant_dedups_and_sorts() {
        let inputs = vec![
            ws().join("b.k"),
            ws().join("a.k"),
            ws().join("a.k"), // duplicate from rapid save
            ws().join("target/skip.k"),
            ws().join("README.md"),
        ];
        let out = filter_relevant(inputs, &ws());
        assert_eq!(out, vec![ws().join("a.k"), ws().join("b.k")]);
    }

    // Short-running integration test: drive the watch loop with a
    // synthetic stop predicate so we can assert the Started event
    // fires without actually watching filesystem events. The
    // debouncer thread gets created but never receives an event
    // before `should_stop` fires.
    #[test]
    fn emits_started_then_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let events = std::sync::Mutex::new(Vec::new());
        let mut rendered = false;
        watch_and_render(
            tmp.path(),
            Duration::from_millis(50),
            |_changed| {
                rendered = true;
                Ok("rendered".to_string())
            },
            |e| events.lock().unwrap().push(e.clone()),
            {
                // Stop immediately on the 2nd check — loop body
                // runs once so we see Started then Stopped.
                let mut calls = 0;
                move || {
                    calls += 1;
                    calls > 1
                }
            },
        )
        .expect("watch loop");

        assert!(rendered, "render closure must fire once at startup");
        let seen = events.into_inner().unwrap();
        assert!(
            matches!(seen.first(), Some(DevEvent::Started { .. })),
            "first event: {:?}",
            seen.first()
        );
        assert!(
            matches!(seen.last(), Some(DevEvent::Stopped)),
            "last event: {:?}",
            seen.last()
        );
    }

    #[test]
    fn startup_render_error_still_enters_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let events = std::sync::Mutex::new(Vec::new());
        watch_and_render(
            tmp.path(),
            Duration::from_millis(50),
            |_changed| Err("broken package".to_string()),
            |e| events.lock().unwrap().push(e.clone()),
            {
                let mut calls = 0;
                move || {
                    calls += 1;
                    calls > 1
                }
            },
        )
        .expect("watch loop");

        let seen = events.into_inner().unwrap();
        assert!(
            matches!(seen.first(), Some(DevEvent::RenderError { .. })),
            "first event: {:?}",
            seen.first()
        );
        assert!(matches!(seen.last(), Some(DevEvent::Stopped)));
    }
}
