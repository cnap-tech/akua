//! # akua
//!
//! Command-line tool for Akua. Scaffolds, previews, tests, builds, and
//! publishes cloud-native packages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{ArgGroup, Parser, Subcommand};

use akua_core::{apply_install_transforms, extract_install_fields, validate_values_schema};

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
    /// Run package tests (`resolve.test.*`, schema validation, etc.).
    Test,
    /// Lint the package (schema validation, transform wiring).
    Lint {
        #[arg(long, default_value = ".")]
        package: PathBuf,
    },
    /// Build the package into an OCI-ready artifact.
    Build {
        #[arg(long, default_value = "./dist")]
        out: String,
    },
    /// Publish the built package to an OCI registry.
    Publish {
        #[arg(long)]
        to: String,
    },
    /// Run the MCP server exposing Akua tools to AI coding agents.
    Mcp,
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
        Commands::Init { .. } => stub("init"),
        Commands::Test => stub("test"),
        Commands::Build { .. } => stub("build"),
        Commands::Publish { .. } => stub("publish"),
        Commands::Mcp => stub("mcp"),
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
