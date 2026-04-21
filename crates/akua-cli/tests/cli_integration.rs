//! End-to-end integration tests.
//!
//! These drive the compiled `akua` binary (not the library surface),
//! asserting exit codes, stdout JSON shapes, and stderr structured-
//! error payloads. Catches regressions the per-verb unit tests miss —
//! clap wiring, `main.rs` dispatch, exit-code propagation.

use std::path::Path;
use std::process::{Command, Output};

/// Path to the compiled `akua` binary, injected by Cargo at build time
/// for integration tests in `tests/`.
const AKUA_BIN: &str = env!("CARGO_BIN_EXE_akua");

/// Run `akua <args>` in `cwd` and return (exit-code, stdout, stderr).
///
/// The binary is forced out of agent-detection mode via
/// `AKUA_NO_AGENT_DETECT=1` so tests get deterministic text output —
/// each test then opts into JSON with `--json` where it wants to
/// assert the structured shape.
fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::new(AKUA_BIN)
        .current_dir(cwd)
        .env("AKUA_NO_AGENT_DETECT", "1")
        .args(args)
        .output()
        .expect("spawn akua binary")
}

fn assert_exit(output: &Output, expected: i32) {
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(
        code,
        expected,
        "expected exit {expected}, got {code}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn stdout_json(output: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().unwrap_or(-1);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n--- exit={code} ---\n--- stdout ---\n{text}\n--- stderr ---\n{stderr}"
        )
    })
}

fn tempdir() -> tempfile::TempDir {
    tempfile::TempDir::new().expect("tempdir")
}

// ---------------------------------------------------------------------------
// whoami / version
// ---------------------------------------------------------------------------

#[test]
fn whoami_json_carries_agent_context_and_version() {
    let dir = tempdir();
    let out = run(dir.path(), &["whoami", "--json"]);
    assert_exit(&out, 0);

    let parsed = stdout_json(&out);
    assert!(parsed.get("agent_context").is_some());
    assert!(parsed["version"].is_string());
    // AKUA_NO_AGENT_DETECT flips detection off.
    assert_eq!(parsed["agent_context"]["detected"], false);
}

#[test]
fn version_json_includes_binary_version() {
    let dir = tempdir();
    let out = run(dir.path(), &["version", "--json"]);
    assert_exit(&out, 0);
    let parsed = stdout_json(&out);
    assert!(parsed["version"].as_str().unwrap().len() >= 3);
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[test]
fn init_scaffolds_three_files_and_reports_them() {
    let dir = tempdir();
    let out = run(dir.path(), &["init", "demo", "--json"]);
    assert_exit(&out, 0);

    let parsed = stdout_json(&out);
    assert_eq!(parsed["name"], "demo");
    assert_eq!(parsed["files"].as_array().unwrap().len(), 3);

    let pkg = dir.path().join("demo");
    assert!(pkg.join("akua.toml").is_file());
    assert!(pkg.join("package.k").is_file());
    assert!(pkg.join("inputs.example.yaml").is_file());
}

#[test]
fn init_without_force_refuses_to_clobber() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]).status.success().then_some(()).unwrap();
    // Second run — same target — should fail with exit 1 (UserError).
    let out = run(dir.path(), &["init", "pkg"]);
    assert_exit(&out, 1);
}

// ---------------------------------------------------------------------------
// render — full init → render → on-disk assertion
// ---------------------------------------------------------------------------

