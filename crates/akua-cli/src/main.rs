//! # akua
//!
//! Cloud-native packaging CLI. One binary, one contract — every verb
//! honours the CLI contract in [`docs/cli-contract.md`](../../../docs/cli-contract.md).

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

use akua_cli::contract::{emit_error, Context, UniversalArgs};
use akua_cli::verbs::{
    add as add_verb, check as check_verb, diff as diff_verb, fmt as fmt_verb, init as init_verb,
    inspect as inspect_verb, lint as lint_verb, remove as remove_verb, render as render_verb,
    tree as tree_verb, verify as verify_verb, version as version_verb, whoami as whoami_verb,
};
use akua_core::cli_contract::{AgentContext, ExitCode, StructuredError};

#[derive(Parser)]
#[command(name = "akua")]
#[command(about = "Cloud-native package build and transform toolkit", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new Package (akua.toml + package.k + inputs.example.yaml).
    Init {
        #[command(flatten)]
        args: UniversalArgs,

        /// Name for the new Package. When provided, also the directory
        /// name (created under the current working directory). When
        /// absent, scaffolds into `.` and uses the CWD basename.
        name: Option<String>,

        /// Overwrite existing scaffold files instead of aborting.
        #[arg(long)]
        force: bool,
    },

    /// Identity + agent-context introspection.
    Whoami {
        #[command(flatten)]
        args: UniversalArgs,
    },

    /// Print `akua` binary version.
    Version {
        #[command(flatten)]
        args: UniversalArgs,
    },

    /// Lockfile ↔ manifest consistency check.
    Verify {
        #[command(flatten)]
        args: UniversalArgs,

        /// Workspace root (default: current directory).
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Execute a `Package.k` against inputs and write manifests.
    Render {
        #[command(flatten)]
        args: UniversalArgs,

        #[command(flatten)]
        render_args: RenderCliArgs,
    },

    /// Insert a dependency into akua.toml. Pure manifest edit — no
    /// OCI fetch, no lockfile mutation.
    #[command(group(ArgGroup::new("source")
        .required(true)
        .args(["oci", "git", "path"])))]
    Add {
        #[command(flatten)]
        args: UniversalArgs,

        /// Local alias the dep is keyed under in `[dependencies]`.
        name: String,

        /// OCI source URL (e.g. `oci://ghcr.io/foo/charts/bar`).
        #[arg(long)]
        oci: Option<String>,

        /// Git source URL.
        #[arg(long)]
        git: Option<String>,

        /// Local filesystem path.
        #[arg(long)]
        path: Option<String>,

        /// Version constraint. Required for OCI deps.
        #[arg(long)]
        version: Option<String>,

        /// Git tag (alternative to `--rev`).
        #[arg(long)]
        tag: Option<String>,

        /// Git commit SHA (alternative to `--tag`).
        #[arg(long)]
        rev: Option<String>,

        /// Replace an existing entry under `name`.
        #[arg(long)]
        force: bool,

        /// Workspace root containing akua.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Print the workspace's declared deps + lockfile entries.
    Tree {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Remove a dependency from akua.toml.
    Remove {
        #[command(flatten)]
        args: UniversalArgs,

        /// Local alias of the dep to remove.
        name: String,

        /// No-op when the dep is already absent.
        #[arg(long)]
        ignore_missing: bool,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Structural diff between two rendered-output directories.
    Diff {
        #[command(flatten)]
        args: UniversalArgs,

        /// Baseline directory.
        before: PathBuf,

        /// Candidate directory.
        after: PathBuf,
    },

    /// Fast workspace check — parses akua.toml + akua.lock + lints package.k.
    Check {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        #[arg(long, default_value = "./package.k")]
        package: PathBuf,
    },

    /// Report a `package.k`'s input surface (options) without executing.
    Inspect {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = "./package.k")]
        package: PathBuf,
    },

    /// Parse-only check of a `package.k` (KCL syntax + imports).
    Lint {
        #[command(flatten)]
        args: UniversalArgs,

        /// Path to the `package.k` file.
        #[arg(long, default_value = "./package.k")]
        package: PathBuf,
    },

    /// Format a `package.k` via KCL's canonical formatter.
    Fmt {
        #[command(flatten)]
        args: UniversalArgs,

        /// Path to the `package.k` file.
        #[arg(long, default_value = "./package.k")]
        package: PathBuf,

        /// Exit 1 if the file would change; don't write.
        #[arg(long)]
        check: bool,

        /// Print formatted source to stdout; don't write.
        #[arg(long)]
        stdout: bool,
    },
}

