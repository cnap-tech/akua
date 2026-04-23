//! # akua
//!
//! Cloud-native packaging CLI. One binary, one contract — every verb
//! honours the CLI contract in [`docs/cli-contract.md`](../../../docs/cli-contract.md).

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

use akua_cli::contract::{emit_error, Context, UniversalArgs};
use akua_cli::verbs::{
    add as add_verb, auth as auth_verb, cache as cache_verb, check as check_verb,
    diff as diff_verb, fmt as fmt_verb, init as init_verb, inspect as inspect_verb,
    lint as lint_verb, pack as pack_verb, publish as publish_verb, pull as pull_verb,
    remove as remove_verb, render as render_verb, test as test_verb, tree as tree_verb,
    verify as verify_verb, version as version_verb, whoami as whoami_verb,
};
#[cfg(feature = "dev-watch")]
use akua_cli::verbs::dev as dev_verb;
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

    /// Watch the workspace + re-render on save. Blocks until Ctrl-C.
    /// Each debounced save batch emits one event (JSON line when
    /// `--json`, one-line status otherwise) carrying the render
    /// duration + summary.
    #[cfg(feature = "dev-watch")]
    Dev {
        #[command(flatten)]
        args: UniversalArgs,

        /// Path to the Package.k.
        #[arg(long, default_value = "./package.k")]
        package: PathBuf,

        /// Inputs file (JSON or YAML). Defaults to auto-discovery
        /// next to the Package.
        #[arg(long)]
        inputs: Option<PathBuf>,

        /// Render output dir.
        #[arg(long, default_value = "./deploy")]
        out: PathBuf,

        /// Workspace root to watch.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Debounce window for batching rapid saves (ms).
        #[arg(long, default_value = "200")]
        debounce_ms: u64,
    },

    /// Run `test_*.k` / `*_test.k` files in the workspace. Each
    /// test is a standalone KCL program; `assert` / `check:`
    /// failures are reported as test failures. `--golden` also runs
    /// snapshot diffs of every package.k against
    /// `snapshots/<pkg>/<inputs-stem>/`; `--update-snapshots`
    /// regenerates those snapshots (implies `--golden`). Exits
    /// non-zero on any failure.
    Test {
        #[command(flatten)]
        args: UniversalArgs,

        /// Workspace root.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Run golden snapshot tests in addition to assertion tests.
        #[arg(long)]
        golden: bool,

        /// Rewrite snapshot files rather than diff. Implies --golden.
        #[arg(long)]
        update_snapshots: bool,
    },

    /// Print the workspace's declared deps + lockfile entries.
    Tree {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Pull a published akua Package from an OCI registry and extract
    /// it to a target directory.
    Pull {
        #[command(flatten)]
        args: UniversalArgs,

        /// Source repository — `oci://<registry>/<repo>`.
        #[arg(long = "ref")]
        oci_ref: String,

        /// Tag to pull. Required.
        #[arg(long)]
        tag: String,

        /// Target directory to extract into. Created if missing.
        #[arg(long, default_value = "./pulled")]
        out: PathBuf,
    },

    /// Package the workspace and push it to an OCI registry as an
    /// akua Package artifact. Uploads the tarball + writes a manifest
    /// under `oci://<ref>:<tag>` (tag defaults to `[package].version`).
    /// Signs by default when `akua.toml [signing].cosign_private_key`
    /// is set; `--no-sign` skips signing explicitly.
    Publish {
        #[command(flatten)]
        args: UniversalArgs,

        /// Target repository — `oci://<registry>/<repo>`.
        #[arg(long = "ref")]
        oci_ref: String,

        /// Tag to publish under. Defaults to the workspace's
        /// `[package].version`.
        #[arg(long)]
        tag: Option<String>,

        /// Skip cosign signing even when a private key is configured
        /// in `akua.toml`.
        #[arg(long)]
        no_sign: bool,

        /// Skip SLSA attestation generation. Has no effect when
        /// `--no-sign` is set (attestation always pairs with signing).
        #[arg(long)]
        no_attest: bool,

        /// Workspace root containing akua.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },

    /// Pack the workspace into a local `.tar.gz` — same shape as the
    /// tarball `akua publish` uploads, but written to disk instead of
    /// pushed. Use for air-gap transfers, offline signing, or
    /// archival diff.
    Pack {
        #[command(flatten)]
        args: UniversalArgs,

        /// Workspace root containing akua.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Output tarball path. Defaults to
        /// `./dist/<name>-<version>.tar.gz` under the workspace
        /// (the `dist/` dir is walker-skipped, so re-packing is
        /// idempotent).
        #[arg(long)]
        out: Option<PathBuf>,

        /// Don't embed resolved OCI/git deps under `.akua/vendor/`.
        /// Produces a smaller tarball that won't render offline.
        #[arg(long)]
        no_vendor: bool,
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

    /// List, clear, or locate the on-disk OCI + git caches under
    /// `$XDG_CACHE_HOME/akua/`. Useful for ephemeral CI runners
    /// and disk-pressure triage.
    Cache {
        #[command(subcommand)]
        sub: CacheSub,
    },

    /// Manage credentials in `$XDG_CONFIG_HOME/akua/auth.toml`.
    /// `add` reads the secret from stdin (no interactive prompt,
    /// no password on the command line — mirrors `docker login
    /// --password-stdin`).
    Auth {
        #[command(subcommand)]
        sub: AuthSub,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum AuthSub {
    /// Show every configured registry across both akua/auth.toml
    /// and ~/.docker/config.json. Never prints secrets.
    List {
        #[command(flatten)]
        args: UniversalArgs,
    },

    /// Write/overwrite an entry in akua/auth.toml. The secret is
    /// read from stdin. Use `--username` for basic auth, `--token`
    /// for bearer (PATs).
    #[command(group(ArgGroup::new("auth_kind")
        .required(true)
        .args(["username", "token"])))]
    Add {
        #[command(flatten)]
        args: UniversalArgs,

        /// Registry host (e.g. `ghcr.io`, `quay.io`).
        #[arg(long)]
        registry: String,

        /// Username for basic auth. The password is read from stdin.
        #[arg(long)]
        username: Option<String>,

        /// Bearer token mode. The token is read from stdin.
        #[arg(long)]
        token: bool,
    },

    /// Drop an entry from akua/auth.toml. No-op when the registry
    /// is absent.
    Remove {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long)]
        registry: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum CacheSub {
    /// Enumerate every OCI blob + git repo/checkout with size.
    List {
        #[command(flatten)]
        args: UniversalArgs,
    },
    /// Wipe the caches. Default clears both; narrow with `--oci` or
    /// `--git`. No-op on absent caches.
    #[command(group(ArgGroup::new("cache_scope").args(["oci", "git"])))]
    Clear {
        #[command(flatten)]
        args: UniversalArgs,

        /// Only wipe the OCI blob cache.
        #[arg(long)]
        oci: bool,

        /// Only wipe the git repo + checkout cache.
        #[arg(long)]
        git: bool,
    },
    /// Print the resolved cache roots.
    Path {
        #[command(flatten)]
        args: UniversalArgs,
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

    /// Root directory where rendered YAML files land.
    #[arg(long, default_value = "./deploy")]
    out: PathBuf,

    /// Render but don't write files.
    #[arg(long)]
    dry_run: bool,

    /// Print rendered manifests as multi-doc YAML to stdout instead of writing files.
    #[arg(long)]
    stdout: bool,

    /// Reject raw-string plugin paths. Every chart must be declared in
    /// `akua.toml` and imported as `charts.<name>`. Equivalent to
    /// Cargo's `--locked` — CI-grade "every dep accounted for."
    #[arg(long)]
    strict: bool,

    /// Forbid network access during resolve. OCI deps must be fully
    /// satisfied from the local cache (populated by a prior
    /// `akua add`). Path + replace deps are unaffected.
    #[arg(long)]
    offline: bool,
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
        Commands::Diff { args, before, after } => run_diff(&args, &before, &after),
        Commands::Pull {
            args,
            oci_ref,
            tag,
            out,
        } => run_pull(&args, &oci_ref, &tag, &out),
        Commands::Publish {
            args,
            oci_ref,
            tag,
            no_sign,
            no_attest,
            workspace,
        } => run_publish(&args, &workspace, &oci_ref, tag.as_deref(), no_sign, no_attest),
        Commands::Remove {
            args,
            name,
            ignore_missing,
            workspace,
        } => run_remove(&args, &name, ignore_missing, &workspace),
        #[cfg(feature = "dev-watch")]
        Commands::Dev {
            args,
            package,
            inputs,
            out,
            workspace,
            debounce_ms,
        } => run_dev(&args, &workspace, &package, inputs.as_deref(), &out, debounce_ms),
        Commands::Test {
            args,
            workspace,
            golden,
            update_snapshots,
        } => run_test(&args, &workspace, golden, update_snapshots),
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
            &args, &name, oci.as_deref(), git.as_deref(), path.as_deref(),
            version.as_deref(), tag.as_deref(), rev.as_deref(), force, &workspace,
        ),
        Commands::Cache { sub } => run_cache(sub),
        Commands::Auth { sub } => run_auth(sub),
        Commands::Pack {
            args,
            workspace,
            out,
            no_vendor,
        } => run_pack(&args, &workspace, out.as_deref(), no_vendor),
    }
}

