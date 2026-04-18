//! # akua
//!
//! Command-line tool for Akua. Scaffolds, previews, tests, builds, and
//! publishes cloud-native packages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{ArgGroup, Parser, Subcommand};

use akua_core::{
    apply_install_transforms, build_metadata, build_provenance, build_umbrella_chart_in,
    extract_install_fields, fetch_dependencies, load_manifest, package_chart, publish_chart,
    render_umbrella, render_umbrella_embedded, validate_values_schema, write_metadata,
    write_umbrella, AkuaMetadata, PackageManifest, PublishOptions, RenderOptions, UmbrellaChart,
};

#[derive(Parser)]
#[command(name = "akua")]
#[command(about = "Cloud-native package build and transform toolkit", long_about = None)]
#[command(version)]
struct Cli {
    /// Log output format. `text` (default) is human-readable; `json`
    /// emits one structured record per event, suitable for Temporal's
    /// `ApplicationFailure.details` or log-aggregator ingestion.
    #[arg(long, value_enum, default_value_t = LogFormat::Text, global = true)]
    log_format: LogFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new package (package.yaml, values.schema.json, README.md).
    Init {
        /// Directory to create the package in. Created if missing.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Package name. Defaults to the target directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Overwrite existing files instead of aborting.
        #[arg(long)]
        force: bool,
    },
    /// Preview the resolved values for a given set of inputs.
    ///
    /// Reads `<package>/values.schema.json`, extracts fields marked with
    /// `x-user-input` / `x-install`, and applies transforms (slugify,
    /// template) over `--inputs`. Prints the resolved values object.
    #[command(group(ArgGroup::new("input_src").args(["inputs", "inputs_file"])))]
    Preview {
        /// Path to a package directory (containing `values.schema.json`).
        #[arg(long, default_value = ".")]
        package: PathBuf,
        /// JSON inputs (inline). Keys are field dot-paths.
        #[arg(long)]
        inputs: Option<String>,
        /// Path to JSON inputs file.
        #[arg(long)]
        inputs_file: Option<PathBuf>,
        /// Emit compact JSON (for scripts / agents) instead of pretty JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Print the resolved dependency tree (umbrella Chart.yaml structure).
    Tree {
        #[arg(long, default_value = ".")]
        package: PathBuf,
    },
    /// Run package tests (`resolve.test.*`, schema validation, etc.).
    Test,
    /// Lint the package (schema validation, transform wiring).
    Lint {
        #[arg(long, default_value = ".")]
        package: PathBuf,
    },
    /// Assemble the umbrella chart on disk (no render, no fetch).
    ///
    /// Writes Chart.yaml + values.yaml for the package into `--out`. Useful as
    /// input to `helm dependency update && helm template` or as a
    /// pre-flight check before `akua render`. By default also emits
    /// `.akua/metadata.yaml` — pass `--strip-metadata` to omit.
    Build {
        #[arg(long, default_value = ".")]
        package: PathBuf,
        #[arg(long, default_value = "./dist/chart")]
        out: PathBuf,
        /// Omit `.akua/metadata.yaml` from the built chart.
        #[arg(long)]
        strip_metadata: bool,
    },
    /// Print the Akua provenance metadata from a built chart directory.
    /// Inspect a chart — either a local built directory or any OCI chart
    /// reference. For `oci://…` targets, pulls the chart (including
    /// non-Akua charts) and emits Chart.yaml + values.schema.json (when
    /// present) + `.akua/metadata.yaml` (when present) as JSON, so
    /// consumers can cache schemas for BYO charts without extra tooling.
    Inspect {
        /// Path to a built chart directory. Mutually exclusive with `--oci`.
        #[arg(long)]
        chart: Option<PathBuf>,
        /// OCI reference, e.g. `oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1`.
        /// Mutually exclusive with `--chart`.
        #[arg(long)]
        oci: Option<String>,
        /// Registry username for basic auth against the OCI host. Pair
        /// with `--oci-password` (or set both via env vars).
        #[arg(long, env = "AKUA_OCI_USERNAME")]
        oci_username: Option<String>,
        /// Registry password for basic auth. Paired with `--oci-username`.
        #[arg(long, env = "AKUA_OCI_PASSWORD", hide_env_values = true)]
        oci_password: Option<String>,
        /// Pre-acquired bearer token for the OCI host. Mutually
        /// exclusive with basic-auth flags.
        #[arg(long, env = "AKUA_OCI_TOKEN", hide_env_values = true)]
        oci_token: Option<String>,
    },
    /// Emit a SLSA v1 provenance attestation from a built chart directory.
    ///
    /// The output is an unsigned predicate. Sign + push with:
    ///     cosign attest --predicate <out> --type slsaprovenance1 <image>
    Attest {
        #[arg(long, default_value = "./dist/chart")]
        chart: PathBuf,
        #[arg(long, default_value = "./dist/attestation.json")]
        out: PathBuf,
    },
    /// Render the umbrella chart to Kubernetes manifests.
    ///
    /// Two engines:
    /// - `--engine helm-wasm` (default) uses the embedded Helm v4 template
    ///   engine. Still calls `helm dependency update` if the umbrella has
    ///   remote deps; the template phase itself needs no `helm` on `$PATH`.
    /// - `--engine helm-cli` shells to `helm template`. Legacy path.
    #[command(group(ArgGroup::new("render_inputs").args(["inputs", "inputs_file"])))]
    Render {
        #[arg(long, default_value = ".")]
        package: PathBuf,
        /// Write rendered manifests to this file. Omit to print to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "release")]
        release: String,
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Render engine: `helm-wasm` (embedded, default) or `helm-cli`.
        #[arg(long, default_value = "helm-wasm")]
        engine: String,
        /// Path to the `helm` CLI (only used with `--engine helm-cli` and for
        /// dep resolution with `helm-wasm` when remote deps are present).
        #[arg(long, default_value = "helm")]
        helm_bin: PathBuf,
        /// JSON user inputs (path → value) applied via schema transforms.
        #[arg(long)]
        inputs: Option<String>,
        #[arg(long)]
        inputs_file: Option<PathBuf>,
    },
    /// Package a built chart directory as a `.tgz` (Helm-compatible).
    ///
    /// Writes `<chart-name>-<chart-version>.tgz` into `--out-dir`. The
    /// archive wraps a single top-level `<chart-name>/` directory — matches
    /// what `helm package` produces, so `helm install ./…tgz` works.
    Package {
        /// Built chart directory (output of `akua build`).
        #[arg(long, default_value = "./dist/chart")]
        chart: PathBuf,
        /// Directory to write the `.tgz` into. Created if missing.
        #[arg(long, default_value = "./dist")]
        out_dir: PathBuf,
    },
    /// Publish a built chart directory to an OCI registry.
    ///
    /// Native OCI push via oci-client — no helm CLI needed. Target is the
    /// namespace URL (e.g., `oci://ghcr.io/acme/charts`); the final ref
    /// becomes `<namespace>/<chart-name>:<chart-version>`.
    Publish {
        /// Built chart directory (output of `akua build`).
        #[arg(long, default_value = "./dist/chart")]
        chart: PathBuf,
        /// OCI namespace URL.
        #[arg(long)]
        to: String,
        /// Optional username for basic auth (paired with --password).
        #[arg(long, env = "AKUA_REGISTRY_USER")]
        username: Option<String>,
        #[arg(long, env = "AKUA_REGISTRY_PASSWORD")]
        password: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_format);
    let cmd_name = command_name(&cli.command);
    let phase = phase_for(&cli.command);
    tracing::info!(
        phase,
        command = cmd_name,
        version = env!("CARGO_PKG_VERSION"),
        "akua starting"
    );

    let result = dispatch(cli.command);
    if let Err(err) = &result {
        // Structured error boundary. `anyhow::Error` chains into `{:#}`.
        tracing::error!(phase, command = cmd_name, error = %format!("{err:#}"), "akua failed");
    } else {
        tracing::info!(phase, command = cmd_name, "akua done");
    }
    result
}

fn command_name(cmd: &Commands) -> &'static str {
    match cmd {
        Commands::Init { .. } => "init",
        Commands::Preview { .. } => "preview",
        Commands::Tree { .. } => "tree",
        Commands::Test => "test",
        Commands::Lint { .. } => "lint",
        Commands::Build { .. } => "build",
        Commands::Inspect { .. } => "inspect",
        Commands::Attest { .. } => "attest",
        Commands::Render { .. } => "render",
        Commands::Package { .. } => "package",
        Commands::Publish { .. } => "publish",
    }
}