#[test]
fn init_then_render_produces_deterministic_manifests() {
    let dir = tempdir();
    run(dir.path(), &["init", "app"]);

    let app = dir.path().join("app");
    let out = run(
        &app,
        &[
            "render",
            "--inputs",
            "inputs.example.yaml",
            "--out",
            "./deploy",
            "--json",
        ],
    );
    assert_exit(&out, 0);

    let parsed = stdout_json(&out);
    assert_eq!(parsed["outputs"][0]["manifests"], 1);
    assert_eq!(parsed["outputs"][0]["format"], "raw-manifests");
    assert!(parsed["outputs"][0]["hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    // The sole ConfigMap landed on disk under the scaffolded name.
    assert!(app.join("deploy").join("000-configmap-hello.yaml").is_file());
}

#[test]
fn init_then_render_without_inputs_flag_uses_scaffold_inputs_example() {
    // After `akua init`, the scaffold drops `inputs.example.yaml` next
    // to package.k. `akua render` without --inputs should auto-discover
    // it so the scaffold workflow is a single command.
    let dir = tempdir();
    run(dir.path(), &["init", "app"]);
    let app = dir.path().join("app");
    let out = run(&app, &["render", "--out", "./deploy", "--json"]);
    assert_exit(&out, 0);
    let parsed = stdout_json(&out);
    assert_eq!(parsed["outputs"][0]["manifests"], 1);
    assert!(app.join("deploy/000-configmap-hello.yaml").is_file());
}

#[test]
fn render_missing_package_surfaces_structured_error_on_stderr() {
    let dir = tempdir();
    let out = run(
        dir.path(),
        &["render", "--package", "does-not-exist.k", "--json"],
    );
    assert_exit(&out, 1);

    // In JSON mode, structured errors go to stderr as JSON-lines.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("structured error on stderr");
    assert_eq!(parsed["code"], "E_PACKAGE_MISSING");
}

// ---------------------------------------------------------------------------
// fmt
// ---------------------------------------------------------------------------

#[test]
fn fmt_check_exits_user_error_when_file_needs_reformatting() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]);
    let pkg = dir.path().join("pkg");

    // Overwrite with deliberately unformatted KCL.
    std::fs::write(pkg.join("package.k"), "schema Input:\n  x:int=1\n").unwrap();

    let out = run(&pkg, &["fmt", "--check", "--json"]);
    assert_exit(&out, 1);

    let parsed = stdout_json(&out);
    assert_eq!(parsed["files"][0]["changed"], true);

    // --check did not write.
    let after = std::fs::read_to_string(pkg.join("package.k")).unwrap();
    assert!(after.contains("x:int=1"));
}

#[test]
fn fmt_writes_back_and_second_run_is_clean() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]);
    let pkg = dir.path().join("pkg");
    std::fs::write(pkg.join("package.k"), "schema Input:\n  x:int=1\n").unwrap();

    assert_exit(&run(&pkg, &["fmt"]), 0);
    // Now the file is formatted; --check should pass.
    assert_exit(&run(&pkg, &["fmt", "--check"]), 0);
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

#[test]
fn lint_reports_ok_on_scaffold_and_fail_on_syntax_error() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]);
    let pkg = dir.path().join("pkg");

    assert_exit(&run(&pkg, &["lint"]), 0);

    std::fs::write(pkg.join("package.k"), "schema X:\n  !!!\n").unwrap();
    let out = run(&pkg, &["lint", "--json"]);
    assert_exit(&out, 1);
    let parsed = stdout_json(&out);
    assert_eq!(parsed["status"], "fail");
    assert!(!parsed["issues"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// inspect
// ---------------------------------------------------------------------------

#[test]
fn inspect_reports_input_option_from_scaffolded_package() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]);
    let pkg = dir.path().join("pkg");

    let out = run(&pkg, &["inspect", "--json"]);
    assert_exit(&out, 0);
    let parsed = stdout_json(&out);
    let opts = parsed["options"].as_array().unwrap();
    assert_eq!(opts.len(), 1);
    assert_eq!(opts[0]["name"], "input");
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

