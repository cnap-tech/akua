//! `akua init` — scaffold a new Package.
//!
//! Spec: [`docs/cli.md`](../../../../docs/cli.md) `akua init` section.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub struct InitArgs<'a> {
    /// Target directory. When `name` is provided it's `<cwd>/<name>`;
    /// when absent the caller passes the current directory.
    pub target: &'a Path,

    /// Package name written into `akua.toml`. Usually derived from the
    /// target directory's file name at the CLI layer.
    pub package_name: &'a str,

    /// Overwrite existing scaffold files when set.
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InitOutput {
    /// Package name recorded in the new `akua.toml`.
    pub name: String,

    /// Absolute target directory the scaffold was written into.
    pub path: PathBuf,

    /// Files written, relative to `path`. Stable order for idempotent
    /// diffs against prior runs.
    pub files: Vec<&'static str>,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{file} already exists; pass --force to overwrite")]
    Exists { file: PathBuf },

    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("package name must be non-empty")]
    EmptyName,

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl InitError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            InitError::Exists { file } => StructuredError::new(
                codes::E_INIT_EXISTS,
                "target directory already contains scaffold files",
            )
            .with_path(file.display().to_string())
            .with_suggestion("pass --force to overwrite, or pick a different directory")
            .with_default_docs(),
            InitError::Io { path, source } => StructuredError::new(codes::E_IO, source.to_string())
                .with_path(path.display().to_string())
                .with_default_docs(),
            InitError::EmptyName => {
                StructuredError::new(codes::E_INIT_EMPTY_NAME, self.to_string())
                    .with_suggestion("pass `akua init <name>` or run from a named directory")
                    .with_default_docs()
            }
            InitError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            InitError::Io { .. } | InitError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

