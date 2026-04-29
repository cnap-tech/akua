//! # akua
//!
//! Cloud-native packaging CLI. One binary, one contract — every verb
//! honours the CLI contract in [`docs/cli-contract.md`](../../../docs/cli-contract.md).

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

use akua_cli::contract::{emit_error, Context, UniversalArgs};
#[cfg(feature = "dev-watch")]
use akua_cli::verbs::dev as dev_verb;
use akua_cli::verbs::{
    add as add_verb, auth as auth_verb, cache as cache_verb, check as check_verb,
    diff as diff_verb, export as export_verb, fmt as fmt_verb, init as init_verb,
    inspect as inspect_verb, lint as lint_verb, lock as lock_verb, pack as pack_verb,
    publish as publish_verb, pull as pull_verb, push as push_verb, remove as remove_verb,
    render as render_verb, repl as repl_verb, test as test_verb, tree as tree_verb,
    update as update_verb, verify as verify_verb, version as version_verb, whoami as whoami_verb,
};
#[cfg(feature = "cosign-verify")]
use akua_cli::verbs::{sign as sign_verb, verify_tarball as verify_tarball_verb};
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

    /// Lockfile ↔ manifest consistency check (workspace mode, default)
    /// OR offline verify of a local tarball against its `.akuasig`
    /// sidecar via `--tarball` (requires `cosign-verify` feature).
    Verify {
        #[command(flatten)]
        args: UniversalArgs,

        /// Workspace root (default: current directory). Ignored when
        /// `--tarball` is set for anything other than resolving a
        /// default public key from `akua.toml [signing]`.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Verify a packed `.tar.gz` instead of a workspace. Pair
        /// with `akua sign` for the offline / air-gap flow.
        #[cfg(feature = "cosign-verify")]
        #[arg(long)]
        tarball: Option<PathBuf>,

        /// Path to the `.akuasig` sidecar. Defaults to
        /// `<tarball>.akuasig`. Only meaningful with `--tarball`.
        #[cfg(feature = "cosign-verify")]
        #[arg(long)]
        sig: Option<PathBuf>,

        /// Path to a PEM-encoded cosign public key. Falls back to
        /// `akua.toml [signing].cosign_public_key`. Only meaningful
        /// with `--tarball`. Absent → signature_verify is skipped.
        #[cfg(feature = "cosign-verify")]
        #[arg(long)]
        public_key: Option<PathBuf>,
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

    /// Produce a cosign signature for a packed tarball without
    /// touching a registry. Writes an `.akuasig` sidecar next to
    /// the tarball for a later `akua push --sig` to upload. Unlocks
    /// the air-gap flow: pack + sign here, transfer, push + upload-
    /// sig there.
    #[cfg(feature = "cosign-verify")]
    Sign {
        #[command(flatten)]
        args: UniversalArgs,

        /// Pre-packed tarball to sign.
        #[arg(long)]
        tarball: PathBuf,

        /// Target repository the tarball will be pushed to. The
        /// signature is bound to this ref + tag — sidecar is not
        /// portable across repositories.
        #[arg(long = "ref")]
        oci_ref: String,

        #[arg(long)]
        tag: String,

        /// Path to a PEM-encoded PKCS#8 P-256 private key. When
        /// absent, loads from `akua.toml [signing].cosign_private_key`.
        #[arg(long)]
        key: Option<PathBuf>,

        /// Workspace root (used to resolve the manifest signing key
        /// when `--key` isn't passed). Defaults to `.`.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Where to write the sidecar. Defaults to
        /// `<tarball>.akuasig`.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Intentionally bump `akua.lock` against whatever upstream now
    /// serves. Distinct from `akua lock`: where `lock` rejects OCI
    /// digest drift (security), `update` accepts it and records the
    /// new digest. `--dep <name>` scopes the refresh to one entry.
    /// Cargo analogue: `cargo update`.
    Update {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Only refresh this dep's lock entry.
        #[arg(long)]
        dep: Option<String>,
    },

    /// Regenerate `akua.lock` from `akua.toml`. Resolves every dep
    /// (online — path, OCI, git) and writes the merged lock back.
    /// `--check` mode diffs without writing; exits 1 on drift.
    /// Analogue of `cargo generate-lockfile`.
    Lock {
        #[command(flatten)]
        args: UniversalArgs,

        #[arg(long, default_value = ".")]
        workspace: PathBuf,

        /// Don't write; exit 1 if the lock would change. Use in
        /// CI / pre-commit to catch stale lockfiles.
        #[arg(long)]
        check: bool,
    },

    /// Upload a pre-packed `.tar.gz` (from `akua pack`) to an OCI
    /// registry. The push half of `akua publish`, decomposed so
    /// air-gap flows work: pack on one host, transfer the tarball,
    /// push from another.
    Push {
        #[command(flatten)]
        args: UniversalArgs,

        /// Pre-packed tarball on disk.
        #[arg(long)]
        tarball: PathBuf,

        /// Target repository — `oci://<registry>/<repo>`.
        #[arg(long = "ref")]
        oci_ref: String,

        /// Tag to publish under. Required — no workspace-local
        /// default; the tarball is just bytes.
        #[arg(long)]
        tag: String,

        /// Path to a `.akuasig` sidecar (from `akua sign`). When
        /// present, the sidecar's ref/tag/manifest_digest are
        /// matched against the push target; on match, the `.sig`
        /// is uploaded alongside the tarball.
        #[cfg(feature = "cosign-verify")]
        #[arg(long)]
        sig: Option<PathBuf>,
    },

    /// Interactive KCL shell — accumulates submitted lines into a
    /// growing `.k` source, re-evaluates on each entry, prints the
    /// top-level bindings. Meta commands start with `.`
    /// (`.load <path>`, `.reset`, `.show`, `.help`, `.exit`). No
    /// engine callables (`helm.template`, `pkg.render`, etc.) — use
    /// `akua render` against a workspace for those.
    Repl {
        #[command(flatten)]
        args: UniversalArgs,
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

    /// Report a Package's input surface (via `--package`) OR a
    /// packed tarball's metadata (via `--tarball`) without executing
    /// anything. Tarball mode pairs with `akua pack` + `akua push`
    /// for air-gap triage.
    #[command(group(ArgGroup::new("inspect_target").args(["package", "tarball"])))]
    Inspect {
        #[command(flatten)]
        args: UniversalArgs,

        /// Path to the Package.k (default mode). Mutually exclusive
        /// with `--tarball`.
        #[arg(long, default_value = "./package.k")]
        package: PathBuf,

        /// Path to a packed `.tar.gz` (from `akua pack` / `akua pull`).
        /// When set, overrides `--package`.
        #[arg(long)]
        tarball: Option<PathBuf>,
    },

    /// Emit the Package's `Input` schema as JSON Schema 2020-12 or
    /// OpenAPI 3.1. Powers UI form renderers, API doc generators,
    /// and admission-webhook schema validators.
    Export {
        #[command(flatten)]
        args: UniversalArgs,

        /// Path to the `package.k` file.
        #[arg(long, default_value = "./package.k")]
        package: PathBuf,

        /// Output format. JSON Schema 2020-12 (raw) or OpenAPI 3.1
        /// (Input wrapped under `components.schemas`).
        #[arg(long, value_enum, default_value_t = export_verb::ExportFormat::JsonSchema)]
        format: export_verb::ExportFormat,

        /// Write output to this path instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
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

    /// Hard cap on the `pkg.render` composition depth. Default 16
    /// (`BudgetSnapshot::DEFAULT_MAX_DEPTH`); lower it to harden CI
    /// against runaway fan-out. Cycle detection is separate.
    #[arg(long, value_name = "N")]
    max_depth: Option<usize>,

    /// Diagnostic — emit the post-eval resources list (pre-YAML
    /// normalization) alongside the rendered manifests under `--json`.
    /// Useful for debugging composition (`pkg.render`, `helm.template`,
    /// `kustomize.build` outputs visible). Hidden from short-help; the
    /// JSON shape is best-effort and may shift between releases.
    #[arg(long, hide_short_help = true)]
    debug: bool,
}

fn main() {
    let cli = Cli::parse();
    let args = universal_args(&cli.command);
    let ctx = resolve_ctx(args);
    let _observability = akua_cli::observability::init_subscriber(args, &ctx);
    let exit = dispatch(cli.command);
    std::process::exit(exit.code());
}

/// Extract the `UniversalArgs` block from whichever subcommand variant
/// was matched. Every leaf variant carries an `args: UniversalArgs`
/// field; the two grouping variants (`Cache`, `Auth`) delegate to their
/// own subcommand.
fn universal_args(cmd: &Commands) -> &UniversalArgs {
    match cmd {
        Commands::Init { args, .. }
        | Commands::Whoami { args }
        | Commands::Version { args }
        | Commands::Verify { args, .. }
        | Commands::Render { args, .. }
        | Commands::Add { args, .. }
        | Commands::Dev { args, .. }
        | Commands::Test { args, .. }
        | Commands::Tree { args, .. }
        | Commands::Pull { args, .. }
        | Commands::Publish { args, .. }
        | Commands::Sign { args, .. }
        | Commands::Update { args, .. }
        | Commands::Lock { args, .. }
        | Commands::Push { args, .. }
        | Commands::Repl { args, .. }
        | Commands::Pack { args, .. }
        | Commands::Remove { args, .. }
        | Commands::Diff { args, .. }
        | Commands::Check { args, .. }
        | Commands::Inspect { args, .. }
        | Commands::Export { args, .. }
        | Commands::Lint { args, .. }
        | Commands::Fmt { args, .. } => args,
        Commands::Cache { sub } => match sub {
            CacheSub::List { args } | CacheSub::Clear { args, .. } | CacheSub::Path { args } => {
                args
            }
        },
        Commands::Auth { sub } => match sub {
            AuthSub::List { args } | AuthSub::Add { args, .. } | AuthSub::Remove { args, .. } => {
                args
            }
        },
    }
}

fn dispatch(command: Commands) -> ExitCode {
    match command {
        Commands::Init { args, name, force } => run_init(&args, name.as_deref(), force),
        Commands::Whoami { args } => run_whoami(&args),
        Commands::Version { args } => run_version(&args),
        Commands::Verify {
            args,
            workspace,
            #[cfg(feature = "cosign-verify")]
            tarball,
            #[cfg(feature = "cosign-verify")]
            sig,
            #[cfg(feature = "cosign-verify")]
            public_key,
        } => {
            #[cfg(feature = "cosign-verify")]
            {
                if let Some(tar) = tarball {
                    run_verify_tarball(
                        &args,
                        &tar,
                        sig.as_deref(),
                        public_key.as_deref(),
                        &workspace,
                    )
                } else {
                    run_verify(&args, &workspace)
                }
            }
            #[cfg(not(feature = "cosign-verify"))]
            {
                run_verify(&args, &workspace)
            }
        }
        Commands::Render { args, render_args } => run_render(&args, &render_args),
        Commands::Fmt {
            args,
            package,
            check,
            stdout,
        } => run_fmt(&args, &package, check, stdout),
        Commands::Inspect {
            args,
            package,
            tarball,
        } => run_inspect(&args, &package, tarball.as_deref()),
        Commands::Export {
            args,
            package,
            format,
            out,
        } => run_export(&args, &package, format, out.as_deref()),
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
        } => run_publish(
            &args,
            &workspace,
            &oci_ref,
            tag.as_deref(),
            no_sign,
            no_attest,
        ),
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
        } => run_dev(
            &args,
            &workspace,
            &package,
            inputs.as_deref(),
            &out,
            debounce_ms,
        ),
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
        Commands::Cache { sub } => run_cache(sub),
        Commands::Auth { sub } => run_auth(sub),
        Commands::Pack {
            args,
            workspace,
            out,
            no_vendor,
        } => run_pack(&args, &workspace, out.as_deref(), no_vendor),
        Commands::Push {
            args,
            tarball,
            oci_ref,
            tag,
            #[cfg(feature = "cosign-verify")]
            sig,
        } => run_push(
            &args,
            &tarball,
            &oci_ref,
            &tag,
            #[cfg(feature = "cosign-verify")]
            sig.as_deref(),
        ),
        Commands::Lock {
            args,
            workspace,
            check,
        } => run_lock(&args, &workspace, check),
        Commands::Update {
            args,
            workspace,
            dep,
        } => run_update(&args, &workspace, dep.as_deref()),
        Commands::Repl { args } => run_repl(&args),
        #[cfg(feature = "cosign-verify")]
        Commands::Sign {
            args,
            tarball,
            oci_ref,
            tag,
            key,
            workspace,
            out,
        } => run_sign(
            &args,
            &tarball,
            &oci_ref,
            &tag,
            key.as_deref(),
            &workspace,
            out.as_deref(),
        ),
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

fn run_lock(args: &UniversalArgs, workspace: &std::path::Path, check: bool) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = lock_verb::LockArgs { workspace, check };
    let mut stdout = io::stdout().lock();
    match lock_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

#[cfg(feature = "cosign-verify")]
#[allow(clippy::too_many_arguments)]
fn run_sign(
    args: &UniversalArgs,
    tarball: &std::path::Path,
    oci_ref: &str,
    tag: &str,
    key: Option<&std::path::Path>,
    workspace: &std::path::Path,
    out: Option<&std::path::Path>,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = sign_verb::SignArgs {
        tarball,
        oci_ref,
        tag,
        key,
        workspace,
        out,
    };
    let mut stdout = io::stdout().lock();
    match sign_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

#[cfg(feature = "cosign-verify")]
fn run_verify_tarball(
    args: &UniversalArgs,
    tarball: &std::path::Path,
    sig: Option<&std::path::Path>,
    public_key: Option<&std::path::Path>,
    workspace: &std::path::Path,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = verify_tarball_verb::VerifyTarballArgs {
        tarball,
        sig,
        public_key,
        workspace,
    };
    let mut stdout = io::stdout().lock();
    match verify_tarball_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_repl(args: &UniversalArgs) -> ExitCode {
    let ctx = resolve_ctx(args);
    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut stdout = io::stdout().lock();
    match repl_verb::run(&ctx, &repl_verb::ReplArgs, &mut stdin_lock, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_update(args: &UniversalArgs, workspace: &std::path::Path, dep: Option<&str>) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = update_verb::UpdateArgs { workspace, dep };
    let mut stdout = io::stdout().lock();
    match update_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_push(
    args: &UniversalArgs,
    tarball: &std::path::Path,
    oci_ref: &str,
    tag: &str,
    #[cfg(feature = "cosign-verify")] sig: Option<&std::path::Path>,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = push_verb::PushArgs {
        tarball,
        oci_ref,
        tag,
        #[cfg(feature = "cosign-verify")]
        sig,
    };
    let mut stdout = io::stdout().lock();
    match push_verb::run(&ctx, &verb_args, &mut stdout) {
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
                    username: username.expect("clap ArgGroup guarantees username when !token"),
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

fn run_pull(args: &UniversalArgs, oci_ref: &str, tag: &str, out: &std::path::Path) -> ExitCode {
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

fn run_inspect(
    args: &UniversalArgs,
    package: &std::path::Path,
    tarball: Option<&std::path::Path>,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let target = match tarball {
        Some(p) => inspect_verb::InspectTarget::Tarball(p),
        None => inspect_verb::InspectTarget::Package(package),
    };
    let verb_args = inspect_verb::InspectArgs { target };
    let mut stdout = io::stdout().lock();
    match inspect_verb::run(&ctx, &verb_args, &mut stdout) {
        Ok(code) => code,
        Err(e) => emit_structured(&ctx, &e.to_structured(), e.exit_code()),
    }
}

fn run_export(
    args: &UniversalArgs,
    package: &std::path::Path,
    format: export_verb::ExportFormat,
    out: Option<&std::path::Path>,
) -> ExitCode {
    let ctx = resolve_ctx(args);
    let verb_args = export_verb::ExportArgs {
        package_path: package,
        format,
        out,
    };
    let mut stdout = io::stdout().lock();
    match export_verb::run(&ctx, &verb_args, &mut stdout) {
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
    let (target, pkg_name) = derive_init_target_and_name(name);
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

/// Decide where `akua init` writes and what `[package].name` it records.
///
/// Four cases:
/// 1. No name → CWD + sanitized basename (`mkdir foo && cd foo && akua init`).
/// 2. `.` / `./` → CWD + sanitized basename (the broken case from #4).
/// 3. Bare valid identifier → `./<name>/` + name as-is.
/// 4. Path-like or invalid identifier → use as path, sanitize basename
///    for the package name.
fn derive_init_target_and_name(name: Option<&str>) -> (PathBuf, String) {
    match name {
        Some(n) if n != "." && n != "./" && akua_core::is_valid_package_name(n) => {
            // Bare identifier — original behavior.
            (PathBuf::from(n), n.to_string())
        }
        Some(n) if n == "." || n == "./" => {
            // Scaffold into CWD; derive name from CWD basename.
            init_target_from_cwd()
        }
        Some(n) => {
            // Path-like name (e.g. `./foo`, `../bar`, `my-pkg/sub`) —
            // use as the target path, derive name from the basename.
            let target = PathBuf::from(n);
            let basename = target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let sanitized = sanitize_package_name(&basename);
            (target, sanitized)
        }
        None => init_target_from_cwd(),
    }
}

fn init_target_from_cwd() -> (PathBuf, String) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Canonicalize so `.` resolves to a real basename instead of the
    // empty string. Falls back to the relative path if canonicalize
    // fails (CWD deleted underneath us, etc.) — `EmptyName` will
    // surface from the verb in that pathological case.
    let resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let basename = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let sanitized = sanitize_package_name(&basename);
    (resolved, sanitized)
}

/// Coerce an arbitrary string into something `is_valid_package_name`
/// accepts. Lowercases ASCII, replaces non-`[a-z0-9_-]` with `_`,
/// strips leading hyphens. Returns empty if nothing usable remains —
/// the verb's existing `EmptyName` error covers that.
fn sanitize_package_name(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            let lc = c.to_ascii_lowercase();
            if lc.is_ascii_alphanumeric() || lc == '_' || lc == '-' {
                lc
            } else {
                '_'
            }
        })
        .collect();
    while out.starts_with('-') {
        out.remove(0);
    }
    out
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
        dry_run: render_args.dry_run,
        stdout_mode: render_args.stdout,
        strict: render_args.strict,
        offline: render_args.offline,
        debug: render_args.debug,
        max_depth: render_args.max_depth,
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
    fn sanitize_package_name_lowercases_and_replaces_dots_with_underscore() {
        assert_eq!(sanitize_package_name("Foo.Bar"), "foo_bar");
        assert_eq!(sanitize_package_name("my pkg"), "my_pkg");
        assert_eq!(sanitize_package_name("HELLO"), "hello");
    }

    #[test]
    fn sanitize_package_name_strips_leading_hyphens_and_keeps_internal() {
        assert_eq!(sanitize_package_name("-leading-dash"), "leading-dash");
        assert_eq!(sanitize_package_name("---multi"), "multi");
        assert_eq!(sanitize_package_name("hello-world"), "hello-world");
    }

    #[test]
    fn sanitize_package_name_returns_empty_for_pathological_input() {
        // The init verb surfaces this as E_INIT_EMPTY_NAME — it's the
        // single failure mode `derive_init_target_and_name` doesn't
        // try to fix.
        assert_eq!(sanitize_package_name(""), "");
        assert_eq!(sanitize_package_name("---"), "");
    }

    // The `Some(".")` → CWD-basename path mutates process-global CWD
    // and would race other tests; covered end-to-end in the
    // tests/cli_integration.rs subprocess harness instead.

    #[test]
    fn derive_init_with_bare_identifier_keeps_legacy_shape() {
        let (target, name) = derive_init_target_and_name(Some("my_pkg"));
        assert_eq!(target, std::path::PathBuf::from("my_pkg"));
        assert_eq!(name, "my_pkg");
    }

    #[test]
    fn derive_init_with_path_arg_uses_path_target_and_sanitized_basename() {
        // `akua init ./Some.Subdir` → target `./Some.Subdir`, name
        // `some_subdir`. Path-like args are a separate case from `.`.
        let (target, name) = derive_init_target_and_name(Some("./Some.Subdir"));
        assert_eq!(target, std::path::PathBuf::from("./Some.Subdir"));
        assert_eq!(name, "some_subdir");
    }

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
            Commands::Inspect {
                package, tarball, ..
            } => {
                assert_eq!(package, PathBuf::from("foo.k"));
                assert!(tarball.is_none());
            }
            _ => panic!("expected inspect"),
        }
    }

    #[test]
    fn parses_inspect_with_tarball_flag() {
        let cli = Cli::parse_from(["akua", "inspect", "--tarball", "./p.tgz"]);
        match cli.command {
            Commands::Inspect { tarball, .. } => {
                assert_eq!(tarball, Some(PathBuf::from("./p.tgz")));
            }
            _ => panic!("expected inspect"),
        }
    }

    #[test]
    fn inspect_rejects_package_and_tarball_together() {
        let err = Cli::try_parse_from([
            "akua",
            "inspect",
            "--package",
            "foo.k",
            "--tarball",
            "p.tgz",
        ])
        .err()
        .expect("should fail");
        assert!(err.to_string().contains("cannot be used"));
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
            Commands::Cache {
                sub: CacheSub::List { args },
            } => {
                assert!(args.json);
            }
            _ => panic!("expected cache list"),
        }
    }

    #[test]
    fn parses_cache_clear_with_oci_scope() {
        let cli = Cli::parse_from(["akua", "cache", "clear", "--oci"]);
        match cli.command {
            Commands::Cache {
                sub: CacheSub::Clear { oci, git, .. },
            } => {
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
    fn parses_repl_bare_verb() {
        let cli = Cli::parse_from(["akua", "repl"]);
        match cli.command {
            Commands::Repl { .. } => {}
            _ => panic!("expected repl"),
        }
    }

    #[test]
    fn parses_update_with_dep_filter() {
        let cli = Cli::parse_from(["akua", "update", "--dep", "nginx"]);
        match cli.command {
            Commands::Update { dep, workspace, .. } => {
                assert_eq!(dep.as_deref(), Some("nginx"));
                assert_eq!(workspace, PathBuf::from("."));
            }
            _ => panic!("expected update"),
        }
    }

    #[test]
    fn parses_update_bare_refreshes_everything() {
        let cli = Cli::parse_from(["akua", "update"]);
        match cli.command {
            Commands::Update { dep, .. } => {
                assert!(dep.is_none());
            }
            _ => panic!("expected update"),
        }
    }

    #[test]
    fn parses_lock_defaults_workspace_and_check() {
        let cli = Cli::parse_from(["akua", "lock"]);
        match cli.command {
            Commands::Lock {
                workspace, check, ..
            } => {
                assert_eq!(workspace, PathBuf::from("."));
                assert!(!check);
            }
            _ => panic!("expected lock"),
        }
    }

    #[test]
    fn parses_lock_with_check_flag() {
        let cli = Cli::parse_from(["akua", "lock", "--check", "--workspace", "./ws"]);
        match cli.command {
            Commands::Lock {
                workspace, check, ..
            } => {
                assert_eq!(workspace, PathBuf::from("./ws"));
                assert!(check);
            }
            _ => panic!("expected lock"),
        }
    }

    #[test]
    fn parses_push_with_tarball_ref_and_tag() {
        let cli = Cli::parse_from([
            "akua",
            "push",
            "--tarball",
            "./dist/p.tgz",
            "--ref",
            "oci://ghcr.io/x/y",
            "--tag",
            "0.1.0",
        ]);
        match cli.command {
            Commands::Push {
                tarball,
                oci_ref,
                tag,
                ..
            } => {
                assert_eq!(tarball, PathBuf::from("./dist/p.tgz"));
                assert_eq!(oci_ref, "oci://ghcr.io/x/y");
                assert_eq!(tag, "0.1.0");
            }
            _ => panic!("expected push"),
        }
    }

    #[test]
    fn push_requires_tag() {
        let err = Cli::try_parse_from([
            "akua",
            "push",
            "--tarball",
            "./p.tgz",
            "--ref",
            "oci://ghcr.io/x/y",
        ])
        .err()
        .expect("should fail");
        assert!(err.to_string().contains("--tag"));
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
                sub:
                    AuthSub::Add {
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
        let cli = Cli::parse_from(["akua", "auth", "add", "--registry", "ghcr.io", "--token"]);
        match cli.command {
            Commands::Auth {
                sub: AuthSub::Add {
                    token, username, ..
                },
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
            "akua",
            "auth",
            "add",
            "--registry",
            "ghcr.io",
            "--username",
            "alice",
            "--token",
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