fn run_pack(
    args: &UniversalArgs,
    workspace: &std::path::Path,
    out: Option<&std::path::Path>,
    no_vendor: bool,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = pack_verb::PackArgs {
        workspace,
        out,
        no_vendor,
    };
    let mut stdout = io::stdout().lock();
    match pack_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_auth(sub: AuthSub) -> ExitCode {
    let (args, action) = match sub {
        AuthSub::List { args } => (args, auth_verb::AuthAction::List),
        AuthSub::Add {
            args,
            registry,
            username,
            token,
        } => {
            let input = if token {
                auth_verb::AuthAddInput::Bearer { registry }
            } else {
                // clap ArgGroup guarantees exactly one of username/token.
                auth_verb::AuthAddInput::Basic {
                    registry,
                    username: username
                        .expect("clap ArgGroup guarantees username when !token"),
                }
            };
            (args, auth_verb::AuthAction::Add(input))
        }
        AuthSub::Remove { args, registry } => (args, auth_verb::AuthAction::Remove { registry }),
    };
    let ctx = resolve_ctx(&args);
    let verb_args = auth_verb::AuthArgs { action };
    let mut stdout = io::stdout().lock();
    let mut stdin_reader = auth_verb::StdinReader;
    match auth_verb::run(&ctx, &verb_args, &mut stdout, &mut stdin_reader) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_cache(sub: CacheSub) -> ExitCode {
    let (args, action) = match sub {
        CacheSub::List { args } => (args, cache_verb::CacheAction::List),
        CacheSub::Clear { args, oci, git } => {
            let scope = match (oci, git) {
                (true, false) => akua_core::cache_inventory::ClearScope::OciOnly,
                (false, true) => akua_core::cache_inventory::ClearScope::GitOnly,
                _ => akua_core::cache_inventory::ClearScope::Both,
            };
            (args, cache_verb::CacheAction::Clear { scope })
        }
        CacheSub::Path { args } => (args, cache_verb::CacheAction::Path),
    };
    let ctx = resolve_ctx(&args);
    let verb_args = cache_verb::CacheArgs { action };
    let mut stdout = io::stdout().lock();
    match cache_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_pull(
    args: &UniversalArgs,
    oci_ref: &str,
    tag: &str,
    out: &std::path::Path,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = pull_verb::PullArgs { oci_ref, tag, out };
    let mut stdout = io::stdout().lock();
    match pull_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_publish(
    args: &UniversalArgs,
    workspace: &std::path::Path,
    oci_ref: &str,
    tag: Option<&str>,
    no_sign: bool,
    no_attest: bool,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = publish_verb::PublishArgs {
        workspace,
        oci_ref,
        tag,
        no_sign,
        no_attest,
    };
    let mut stdout = io::stdout().lock();
    match publish_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

#[cfg(feature = "dev-watch")]
fn run_dev(
    args: &UniversalArgs,
    workspace: &std::path::Path,
    package: &std::path::Path,
    inputs: Option<&std::path::Path>,
    out: &std::path::Path,
    debounce_ms: u64,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = dev_verb::DevArgs {
        workspace,
        package_path: package.to_path_buf(),
        inputs_path: inputs.map(|p| p.to_path_buf()),
        out_dir: out.to_path_buf(),
        debounce: std::time::Duration::from_millis(debounce_ms),
    };
    let mut stdout = io::stdout().lock();
    match dev_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_test(
    args: &UniversalArgs,
    workspace: &std::path::Path,
    golden: bool,
    update_snapshots: bool,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = test_verb::TestArgs {
        workspace,
        golden,
        update_snapshots,
    };
    let mut stdout = io::stdout().lock();
    match test_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
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

fn run_check(args: &UniversalArgs, workspace: &std::path::Path, package: &std::path::Path) -> ExitCode {
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

fn run_fmt(args: &UniversalArgs, package: &std::path::Path, check: bool, stdout_mode: bool) -> ExitCode {
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
        dry_run: render_args.dry_run,
        stdout_mode: render_args.stdout,
        strict: render_args.strict,
        offline: render_args.offline,
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
            "--dry-run",
        ]);
        match cli.command {
            Commands::Render { render_args, .. } => {
                assert_eq!(render_args.package, PathBuf::from("my.k"));
                assert_eq!(render_args.inputs, Some(PathBuf::from("in.yaml")));
                assert_eq!(render_args.out, PathBuf::from("./dist"));
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
                name, oci, version, force, ..
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
        let err = Cli::try_parse_from(["akua", "add", "x"]).err().expect("should fail");
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn add_rejects_two_sources_at_once() {
        let err = Cli::try_parse_from([
            "akua", "add", "x", "--oci", "oci://a", "--git", "https://b",
        ])
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
    fn parses_cache_list_subverb() {
        let cli = Cli::parse_from(["akua", "cache", "list", "--json"]);
        match cli.command {
            Commands::Cache { sub: CacheSub::List { args } } => {
                assert!(args.json);
            }
            _ => panic!("expected cache list"),
        }
    }

    #[test]
    fn parses_cache_clear_with_oci_scope() {
        let cli = Cli::parse_from(["akua", "cache", "clear", "--oci"]);
        match cli.command {
            Commands::Cache { sub: CacheSub::Clear { oci, git, .. } } => {
                assert!(oci);
                assert!(!git);
            }
            _ => panic!("expected cache clear"),
        }
    }

    #[test]
    fn cache_clear_rejects_both_scope_flags_together() {
        let err = Cli::try_parse_from(["akua", "cache", "clear", "--oci", "--git"])
            .err()
            .expect("should fail");
        assert!(err.to_string().contains("cannot be used"));
    }

    #[test]
    fn parses_pack_with_out_and_no_vendor() {
        let cli = Cli::parse_from([
            "akua",
            "pack",
            "--workspace",
            "./ws",
            "--out",
            "./dist/p.tgz",
            "--no-vendor",
        ]);
        match cli.command {
            Commands::Pack {
                workspace,
                out,
                no_vendor,
                ..
            } => {
                assert_eq!(workspace, PathBuf::from("./ws"));
                assert_eq!(out, Some(PathBuf::from("./dist/p.tgz")));
                assert!(no_vendor);
            }
            _ => panic!("expected pack"),
        }
    }

    #[test]
    fn pack_defaults_workspace_and_omits_out() {
        let cli = Cli::parse_from(["akua", "pack"]);
        match cli.command {
            Commands::Pack {
                workspace,
                out,
                no_vendor,
                ..
            } => {
                assert_eq!(workspace, PathBuf::from("."));
                assert!(out.is_none());
                assert!(!no_vendor);
            }
            _ => panic!("expected pack"),
        }
    }

    #[test]
    fn parses_auth_add_with_username() {
        let cli = Cli::parse_from([
            "akua",
            "auth",
            "add",
            "--registry",
            "ghcr.io",
            "--username",
            "alice",
        ]);
        match cli.command {
            Commands::Auth {
                sub: AuthSub::Add {
                    registry,
                    username,
                    token,
                    ..
                },
            } => {
                assert_eq!(registry, "ghcr.io");
                assert_eq!(username.as_deref(), Some("alice"));
                assert!(!token);
            }
            _ => panic!("expected auth add"),
        }
    }

    #[test]
    fn parses_auth_add_with_token_flag() {
        let cli =
            Cli::parse_from(["akua", "auth", "add", "--registry", "ghcr.io", "--token"]);
        match cli.command {
            Commands::Auth {
                sub: AuthSub::Add { token, username, .. },
            } => {
                assert!(token);
                assert!(username.is_none());
            }
            _ => panic!("expected auth add"),
        }
    }

    #[test]
    fn auth_add_rejects_username_and_token_together() {
        let err = Cli::try_parse_from([
            "akua", "auth", "add", "--registry", "ghcr.io", "--username", "alice", "--token",
        ])
        .err()
        .expect("should fail");
        assert!(err.to_string().contains("cannot be used"));
    }

    #[test]
    fn auth_add_requires_a_kind() {
        let err = Cli::try_parse_from(["akua", "auth", "add", "--registry", "ghcr.io"])
            .err()
            .expect("should fail");
        assert!(err.to_string().contains("required"));
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