/// Scaffold the files into `args.target`. The directory is created if
/// missing. Existing files are left alone unless `args.force` is set.
pub fn run<W: Write>(
    ctx: &Context,
    args: &InitArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, InitError> {
    if args.package_name.is_empty() {
        return Err(InitError::EmptyName);
    }

    std::fs::create_dir_all(args.target).map_err(|e| InitError::Io {
        path: args.target.to_path_buf(),
        source: e,
    })?;

    let files = [
        ("akua.toml", akua_toml(args.package_name)),
        ("package.k", PACKAGE_K.to_string()),
        ("inputs.example.yaml", INPUTS_EXAMPLE.to_string()),
    ];

    for (filename, _) in &files {
        let path = args.target.join(filename);
        if path.exists() && !args.force {
            return Err(InitError::Exists { file: path });
        }
    }

    for (filename, contents) in &files {
        let path = args.target.join(filename);
        std::fs::write(&path, contents).map_err(|e| InitError::Io {
            path: path.clone(),
            source: e,
        })?;
    }

    let output = InitOutput {
        name: args.package_name.to_string(),
        path: absolute(args.target),
        files: files.iter().map(|(n, _)| *n).collect(),
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(InitError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn absolute(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn write_text<W: Write>(writer: &mut W, output: &InitOutput) -> std::io::Result<()> {
    writeln!(
        writer,
        "scaffolded {} at {}",
        output.name,
        output.path.display()
    )?;
    for file in &output.files {
        writeln!(writer, "  + {file}")?;
    }
    Ok(())
}

fn akua_toml(name: &str) -> String {
    format!(
        "[package]\nname    = \"{name}\"\nversion = \"0.1.0\"\nedition = \"akua.dev/v1alpha1\"\n\n[dependencies]\n"
    )
}

const PACKAGE_K: &str = r#"# Minimal Package.k — edit me.
#
# See docs/package-format.md for the full authoring reference.

import akua.ctx

schema Input:
    """Public inputs for this Package."""

    appName: str
    """Application name. Used as resource-name prefix."""

    replicas: int = 2
    """Number of replicas."""

    check:
        replicas >= 1, "replicas must be at least 1"

input: Input = ctx.input()

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
"#;

const INPUTS_EXAMPLE: &str = r#"appName: hello
replicas: 3
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn args<'a>(dir: &'a Path, name: &'a str) -> InitArgs<'a> {
        InitArgs {
            target: dir,
            package_name: name,
            force: false,
        }
    }

    #[test]
    fn scaffolds_all_three_files() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("my-pkg");
        let mut stdout = Vec::new();
        let code = run(&Context::human(), &args(&pkg, "my-pkg"), &mut stdout).expect("run");
        assert_eq!(code, ExitCode::Success);
        assert!(pkg.join("akua.toml").is_file());
        assert!(pkg.join("package.k").is_file());
        assert!(pkg.join("inputs.example.yaml").is_file());
    }

    #[test]
    fn json_output_lists_files_and_resolved_path() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("demo");
        let ctx = Context::json();
        let mut stdout = Vec::new();
        run(&ctx, &args(&pkg, "demo"), &mut stdout).expect("run");

        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["name"], "demo");
        assert_eq!(
            parsed["files"].as_array().unwrap().len(),
            3,
            "expected 3 scaffolded files"
        );
        assert!(parsed["path"].as_str().unwrap().ends_with("demo"));
    }

    #[test]
    fn refuses_to_clobber_existing_files() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("existing");
        fs::create_dir(&pkg).unwrap();
        fs::write(pkg.join("package.k"), "existing content").unwrap();

        let err = run(&Context::human(), &args(&pkg, "existing"), &mut Vec::new()).unwrap_err();
        assert!(matches!(err, InitError::Exists { .. }));
        assert_eq!(err.to_structured().code, codes::E_INIT_EXISTS);
        assert_eq!(err.exit_code(), ExitCode::UserError);

        // Pre-existing file untouched.
        assert_eq!(
            fs::read_to_string(pkg.join("package.k")).unwrap(),
            "existing content"
        );
    }

    #[test]
    fn force_overwrites_existing_files() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("existing");
        fs::create_dir(&pkg).unwrap();
        fs::write(pkg.join("package.k"), "stale").unwrap();

        let a = InitArgs {
            force: true,
            ..args(&pkg, "existing")
        };
        run(&Context::human(), &a, &mut Vec::new()).expect("run");
        assert!(fs::read_to_string(pkg.join("package.k"))
            .unwrap()
            .contains("schema Input"));
    }

    #[test]
    fn creates_target_directory_when_missing() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("nested").join("deeper").join("pkg");
        run(&Context::human(), &args(&pkg, "pkg"), &mut Vec::new()).expect("run");
        assert!(pkg.join("akua.toml").is_file());
    }

    #[test]
    fn empty_package_name_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let err = run(&Context::human(), &args(tmp.path(), ""), &mut Vec::new()).unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_INIT_EMPTY_NAME);
    }

    #[test]
    fn scaffolded_akua_toml_and_package_k_round_trip_through_parsers() {
        // Regression guard: the scaffold must produce files the rest of
        // the toolchain accepts without modification.
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("smoke");
        run(&Context::human(), &args(&pkg, "smoke"), &mut Vec::new()).expect("run");

        let toml = fs::read_to_string(pkg.join("akua.toml")).unwrap();
        let manifest = akua_core::AkuaManifest::parse(&toml).expect("akua.toml parses");
        assert_eq!(manifest.package.name, "smoke");

        let loaded = akua_core::PackageK::load(&pkg.join("package.k")).expect("package.k loads");
        let inputs =
            serde_yaml::from_str(&fs::read_to_string(pkg.join("inputs.example.yaml")).unwrap())
                .expect("inputs.yaml parses");
        // Render through the wasmtime sandbox — same path `akua render`
        // uses in production. Scaffold must be valid all the way
        // through, not just parseable.
        let rendered = crate::verbs::render::render_in_worker(
            &loaded,
            &inputs,
            &akua_core::chart_resolver::ResolvedCharts::default(),
            false,
            akua_core::kcl_plugin::BudgetSnapshot::default(),
        )
        .expect("renders");
        assert_eq!(rendered.resources.len(), 1);
    }
}