/// Stable pipeline phase tag attached to top-level log events so
/// downstream aggregators (Temporal workers, log shippers) can group
/// events without parsing the subcommand label. At the CLI layer
/// `phase` matches `command` 1:1; library callers can emit richer
/// sub-phases (e.g. `fetch_deps`) on their own events.
fn phase_for(cmd: &Commands) -> &'static str {
    command_name(cmd)
}

fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Preview {
            package,
            inputs,
            inputs_file,
            compact,
        } => run_preview(&package, inputs.as_deref(), inputs_file.as_deref(), compact),
        Commands::Lint { package } => run_lint(&package),
        Commands::Tree { package } => run_tree(&package),
        Commands::Build {
            package,
            out,
            strip_metadata,
        } => run_build(&package, &out, strip_metadata),
        Commands::Inspect {
            chart,
            oci,
            oci_username,
            oci_password,
            oci_token,
        } => run_inspect(
            chart.as_deref(),
            oci.as_deref(),
            OciAuthArgs {
                username: oci_username,
                password: oci_password,
                token: oci_token,
            },
        ),
        Commands::Attest { chart, out } => run_attest(&chart, &out),
        Commands::Render {
            package,
            out,
            release,
            namespace,
            engine,
            helm_bin,
            inputs,
            inputs_file,
        } => run_render(RenderArgs {
            package_dir: &package,
            out: out.as_deref(),
            release: &release,
            namespace: &namespace,
            engine: &engine,
            helm_bin: &helm_bin,
            inputs_inline: inputs.as_deref(),
            inputs_file: inputs_file.as_deref(),
        }),
        Commands::Init { dir, name, force } => run_init(&dir, name.as_deref(), force),
        Commands::Test => stub("test"),
        Commands::Package { chart, out_dir } => run_package(&chart, &out_dir),
        Commands::Publish {
            chart,
            to,
            username,
            password,
        } => run_publish(&chart, &to, username, password),
    }
}

