//! # akua
//!
//! Command-line tool for Akua. Scaffolds, previews, tests, builds, and
//! publishes cloud-native packages.
//!
//! ## Status
//!
//! Pre-alpha. Subcommand shape is stabilizing; expect breaking changes.

use clap::{Parser, Subcommand};

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
    /// Preview the resolved manifests for a given set of inputs.
    Preview {
        /// JSON inputs (inline).
        #[arg(long)]
        inputs: Option<String>,
        /// Path to JSON inputs file.
        #[arg(long)]
        inputs_file: Option<String>,
        /// Emit JSON (for scripts / agents) instead of human-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Run package tests (`resolve.test.*`, schema validation, etc.).
    Test,
    /// Lint the package (schema, transforms, Helm render sanity).
    Lint,
    /// Build the package into an OCI-ready artifact.
    Build {
        /// Output directory for the built artifact.
        #[arg(long, default_value = "./dist")]
        out: String,
    },
    /// Publish the built package to an OCI registry.
    Publish {
        /// Target OCI reference (e.g., oci://ghcr.io/org/my-pkg).
        #[arg(long)]
        to: String,
    },
    /// Run the MCP server exposing Akua tools to AI coding agents.
    Mcp,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { from: _ } => {
            eprintln!("akua init — not yet implemented (milestone v4)");
        }
        Commands::Preview { .. } => {
            eprintln!("akua preview — not yet implemented (milestone v4)");
        }
        Commands::Test => {
            eprintln!("akua test — not yet implemented (milestone v4)");
        }
        Commands::Lint => {
            eprintln!("akua lint — not yet implemented (milestone v4)");
        }
        Commands::Build { out: _ } => {
            eprintln!("akua build — not yet implemented (milestone v4)");
        }
        Commands::Publish { to: _ } => {
            eprintln!("akua publish — not yet implemented (milestone v4)");
        }
        Commands::Mcp => {
            eprintln!("akua mcp — not yet implemented (milestone v5)");
        }
    }

    Ok(())
}