#[derive(Args, Clone, Debug)]
struct RenderCliArgs {
    /// Path to the `package.k` file.
    #[arg(long, default_value = "./package.k")]
    package: PathBuf,

    /// Inputs file (JSON or YAML).
    #[arg(long)]
    inputs: Option<PathBuf>,

    /// Root directory for `RawManifests` outputs.
    #[arg(long, default_value = "./deploy")]
    out: PathBuf,

    /// Render only the named output.
    #[arg(long)]
    output: Option<String>,

    /// Render but don't write files.
    #[arg(long)]
    dry_run: bool,

    /// Print rendered manifests to stdout (requires a single selected output).
    #[arg(long)]
    stdout: bool,
}

fn main() {
    let cli = Cli::parse();
    let exit = dispatch(cli.command);
    std::process::exit(exit.code());
}

fn dispatch(command: Commands) -> ExitCode {
    match command {
        Commands::Init { args, name, force } => run_init(&args, name.as_deref(), force),
        Commands::Whoami { args } => run_whoami(&args),
        Commands::Version { args } => run_version(&args),
        Commands::Verify { args, workspace } => run_verify(&args, &workspace),
        Commands::Render { args, render_args } => run_render(&args, &render_args),
        Commands::Fmt {
            args,
            package,
            check,
            stdout,
        } => run_fmt(&args, &package, check, stdout),
        Commands::Inspect { args, package } => run_inspect(&args, &package),
        Commands::Lint { args, package } => run_lint(&args, &package),
        Commands::Check {
            args,
            workspace,
            package,
        } => run_check(&args, &workspace, &package),
        Commands::Diff {
            args,
            before,
            after,
        } => run_diff(&args, &before, &after),
        Commands::Remove {
            args,
            name,
            ignore_missing,
            workspace,
        } => run_remove(&args, &name, ignore_missing, &workspace),
        Commands::Tree { args, workspace } => run_tree(&args, &workspace),
        Commands::Add {
            args,
            name,
            oci,
            git,
            path,
            version,
            tag,
            rev,
            force,
            workspace,
        } => run_add(
            &args,
            &name,
            oci.as_deref(),
            git.as_deref(),
            path.as_deref(),
            version.as_deref(),
            tag.as_deref(),
            rev.as_deref(),
            force,
            &workspace,
        ),
    }
}