fn stub(name: &str) -> Result<()> {
    eprintln!("akua {name} — not yet implemented");
    Ok(())
}

/// Scaffold a starter package: `package.yaml`, `values.schema.json`,
/// `README.md`. Refuses to clobber unless `force` is set.
fn run_init(dir: &Path, name_override: Option<&str>, force: bool) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let resolved_name = match name_override {
        Some(n) => n.to_string(),
        None => dir
            .canonicalize()
            .with_context(|| format!("canonicalising {}", dir.display()))?
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("cannot derive package name from {}", dir.display()))?
            .to_string(),
    };

    let files: [(&str, String); 3] = [
        ("package.yaml", starter_package_yaml(&resolved_name)),
        ("values.schema.json", starter_values_schema()),
        ("README.md", starter_readme(&resolved_name)),
    ];

    for (fname, _) in &files {
        let path = dir.join(fname);
        if path.exists() && !force {
            bail!(
                "{} already exists — pass --force to overwrite",
                path.display()
            );
        }
    }
    for (fname, contents) in files {
        let path = dir.join(fname);
        std::fs::write(&path, contents)
            .with_context(|| format!("writing {}", path.display()))?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

fn starter_package_yaml(name: &str) -> String {
    format!(
        "apiVersion: akua.dev/v1alpha1\n\
         \n\
         name: {name}\n\
         version: 0.1.0\n\
         description: TODO — describe this package.\n\
         \n\
         schema: ./values.schema.json\n\
         \n\
         sources:\n\
         \x20\x20- name: app\n\
         \x20\x20\x20\x20helm:\n\
         \x20\x20\x20\x20\x20\x20repo: https://charts.bitnami.com/bitnami\n\
         \x20\x20\x20\x20\x20\x20chart: nginx\n\
         \x20\x20\x20\x20\x20\x20version: 18.1.0\n\
         \x20\x20\x20\x20values:\n\
         \x20\x20\x20\x20\x20\x20replicaCount: 1\n"
    )
}

fn starter_values_schema() -> String {
    r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "replicaCount": {
      "type": "integer",
      "default": 1
    }
  }
}
"#
    .to_string()
}

fn starter_readme(name: &str) -> String {
    format!(
        "# {name}\n\n\
         An Akua package. Edit `package.yaml` and `values.schema.json`, then:\n\n\
         ```sh\n\
         akua preview\n\
         akua build\n\
         ```\n"
    )
}

/// Initialise the global tracing subscriber. Text format (default) goes
/// to stderr as compact human-readable output; JSON goes to stderr as
/// one newline-delimited record per event for downstream parsers
/// (Temporal's `ApplicationFailure.details`, log aggregators, etc.).
fn init_tracing(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::from_default_env();
    let base = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Text => base.init(),
        LogFormat::Json => base.json().flatten_event(true).init(),
    }
}

fn load_schema(package: &Path) -> Result<serde_json::Value> {
    let schema_path = package.join("values.schema.json");
    let bytes = std::fs::read(&schema_path)
        .with_context(|| format!("reading {}", schema_path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", schema_path.display()))
}

/// Accept either an inline JSON string or a path to a JSON file. The JSON must
/// be an object whose values are strings (or null / scalars, which are coerced
/// via `to_string`). Callers pass these straight to `apply_install_transforms`,
/// which expects `HashMap<String, String>`.
fn load_inputs(inline: Option<&str>, file: Option<&Path>) -> Result<HashMap<String, String>> {
    let value: serde_json::Value = match (inline, file) {
        (Some(s), _) => serde_json::from_str(s).context("parsing --inputs as JSON")?,
        (None, Some(p)) => {
            let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {} as JSON", p.display()))?
        }
        (None, None) => return Ok(HashMap::new()),
    };
    let obj = match value {
        serde_json::Value::Object(o) => o,
        _ => bail!("inputs must be a JSON object of {{path: value}}"),
    };
    Ok(obj
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (k, s)
        })
        .collect())
}

fn validate_or_bail(schema: &serde_json::Value) -> Result<()> {
    if let Some(err) = validate_values_schema(schema) {
        bail!("invalid schema: {err}");
    }
    Ok(())
}

fn run_preview(
    package: &Path,
    inputs_inline: Option<&str>,
    inputs_file: Option<&Path>,
    compact: bool,
) -> Result<()> {
    let schema = load_schema(package)?;
    validate_or_bail(&schema)?;
    let inputs = load_inputs(inputs_inline, inputs_file)?;
    let fields = extract_install_fields(&schema);
    let resolved = apply_install_transforms(&fields, &inputs)
        .map_err(|e| anyhow::anyhow!("resolving inputs: {e}"))?;
    let out = if compact {
        serde_json::to_string(&resolved)?
    } else {
        serde_json::to_string_pretty(&resolved)?
    };
    println!("{out}");
    Ok(())
}