#[test]
fn check_passes_on_clean_workspace() {
    let dir = tempdir();
    run(dir.path(), &["init", "pkg"]);
    let pkg = dir.path().join("pkg");

    let out = run(&pkg, &["check", "--json"]);
    assert_exit(&out, 0);
    let parsed = stdout_json(&out);
    assert_eq!(parsed["status"], "ok");
    let names: Vec<&str> = parsed["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["manifest", "package"]);
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

#[test]
fn diff_two_identical_dirs_exits_zero() {
    let dir = tempdir();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("f.yaml"), "x").unwrap();
    std::fs::write(b.join("f.yaml"), "x").unwrap();

    let out = run(
        dir.path(),
        &["diff", a.to_str().unwrap(), b.to_str().unwrap()],
    );
    assert_exit(&out, 0);
}

#[test]
fn diff_two_different_dirs_exits_one_and_reports_changes() {
    let dir = tempdir();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("f.yaml"), "x").unwrap();
    std::fs::write(b.join("f.yaml"), "y").unwrap();
    std::fs::write(b.join("extra.yaml"), "z").unwrap();

    let out = run(
        dir.path(),
        &["diff", a.to_str().unwrap(), b.to_str().unwrap(), "--json"],
    );
    assert_exit(&out, 1);
    let parsed = stdout_json(&out);
    assert_eq!(parsed["changed"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["added"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// add / remove / tree / verify — full manifest lifecycle
// ---------------------------------------------------------------------------

#[test]
fn add_remove_tree_verify_lifecycle() {
    let dir = tempdir();
    run(dir.path(), &["init", "ws"]);
    let ws = dir.path().join("ws");

    // add
    let out = run(
        &ws,
        &[
            "add",
            "cnpg",
            "--oci",
            "oci://ghcr.io/cnpg/cluster",
            "--version",
            "0.20.0",
            "--json",
        ],
    );
    assert_exit(&out, 0);
    assert_eq!(stdout_json(&out)["replaced"], false);

    // tree (with no lockfile — just manifest-declared deps)
    let out = run(&ws, &["tree", "--json"]);
    assert_exit(&out, 0);
    let parsed = stdout_json(&out);
    assert_eq!(parsed["dependencies"][0]["name"], "cnpg");
    assert_eq!(parsed["dependencies"][0]["source"], "oci");
    assert_eq!(parsed["dependencies"][0]["version"], "0.20.0");

    // verify without akua.lock — with declared deps, this is a hard
    // error (E_LOCK_MISSING on stderr, exit 1) not a verdict. Once
    // the lockfile pipeline lands the assertion shape changes; for
    // now the structured error itself is the behaviour under test.
    let out = run(&ws, &["verify", "--json"]);
    assert_exit(&out, 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let structured: serde_json::Value = serde_json::from_str(stderr.trim()).expect("stderr json");
    assert_eq!(structured["code"], "E_LOCK_MISSING");

    // remove the dep — manifest now matches an empty lockfile
    // (still absent), so verify would still fail with E_LOCK_MISSING.
    // We just confirm the remove round-tripped.
    assert_exit(&run(&ws, &["remove", "cnpg"]), 0);
    let tree_after = stdout_json(&run(&ws, &["tree", "--json"]));
    assert!(tree_after["dependencies"].as_array().unwrap().is_empty());
}

#[test]
fn remove_missing_dep_errors_without_ignore_missing() {
    let dir = tempdir();
    run(dir.path(), &["init", "ws"]);
    let ws = dir.path().join("ws");

    let out = run(&ws, &["remove", "ghost"]);
    assert_exit(&out, 1);
}

#[test]
fn remove_missing_dep_is_noop_with_ignore_missing() {
    let dir = tempdir();
    run(dir.path(), &["init", "ws"]);
    let ws = dir.path().join("ws");

    let out = run(&ws, &["remove", "ghost", "--ignore-missing", "--json"]);
    assert_exit(&out, 0);
    assert_eq!(stdout_json(&out)["removed"], false);
}

// ---------------------------------------------------------------------------
// agent-context auto-detection
// ---------------------------------------------------------------------------

#[test]
fn agent_env_flips_output_mode_to_json_automatically() {
    // Run without --json but with CLAUDECODE=1 (one of the agent
    // triggers). Contract §1.5 says output switches to JSON.
    //
    // env_remove is a guard: if the developer's own shell has
    // AKUA_NO_AGENT_DETECT set, Command would inherit it and silently
    // defeat this test.
    let dir = tempdir();
    let out = Command::new(AKUA_BIN)
        .current_dir(dir.path())
        .env_remove("AKUA_NO_AGENT_DETECT")
        .env("CLAUDECODE", "1")
        .args(["version"])
        .output()
        .expect("spawn");
    assert_exit(&out, 0);

    let text = String::from_utf8_lossy(&out.stdout);
    // Auto-JSON mode means the output parses as JSON even though the
    // caller didn't ask for it.
    let parsed: serde_json::Value =
        serde_json::from_str(text.trim()).expect("agent-detected output should be JSON");
    assert!(parsed["version"].is_string());
}

// ---------------------------------------------------------------------------
// help + unknown-command routing
// ---------------------------------------------------------------------------

#[test]
fn help_lists_all_twelve_verbs() {
    let dir = tempdir();
    let out = run(dir.path(), &["--help"]);
    assert_exit(&out, 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for verb in [
        "init", "whoami", "version", "verify", "render", "fmt", "lint", "check", "diff", "add",
        "remove", "tree", "inspect",
    ] {
        assert!(stdout.contains(verb), "help missing verb `{verb}`\n{stdout}");
    }
}

#[test]
fn unknown_verb_exits_clap_error_code() {
    let dir = tempdir();
    let out = run(dir.path(), &["no-such-verb"]);
    // clap emits exit code 2 for usage errors.
    assert_eq!(out.status.code(), Some(2));
}

