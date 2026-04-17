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
    extract_install_fields, load_manifest, publish_chart, render_umbrella,
    render_umbrella_embedded, validate_values_schema, write_metadata, write_umbrella,
    AkuaMetadata, PackageManifest, PublishOptions, RenderOptions, UmbrellaChart,
};

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
    /// Scaffold a new package in the current directory.
    Init {
        /// Scaffold around an existing public Helm chart.
        #[arg(long)]
        from: Option<String>,
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
    Inspect {
        #[arg(long, default_value = "./dist/chart")]
        chart: PathBuf,
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
        #[arg(long, default_value = "./dist/chart")]
        out: PathBuf,
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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        Commands::Preview { package, inputs, inputs_file, compact } => {
            run_preview(&package, inputs.as_deref(), inputs_file.as_deref(), compact)
        }
        Commands::Lint { package } => run_lint(&package),
        Commands::Tree { package } => run_tree(&package),
        Commands::Build {
            package,
            out,
            strip_metadata,
        } => run_build(&package, &out, strip_metadata),
        Commands::Inspect { chart } => run_inspect(&chart),
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
            out: &out,
            release: &release,
            namespace: &namespace,
            engine: &engine,
            helm_bin: &helm_bin,
            inputs_inline: inputs.as_deref(),
            inputs_file: inputs_file.as_deref(),
        }),
        Commands::Init { .. } => stub("init"),
        Commands::Test => stub("test"),
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

fn load_package(
    package_dir: &Path,
    work_dir: &Path,
) -> Result<(PackageManifest, UmbrellaChart)> {
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
    let sources = resolve_source_paths(&manifest.sources, &abs_package);

    let umbrella = build_umbrella_chart_in(&manifest.name, &manifest.version, &sources, &abs_work)
        .context("assembling umbrella chart")?;
    Ok((manifest, umbrella))
}

/// Rewrite any `file://<relative>` URL in a source's `chart.repoUrl` to an
/// absolute `file://<abs>` form, resolved against `base_dir`. Engines that
/// load local files (KCL entrypoints, helmfile.yaml) see absolute paths only
/// — no CWD dependency, safe for concurrent use.
fn resolve_source_paths(
    sources: &[akua_core::HelmSource],
    base_dir: &Path,
) -> Vec<akua_core::HelmSource> {
    sources
        .iter()
        .map(|s| {
            let mut out = s.clone();
            if let Some(rest) = out.chart.repo_url.strip_prefix("file://") {
                let path = PathBuf::from(rest);
                let abs = if path.is_absolute() {
                    path
                } else {
                    base_dir.join(path)
                };
                out.chart.repo_url = format!("file://{}", abs.display());
            }
            if let Some(p) = &out.chart.path {
                let path = PathBuf::from(p);
                if !path.is_absolute() && !out.chart.repo_url.starts_with("http")
                    && !out.chart.repo_url.starts_with("oci://")
                {
                    out.chart.path = Some(base_dir.join(path).display().to_string());
                }
            }
            out
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
    let (manifest, umbrella) = load_package(package_dir, out)?;
    write_umbrella(&umbrella, out).context("writing umbrella chart")?;
    println!("wrote {}/Chart.yaml + values.yaml", out.display());

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
        println!("wrote {}/.akua/metadata.yaml", out.display());
    }

    if !umbrella.git_sources.is_empty() {
        eprintln!(
            "note: {} git source(s) skipped — Helm cannot fetch these",
            umbrella.git_sources.len()
        );
    }
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
    println!("pushed: {}", outcome.pushed_ref);
    println!("digest: {}", outcome.digest);
    Ok(())
}

fn read_akua_metadata(chart_dir: &Path) -> Result<AkuaMetadata> {
    let path = chart_dir.join(".akua").join("metadata.yaml");
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (chart built with --strip-metadata?)", path.display()))?;
    serde_yaml::from_slice(&bytes)
        .with_context(|| format!("parsing {} as AkuaMetadata", path.display()))
}

fn read_chart_yaml(chart_dir: &Path) -> Result<(String, String)> {
    let path = chart_dir.join("Chart.yaml");
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let v: serde_yaml::Value = serde_yaml::from_slice(&bytes)
        .with_context(|| format!("parsing {}", path.display()))?;
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

fn run_inspect(chart_dir: &Path) -> Result<()> {
    let path = chart_dir.join(".akua").join("metadata.yaml");
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {} (chart built with --strip-metadata?)", path.display()))?;
    print!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
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
    println!("wrote {}", out.display());
    println!("sign + push with:");
    println!(
        "  cosign attest --predicate {} --type slsaprovenance1 <image>",
        out.display()
    );
    Ok(())
}

struct RenderArgs<'a> {
    package_dir: &'a Path,
    out: &'a Path,
    release: &'a str,
    namespace: &'a str,
    engine: &'a str,
    helm_bin: &'a Path,
    inputs_inline: Option<&'a str>,
    inputs_file: Option<&'a Path>,
}

fn run_render(args: RenderArgs<'_>) -> Result<()> {
    std::fs::create_dir_all(args.out)
        .with_context(|| format!("creating output dir {}", args.out.display()))?;
    let (_, umbrella) = load_package(args.package_dir, args.out)?;
    let override_values =
        resolve_inputs_to_values(args.package_dir, args.inputs_inline, args.inputs_file)?;
    let opts = RenderOptions {
        helm_bin: args.helm_bin.to_path_buf(),
        release_name: args.release.to_string(),
        namespace: args.namespace.to_string(),
        override_values,
    };
    let manifest_yaml = match args.engine {
        "helm-cli" => render_umbrella(&umbrella, args.out, &opts).context("helm-cli render")?,
        "helm-wasm" => render_umbrella_embedded(&umbrella, args.out, &opts)
            .context("helm-wasm (embedded) render")?,
        other => anyhow::bail!(
            "unknown --engine `{other}`; expected `helm-wasm` or `helm-cli`"
        ),
    };
    print!("{manifest_yaml}");
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
    if umbrella.chart_yaml.dependencies.is_empty() && umbrella.git_sources.is_empty() {
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
    for git in &umbrella.git_sources {
        let path = git.chart.path.as_deref().unwrap_or(".");
        println!(
            "  - (git) {repo}@{rev} path={path}",
            repo = git.chart.repo_url,
            rev = git.chart.target_revision,
            path = path
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