fn load_package(package_dir: &Path, work_dir: &Path) -> Result<(PackageManifest, UmbrellaChart)> {
    let manifest = load_manifest(package_dir)
        .with_context(|| format!("loading package manifest from {}", package_dir.display()))?;

    // Absolutise paths so engines never depend on the process CWD. Safe to
    // call from any thread / concurrent render.
    let abs_work = work_dir
        .canonicalize()
        .with_context(|| format!("resolving work dir {}", work_dir.display()))?;
    let abs_package = package_dir
        .canonicalize()
        .with_context(|| format!("resolving package dir {}", package_dir.display()))?;
    let sources = resolve_source_paths(&manifest.sources, &abs_package)?;

    let umbrella = build_umbrella_chart_in(&manifest.name, &manifest.version, &sources, &abs_work)
        .context("assembling umbrella chart")?;
    Ok((manifest, umbrella))
}

/// Resolve relative paths in KCL / helmfile engine blocks against
/// `base_dir` and confine them to `base_dir`. Absolute paths and `..`
/// traversal are rejected — a malicious `package.yaml` setting
/// `entrypoint: /etc/passwd` would otherwise trigger engine reads of
/// arbitrary host files (leaking contents via error output).
fn resolve_source_paths(
    sources: &[akua_core::Source],
    base_dir: &Path,
) -> Result<Vec<akua_core::Source>> {
    let base = base_dir
        .canonicalize()
        .with_context(|| format!("canonicalising package dir {}", base_dir.display()))?;

    let confine = |field: &str, p: &str| -> Result<String> {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            bail!("source.{field} must be a relative path inside the package ({p} is absolute)");
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                bail!("source.{field} must not contain `..` ({p})");
            }
        }
        let joined = base.join(&path);
        Ok(joined.display().to_string())
    };

    sources
        .iter()
        .map(|s| {
            let mut out = s.clone();
            if let Some(k) = out.kcl.as_mut() {
                k.entrypoint = confine("kcl.entrypoint", &k.entrypoint)?;
            }
            if let Some(hf) = out.helmfile.as_mut() {
                hf.path = confine("helmfile.path", &hf.path)?;
            }
            Ok(out)
        })
        .collect()
}

fn resolve_inputs_to_values(
    package_dir: &Path,
    inline: Option<&str>,
    file: Option<&Path>,
) -> Result<Option<serde_json::Value>> {
    if inline.is_none() && file.is_none() {
        return Ok(None);
    }
    let schema = load_schema(package_dir)?;
    validate_or_bail(&schema)?;
    let inputs = load_inputs(inline, file)?;
    let fields = extract_install_fields(&schema);
    let resolved = apply_install_transforms(&fields, &inputs)
        .map_err(|e| anyhow::anyhow!("resolving inputs: {e}"))?;
    Ok(Some(resolved))
}

fn run_build(package_dir: &Path, out: &Path, strip_metadata: bool) -> Result<()> {
    std::fs::create_dir_all(out)
        .with_context(|| format!("creating output dir {}", out.display()))?;

    // Stage engine-materialised subcharts (KCL / helmfile) in a scratch
    // dir — `fetch_dependencies` will copy them into `out/charts/` below,
    // so we don't want engines writing directly next to the umbrella
    // files and leaving a parallel tree alongside `charts/`.
    let scratch = tempfile::tempdir().context("creating scratch work dir")?;
    let (manifest, mut umbrella) = load_package(package_dir, scratch.path())?;

    // Populate `out/charts/` so the build output is helm-tooling ready:
    //   helm template <out> / helm install <out> / helm package <out>
    // all work without a separate `helm dep update` pass. Fetch runs
    // BEFORE writing Chart.yaml so we can strip the scratch-tempdir
    // file:// URLs from the shipped manifest (below).
    let charts_dir = out.join("charts");
    fetch_dependencies(&umbrella.chart_yaml.dependencies, &charts_dir)
        .with_context(|| format!("populating {}", charts_dir.display()))?;

    // Rewrite engine-materialised file:// deps to helm's in-chart
    // convention (empty repository). They already live in `charts/<alias>/`
    // after the fetch, so helm resolves them locally at template time —
    // the absolute scratch path would only leak the builder's filesystem
    // into a chart that's meant to be shipped via OCI or `.tgz`.
    for dep in &mut umbrella.chart_yaml.dependencies {
        if dep.repository.starts_with("file://") {
            dep.repository.clear();
        }
    }

    write_umbrella(&umbrella, out).context("writing umbrella chart")?;
    eprintln!("wrote {}/Chart.yaml + values.yaml", out.display());
    if !umbrella.chart_yaml.dependencies.is_empty() {
        eprintln!(
            "fetched {} dependenc{} into {}/charts/",
            umbrella.chart_yaml.dependencies.len(),
            if umbrella.chart_yaml.dependencies.len() == 1 {
                "y"
            } else {
                "ies"
            },
            out.display()
        );
    }

    if !strip_metadata {
        // Schema is optional for `build`. If absent, fields list is empty and
        // the transforms section is skipped naturally.
        let fields = load_schema(package_dir)
            .ok()
            .as_ref()
            .map(extract_install_fields)
            .unwrap_or_default();
        let metadata = build_metadata(&manifest.sources, &fields);
        write_metadata(&metadata, out).context("writing .akua/metadata.yaml")?;
        eprintln!("wrote {}/.akua/metadata.yaml", out.display());
    }

    Ok(())
}