fn run_tree(args: &UniversalArgs, workspace: &std::path::Path) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = tree_verb::TreeArgs { workspace };
    let mut stdout = io::stdout().lock();
    match tree_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_remove(
    args: &UniversalArgs,
    name: &str,
    ignore_missing: bool,
    workspace: &std::path::Path,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = remove_verb::RemoveArgs {
        workspace,
        name,
        ignore_missing,
    };
    let mut stdout = io::stdout().lock();
    match remove_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_add(
    args: &UniversalArgs,
    name: &str,
    oci: Option<&str>,
    git: Option<&str>,
    path: Option<&str>,
    version: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
    force: bool,
    workspace: &std::path::Path,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let source = match (oci, git, path) {
        (Some(s), None, None) => add_verb::AddSource::Oci(s),
        (None, Some(s), None) => add_verb::AddSource::Git(s),
        (None, None, Some(s)) => add_verb::AddSource::Path(s),
        // The clap ArgGroup makes any other combination unreachable;
        // `unreachable!` here would surface a meaningful panic if
        // someone changes the group config without realising.
        _ => unreachable!("clap ArgGroup `source` is required + mutually-exclusive"),
    };
    let verb_args = add_verb::AddArgs {
        workspace,
        name,
        source,
        version,
        tag,
        rev,
        force,
    };
    let mut stdout = io::stdout().lock();
    match add_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_diff(args: &UniversalArgs, before: &std::path::Path, after: &std::path::Path) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = diff_verb::DiffArgs { before, after };
    let mut stdout = io::stdout().lock();
    match diff_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_check(
    args: &UniversalArgs,
    workspace: &std::path::Path,
    package: &std::path::Path,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = check_verb::CheckArgs {
        workspace,
        package_path: package,
    };
    let mut stdout = io::stdout().lock();
    match check_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_inspect(args: &UniversalArgs, package: &std::path::Path) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = inspect_verb::InspectArgs {
        package_path: package,
    };
    let mut stdout = io::stdout().lock();
    match inspect_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_lint(args: &UniversalArgs, package: &std::path::Path) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = lint_verb::LintArgs {
        package_path: package,
    };
    let mut stdout = io::stdout().lock();
    match lint_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_init(args: &UniversalArgs, name: Option<&str>, force: bool) -> ExitCode {
    let ctx = resolve_ctx(args);

    // When `name` is absent, scaffold into CWD and derive the package
    // name from its basename. When provided, scaffold into `./<name>/`.
    let (target, pkg_name) = match name {
        Some(n) => (PathBuf::from(n), n.to_string()),
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let derived = cwd
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            (cwd, derived)
        }
    };
    let verb_args = init_verb::InitArgs {
        target: &target,
        package_name: &pkg_name,
        force,
    };
    let mut stdout = io::stdout().lock();
    match init_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn resolve_ctx(args: &UniversalArgs) -> Context {
    Context::resolve(args, AgentContext::detect())
}

fn run_whoami(args: &UniversalArgs) -> ExitCode {
    let ctx = resolve_ctx(args);
    let mut stdout = io::stdout().lock();
    match whoami_verb::run(&ctx, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_io_error(&ctx, &e),
    }
}

fn run_version(args: &UniversalArgs) -> ExitCode {
    let ctx = resolve_ctx(args);
    let mut stdout = io::stdout().lock();
    match version_verb::run(&ctx, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_io_error(&ctx, &e),
    }
}

fn run_verify(args: &UniversalArgs, workspace: &std::path::Path) -> ExitCode {
    let ctx = resolve_ctx(args);
    let mut stdout = io::stdout().lock();
    match verify_verb::run(&ctx, workspace, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_fmt(
    args: &UniversalArgs,
    package: &std::path::Path,
    check: bool,
    stdout_mode: bool,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = fmt_verb::FmtArgs {
        package_path: package,
        check,
        stdout_mode,
    };
    let mut stdout = io::stdout().lock();
    match fmt_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_render(args: &UniversalArgs, render_args: &RenderCliArgs) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = render_verb::RenderArgs {
        package_path: &render_args.package,
        inputs_path: render_args.inputs.as_deref(),
        out_dir: &render_args.out,
        output_filter: render_args.output.as_deref(),
        dry_run: render_args.dry_run,
        stdout_mode: render_args.stdout,
    };
    let mut stdout = io::stdout().lock();
    match render_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

/// Emit a structured error to stderr and return the given exit code.
fn emit_structured(ctx: &Context, err: &StructuredError, code: ExitCode) -> ExitCode {
    let mut stderr = io::stderr().lock();
    let _ = emit_error(&mut stderr, ctx, err);
    let _ = stderr.flush();
    code
}

/// Fallback for verbs whose only error type is `std::io::Error` (stdout
/// write failures). Maps to a generic E_IO structured error and
/// [`ExitCode::SystemError`].
fn emit_io_error(ctx: &Context, err: &io::Error) -> ExitCode {
    let structured = StructuredError::new("E_IO", err.to_string()).with_default_docs();
    emit_structured(ctx, &structured, ExitCode::SystemError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whoami_with_universal_flags() {
        let cli = Cli::parse_from(["akua", "whoami", "--json"]);
        match cli.command {
            Commands::Whoami { args } => assert!(args.json),
            _ => panic!("expected whoami"),
        }
    }

    #[test]
    fn parses_render_with_all_flags() {
        let cli = Cli::parse_from([
            "akua",
            "render",
            "--package",
            "my.k",
            "--inputs",
            "in.yaml",
            "--out",
            "./dist",
            "--output",
            "static",
            "--dry-run",
        ]);
        match cli.command {
            Commands::Render { render_args, .. } => {
                assert_eq!(render_args.package, PathBuf::from("my.k"));
                assert_eq!(render_args.inputs, Some(PathBuf::from("in.yaml")));
                assert_eq!(render_args.out, PathBuf::from("./dist"));
                assert_eq!(render_args.output.as_deref(), Some("static"));
                assert!(render_args.dry_run);
                assert!(!render_args.stdout);
            }
            _ => panic!("expected render"),
        }
    }

    #[test]
    fn verify_defaults_workspace_to_dot() {
        let cli = Cli::parse_from(["akua", "verify"]);
        match cli.command {
            Commands::Verify { workspace, .. } => {
                assert_eq!(workspace, PathBuf::from("."));
            }
            _ => panic!("expected verify"),
        }
    }

    #[test]
    fn parses_init_with_name_and_force() {
        let cli = Cli::parse_from(["akua", "init", "my-pkg", "--force"]);
        match cli.command {
            Commands::Init { name, force, .. } => {
                assert_eq!(name.as_deref(), Some("my-pkg"));
                assert!(force);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_without_name_is_valid() {
        let cli = Cli::parse_from(["akua", "init"]);
        match cli.command {
            Commands::Init { name, force, .. } => {
                assert!(name.is_none());
                assert!(!force);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn parses_add_with_oci_and_version() {
        let cli = Cli::parse_from([
            "akua",
            "add",
            "cnpg",
            "--oci",
            "oci://ghcr.io/x/y",
            "--version",
            "1.2.3",
        ]);
        match cli.command {
            Commands::Add {
                name,
                oci,
                version,
                force,
                ..
            } => {
                assert_eq!(name, "cnpg");
                assert_eq!(oci.as_deref(), Some("oci://ghcr.io/x/y"));
                assert_eq!(version.as_deref(), Some("1.2.3"));
                assert!(!force);
            }
            _ => panic!("expected add"),
        }
    }

    #[test]
    fn add_requires_a_source_flag() {
        let err = Cli::try_parse_from(["akua", "add", "x"])
            .err()
            .expect("should fail");
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn add_rejects_two_sources_at_once() {
        let err =
            Cli::try_parse_from(["akua", "add", "x", "--oci", "oci://a", "--git", "https://b"])
                .err()
                .expect("should fail");
        assert!(err.to_string().contains("cannot be used"));
    }

    #[test]
    fn parses_diff_with_two_positional_dirs() {
        let cli = Cli::parse_from(["akua", "diff", "./before", "./after"]);
        match cli.command {
            Commands::Diff { before, after, .. } => {
                assert_eq!(before, PathBuf::from("./before"));
                assert_eq!(after, PathBuf::from("./after"));
            }
            _ => panic!("expected diff"),
        }
    }

    #[test]
    fn parses_inspect_with_package_override() {
        let cli = Cli::parse_from(["akua", "inspect", "--package", "foo.k"]);
        match cli.command {
            Commands::Inspect { package, .. } => {
                assert_eq!(package, PathBuf::from("foo.k"));
            }
            _ => panic!("expected inspect"),
        }
    }

    #[test]
    fn parses_lint_with_package_override() {
        let cli = Cli::parse_from(["akua", "lint", "--package", "foo.k"]);
        match cli.command {
            Commands::Lint { package, .. } => {
                assert_eq!(package, PathBuf::from("foo.k"));
            }
            _ => panic!("expected lint"),
        }
    }

    #[test]
    fn parses_fmt_with_check_flag() {
        let cli = Cli::parse_from(["akua", "fmt", "--check", "--package", "foo.k"]);
        match cli.command {
            Commands::Fmt {
                package,
                check,
                stdout,
                ..
            } => {
                assert_eq!(package, PathBuf::from("foo.k"));
                assert!(check);
                assert!(!stdout);
            }
            _ => panic!("expected fmt"),
        }
    }
}