fn run_package(chart_dir: &Path, out_dir: &Path) -> Result<()> {
    let outcome = package_chart(chart_dir, out_dir).context("packaging chart")?;
    eprintln!(
        "packaged: {} ({} {}, {} bytes)",
        outcome.path.display(),
        outcome.name,
        outcome.version,
        outcome.size,
    );
    Ok(())
}

fn run_publish(
    chart_dir: &Path,
    to: &str,
    username: Option<String>,
    password: Option<String>,
) -> Result<()> {
    let auth = match (username, password) {
        (Some(u), Some(p)) => Some(akua_core::publish::BasicAuth {
            username: u,
            password: p,
        }),
        (None, None) => None,
        _ => anyhow::bail!("--username and --password must be provided together"),
    };
    let opts = PublishOptions {
        target: to.to_string(),
        auth,
    };
    let outcome = publish_chart(chart_dir, &opts).context("publishing chart")?;
    // pushed_ref on stdout so scripts can capture it; digest to stderr as
    // human-readable confirmation.
    println!("{}", outcome.pushed_ref);
    eprintln!("digest: {}", outcome.digest);
    Ok(())
}

fn read_akua_metadata(chart_dir: &Path) -> Result<AkuaMetadata> {
    let path = chart_dir.join(".akua").join("metadata.yaml");
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "reading {} (chart built with --strip-metadata?)",
            path.display()
        )
    })?;
    serde_yaml::from_slice(&bytes)
        .with_context(|| format!("parsing {} as AkuaMetadata", path.display()))
}

fn read_chart_yaml(chart_dir: &Path) -> Result<(String, String)> {
    let path = chart_dir.join("Chart.yaml");
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let v: serde_yaml::Value =
        serde_yaml::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("Chart.yaml missing `name`"))?
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("Chart.yaml missing `version`"))?
        .to_string();
    Ok((name, version))
}

/// Output keys emitted by `akua inspect` as pretty JSON:
/// - `chartYaml`: parsed Chart.yaml (Helm Chart v2 schema).
/// - `valuesSchema`: parsed values.schema.json when the chart ships one
///   (Helm 3.5+ feature); null otherwise.
/// - `akuaMetadata`: parsed `.akua/metadata.yaml` when the chart is
///   Akua-built; null for vanilla Helm charts.
struct OciAuthArgs {
    username: Option<String>,
    password: Option<String>,
    token: Option<String>,
}

fn run_inspect(
    chart_dir: Option<&Path>,
    oci_ref: Option<&str>,
    auth_args: OciAuthArgs,
) -> Result<()> {
    match (chart_dir, oci_ref) {
        (Some(_), Some(_)) => bail!("--chart and --oci are mutually exclusive"),
        (None, None) => bail!("provide either --chart <dir> or --oci <ref>"),
        (Some(dir), None) => inspect_local(dir),
        (None, Some(r)) => inspect_oci(r, auth_args),
    }
}

fn inspect_local(chart_dir: &Path) -> Result<()> {
    let c = read_chart_contents(chart_dir)?;
    let output = inspect_output_json(c.chart_yaml, c.values_schema, c.akua_metadata, None);
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn inspect_output_json(
    chart_yaml: serde_json::Value,
    values_schema: Option<serde_json::Value>,
    akua_metadata: Option<serde_json::Value>,
    oci_manifest_digest: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "chartYaml": chart_yaml,
        "valuesSchema": values_schema,
        "akuaMetadata": akua_metadata,
        "ociManifestDigest": oci_manifest_digest,
    })
}

fn inspect_oci(reference: &str, auth_args: OciAuthArgs) -> Result<()> {
    let (repository, chart, version) = parse_oci_inspect_ref(reference)?;
    let host = repository
        .trim_start_matches("oci://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let oci_auth = build_oci_auth(&host, auth_args)?;

    let tmp = tempfile::tempdir().context("creating scratch dir")?;
    let charts_dir = tmp.path().join("charts");
    let dep = akua_core::Dependency {
        name: chart.clone(),
        version: version.clone(),
        repository: repository.clone(),
        alias: Some("__inspect".to_string()),
        condition: None,
    };
    akua_core::fetch_dependencies_with_auth(std::slice::from_ref(&dep), &charts_dir, &oci_auth)
        .with_context(|| {
            // Scrub any `user:pass@` that the user may have embedded
            // in the reference/repository.
            format!(
                "pulling {} (chart `{chart}` version `{version}`) from {}",
                redact_userinfo_simple(reference),
                redact_userinfo_simple(&repository),
            )
        })?;

    // Resolve the upstream manifest digest via a single HEAD request so
    // consumers can detect content changes without re-pulling. Best-effort:
    // if the registry misbehaves, emit null rather than failing inspect.
    let oci_ref = akua_core::OciRef::parse(&repository, &chart, &version)
        .context("parsing OCI reference for digest lookup")?;
    let digest = akua_core::fetch_oci_manifest_digest_blocking(&oci_ref, &oci_auth).ok();

    let c = read_chart_contents(&charts_dir.join("__inspect"))?;
    let output = inspect_output_json(
        c.chart_yaml,
        c.values_schema,
        c.akua_metadata,
        digest.as_deref(),
    );
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn build_oci_auth(host: &str, args: OciAuthArgs) -> Result<akua_core::OciAuth> {
    use akua_core::RegistryCredentials;
    let mut auth = akua_core::OciAuth::default();
    match (args.username, args.password, args.token) {
        (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
            bail!("--oci-token is mutually exclusive with --oci-username / --oci-password")
        }
        (Some(u), Some(p), None) => {
            auth.creds.insert(
                host.to_string(),
                RegistryCredentials::Basic {
                    username: u,
                    password: p,
                },
            );
        }
        (Some(_), None, None) | (None, Some(_), None) => {
            bail!("--oci-username and --oci-password must be provided together")
        }
        (None, None, Some(token)) => {
            auth.creds
                .insert(host.to_string(), RegistryCredentials::Bearer(token));
        }
        (None, None, None) => {}
    }
    Ok(auth)
}

/// Require a fully-tagged `oci://host/path/chart:version`. Returns
/// `(repository, chart, version)`. Delegates parsing to
/// `akua_core::source::parse_oci_url` (shared with the fetcher).
/// Strip `user:pass@` userinfo fragments from a URL-like string for
/// use in user-facing error messages. Mirrors the redaction in
/// `akua-core::fetch::redact_userinfo` but kept local so the CLI
/// doesn't depend on a private symbol.
fn redact_userinfo_simple(s: &str) -> String {
    if let Some(colon_slash) = s.find("://") {
        let prefix = &s[..colon_slash + 3];
        let rest = &s[colon_slash + 3..];
        if let Some(at) = rest.find('@') {
            let path_start = rest[..at].find('/').unwrap_or(at);
            if path_start > at {
                return format!("{prefix}<redacted>@{}", &rest[at + 1..]);
            }
            return format!("{prefix}<redacted>@{}", &rest[at + 1..]);
        }
    }
    s.to_string()
}

fn parse_oci_inspect_ref(reference: &str) -> Result<(String, String, String)> {
    let parsed = akua_core::source::parse_oci_url(reference)
        .ok_or_else(|| anyhow::anyhow!("`{reference}` is not a valid oci:// URL"))?;
    let version = parsed.tag.ok_or_else(|| {
        anyhow::anyhow!(
            "`{reference}` missing `:<version>` suffix — inspect needs an exact version"
        )
    })?;
    Ok((parsed.repository, parsed.chart_name, version))
}

#[derive(Debug)]
struct ChartContents {
    chart_yaml: serde_json::Value,
    values_schema: Option<serde_json::Value>,
    akua_metadata: Option<serde_json::Value>,
}

/// Shared by local + OCI inspect — reads whatever files the chart dir
/// happens to have. Missing `values.schema.json` and `.akua/metadata.yaml`
/// are both fine; vanilla Helm charts don't ship either.
fn read_chart_contents(chart_dir: &Path) -> Result<ChartContents> {
    let chart_yaml_path = chart_dir.join("Chart.yaml");
    let chart_bytes = std::fs::read(&chart_yaml_path)
        .with_context(|| format!("reading {}", chart_yaml_path.display()))?;
    let chart_yaml: serde_json::Value = serde_yaml::from_slice(&chart_bytes)
        .with_context(|| format!("parsing {}", chart_yaml_path.display()))?;

    let schema_path = chart_dir.join("values.schema.json");
    let values_schema = match std::fs::read(&schema_path) {
        Ok(bytes) => Some(
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .with_context(|| format!("parsing {}", schema_path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context(format!("reading {}", schema_path.display())),
    };

    let metadata_path = chart_dir.join(".akua").join("metadata.yaml");
    let akua_metadata = match std::fs::read(&metadata_path) {
        Ok(bytes) => Some(
            serde_yaml::from_slice::<serde_json::Value>(&bytes)
                .with_context(|| format!("parsing {}", metadata_path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context(format!("reading {}", metadata_path.display())),
    };

    Ok(ChartContents {
        chart_yaml,
        values_schema,
        akua_metadata,
    })
}

fn run_attest(chart_dir: &Path, out: &Path) -> Result<()> {
    let metadata = read_akua_metadata(chart_dir)?;
    let (name, version) = read_chart_yaml(chart_dir)?;
    let prov = build_provenance(&name, &version, &metadata);
    let json = serde_json::to_string_pretty(&prov)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("wrote {}", out.display());
    eprintln!("sign + push with:");
    eprintln!(
        "  cosign attest --predicate {} --type slsaprovenance1 <image>",
        out.display()
    );
    Ok(())
}

struct RenderArgs<'a> {
    package_dir: &'a Path,
    out: Option<&'a Path>,
    release: &'a str,
    namespace: &'a str,
    engine: &'a str,
    helm_bin: &'a Path,
    inputs_inline: Option<&'a str>,
    inputs_file: Option<&'a Path>,
}

/// Resolve the `--helm-bin` argument to an absolute, non-ambiguous
/// path. PATH-based resolution is rejected for the `helm-cli` engine —
/// a writable directory on `$PATH` (e.g. a malicious drop in
/// `~/.local/bin`) would otherwise let an attacker shadow the real
/// `helm` binary. Accept only absolute paths.
fn resolve_helm_bin(helm_bin: &Path, engine: &str) -> Result<PathBuf> {
    if engine != "helm-cli" {
        return Ok(helm_bin.to_path_buf());
    }
    if helm_bin.is_absolute() {
        return Ok(helm_bin.to_path_buf());
    }
    bail!(
        "--helm-bin must be an absolute path when --engine=helm-cli (got `{}`). \
         PATH-based resolution is rejected to prevent a writable directory on \
         $PATH from shadowing the real `helm` binary. Pass the full path, e.g. \
         `--helm-bin /usr/local/bin/helm`.",
        helm_bin.display()
    );
}

fn run_render(args: RenderArgs<'_>) -> Result<()> {
    // Stage the umbrella in a throwaway temp dir. Render is a transient
    // operation — the durable chart artifact is what `akua build` emits.
    let scratch = tempfile::tempdir().context("creating scratch work dir")?;
    let (_, umbrella) = load_package(args.package_dir, scratch.path())?;
    let override_values =
        resolve_inputs_to_values(args.package_dir, args.inputs_inline, args.inputs_file)?;
    let helm_bin = resolve_helm_bin(args.helm_bin, args.engine)?;
    let opts = RenderOptions {
        helm_bin,
        release_name: args.release.to_string(),
        namespace: args.namespace.to_string(),
        override_values,
    };
    let manifest_yaml = match args.engine {
        "helm-cli" => {
            render_umbrella(&umbrella, scratch.path(), &opts).context("helm-cli render")?
        }
        "helm-wasm" => render_umbrella_embedded(&umbrella, scratch.path(), &opts)
            .context("helm-wasm (embedded) render")?,
        other => anyhow::bail!("unknown --engine `{other}`; expected `helm-wasm` or `helm-cli`"),
    };
    match args.out {
        Some(out) => {
            if let Some(parent) = out.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("creating parent directory {}", parent.display())
                    })?;
                }
            }
            std::fs::write(out, &manifest_yaml)
                .with_context(|| format!("writing {}", out.display()))?;
            eprintln!("wrote {}", out.display());
        }
        None => print!("{manifest_yaml}"),
    }
    Ok(())
}

fn run_tree(package_dir: &Path) -> Result<()> {
    // Engines that materialise subcharts write to a temp dir we throw away —
    // `tree` only cares about the umbrella shape, not the rendered output.
    let scratch = tempfile::tempdir().context("creating scratch work dir")?;
    let (manifest, umbrella) = load_package(package_dir, scratch.path())?;

    println!(
        "{} {} ({} sources)",
        umbrella.chart_yaml.name,
        umbrella.chart_yaml.version,
        manifest.sources.len()
    );
    if umbrella.chart_yaml.dependencies.is_empty() {
        println!("  (no dependencies)");
        return Ok(());
    }
    for dep in &umbrella.chart_yaml.dependencies {
        let alias = dep
            .alias
            .as_deref()
            .map(|a| format!(" as {a}"))
            .unwrap_or_default();
        println!(
            "  - {name}@{version}{alias}  [{repo}]",
            name = dep.name,
            version = dep.version,
            alias = alias,
            repo = dep.repository
        );
    }
    Ok(())
}

fn run_lint(package: &Path) -> Result<()> {
    let schema = load_schema(package)?;
    validate_or_bail(&schema)?;
    let fields = extract_install_fields(&schema);
    println!("schema ok — {} user-input field(s)", fields.len());
    for f in &fields {
        println!("  - {}", f.path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_oci_inspect_ref_splits_host_chart_and_version() {
        let (repo, chart, version) =
            parse_oci_inspect_ref("oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1").unwrap();
        assert_eq!(repo, "oci://ghcr.io/stefanprodan/charts");
        assert_eq!(chart, "podinfo");
        assert_eq!(version, "6.7.1");
    }

    #[test]
    fn parse_oci_inspect_ref_shallow_path() {
        let (repo, chart, version) =
            parse_oci_inspect_ref("oci://registry.local/mychart:1.0.0").unwrap();
        assert_eq!(repo, "oci://registry.local");
        assert_eq!(chart, "mychart");
        assert_eq!(version, "1.0.0");
    }

    #[test]
    fn parse_oci_inspect_ref_preserves_deep_path() {
        let (repo, chart, version) =
            parse_oci_inspect_ref("oci://registry.local/a/b/c/d/leaf-chart:v2.0.0-rc1").unwrap();
        assert_eq!(repo, "oci://registry.local/a/b/c/d");
        assert_eq!(chart, "leaf-chart");
        assert_eq!(version, "v2.0.0-rc1");
    }

    #[test]
    fn parse_oci_inspect_ref_rejects_missing_scheme() {
        // Parser rejects non-oci URLs up-front — inspect turns that into
        // "not a valid oci:// URL".
        let err = parse_oci_inspect_ref("ghcr.io/foo/bar:1.0").unwrap_err();
        assert!(format!("{err}").contains("not a valid oci:// URL"));
    }

    #[test]
    fn parse_oci_inspect_ref_rejects_missing_version() {
        let err = parse_oci_inspect_ref("oci://ghcr.io/foo/bar").unwrap_err();
        assert!(format!("{err}").contains("missing `:<version>`"));
    }

    #[test]
    fn parse_oci_inspect_ref_rejects_empty_version() {
        // Empty tag parses as "no tag" → same error path as truly
        // missing tag, one code branch to reason about.
        let err = parse_oci_inspect_ref("oci://ghcr.io/foo/bar:").unwrap_err();
        assert!(format!("{err}").contains("missing `:<version>`"));
    }

    #[test]
    fn parse_oci_inspect_ref_rejects_missing_chart_path() {
        // `oci://ghcr.io:1.0.0` has no path segments so the shared
        // parser rejects it; inspect surfaces that as "not a valid oci:// URL".
        let err = parse_oci_inspect_ref("oci://ghcr.io:1.0.0").unwrap_err();
        assert!(format!("{err}").contains("not a valid oci:// URL"));
    }

    #[test]
    fn read_chart_contents_returns_nulls_for_vanilla_chart() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Chart.yaml"),
            "apiVersion: v2\nname: vanilla\nversion: 0.1.0\n",
        )
        .unwrap();
        let out = read_chart_contents(tmp.path()).unwrap();
        assert_eq!(out.chart_yaml["name"], "vanilla");
        assert!(out.values_schema.is_none());
        assert!(out.akua_metadata.is_none());
    }

    #[test]
    fn read_chart_contents_includes_values_schema_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Chart.yaml"),
            "apiVersion: v2\nname: x\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("values.schema.json"),
            r#"{"type":"object","properties":{"replicas":{"type":"integer"}}}"#,
        )
        .unwrap();
        let out = read_chart_contents(tmp.path()).unwrap();
        let schema = out.values_schema.expect("values.schema.json");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["replicas"]["type"], "integer");
    }

    #[test]
    fn read_chart_contents_errors_on_missing_chart_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let err = read_chart_contents(tmp.path()).unwrap_err();
        assert!(format!("{err:#}").contains("Chart.yaml"));
    }

    #[test]
    fn log_format_clap_values_parse() {
        use clap::ValueEnum;
        assert_eq!(LogFormat::from_str("text", true).unwrap(), LogFormat::Text);
        assert_eq!(LogFormat::from_str("json", true).unwrap(), LogFormat::Json);
        assert!(LogFormat::from_str("xml", true).is_err());
    }

    #[test]
    fn log_format_defaults_to_text() {
        // Regression guard: the global default is Text. Flipping to Json
        // by accident would break every existing user's stderr.
        let cli = Cli::parse_from(["akua", "tree", "--package", "."]);
        assert_eq!(cli.log_format, LogFormat::Text);
    }

    #[test]
    fn log_format_flag_accepts_json() {
        let cli = Cli::parse_from(["akua", "--log-format", "json", "tree", "--package", "."]);
        assert_eq!(cli.log_format, LogFormat::Json);
    }

    #[test]
    fn command_name_labels_match_subcommands() {
        // Spot-check each variant so we don't forget to add a label when
        // introducing a new subcommand.
        let cli = Cli::parse_from(["akua", "build", "--package", "."]);
        assert_eq!(command_name(&cli.command), "build");
        let cli = Cli::parse_from(["akua", "render", "--package", "."]);
        assert_eq!(command_name(&cli.command), "render");
        let cli = Cli::parse_from(["akua", "inspect", "--chart", "."]);
        assert_eq!(command_name(&cli.command), "inspect");
        let cli = Cli::parse_from(["akua", "package", "--chart", "."]);
        assert_eq!(command_name(&cli.command), "package");
    }

    #[test]
    fn phase_for_matches_command_name() {
        // At the CLI layer phase == command; contract documented on phase_for.
        for argv in [
            vec!["akua", "build", "--package", "."],
            vec!["akua", "render", "--package", "."],
            vec!["akua", "inspect", "--chart", "."],
            vec!["akua", "package", "--chart", "."],
            vec!["akua", "publish", "--to", "oci://x/y"],
        ] {
            let cli = Cli::parse_from(argv);
            assert_eq!(phase_for(&cli.command), command_name(&cli.command));
        }
    }

    #[test]
    fn run_init_creates_starter_files() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("my-pkg");
        run_init(&pkg, None, false).unwrap();

        let pkg_yaml = std::fs::read_to_string(pkg.join("package.yaml")).unwrap();
        assert!(pkg_yaml.contains("apiVersion: akua.dev/v1alpha1"));
        assert!(pkg_yaml.contains("name: my-pkg"));

        let schema = std::fs::read_to_string(pkg.join("values.schema.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&schema).unwrap();
        assert_eq!(parsed["type"], "object");

        assert!(std::fs::read_to_string(pkg.join("README.md"))
            .unwrap()
            .contains("# my-pkg"));
    }

    #[test]
    fn run_init_name_override_wins() {
        let tmp = tempfile::tempdir().unwrap();
        run_init(tmp.path(), Some("override"), false).unwrap();
        let pkg_yaml = std::fs::read_to_string(tmp.path().join("package.yaml")).unwrap();
        assert!(pkg_yaml.contains("name: override"));
    }

    #[test]
    fn run_init_refuses_to_clobber_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.yaml"), "existing").unwrap();
        let err = run_init(tmp.path(), Some("x"), false).unwrap_err();
        assert!(format!("{err}").contains("--force"));
        // Untouched by a failed init.
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("package.yaml")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn run_init_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.yaml"), "stale").unwrap();
        run_init(tmp.path(), Some("x"), true).unwrap();
        let pkg_yaml = std::fs::read_to_string(tmp.path().join("package.yaml")).unwrap();
        assert!(pkg_yaml.contains("name: x"));
    }
}
