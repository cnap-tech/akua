//! `akua inspect` — report a Package's input surface OR a packed
//! tarball's metadata, without executing either.
//!
//! Two modes:
//!
//! - **Package mode** (`--package`): parse the Package.k, enumerate
//!   every `option()` call-site. Agents use the JSON shape to
//!   discover what inputs the Package expects before invoking it.
//! - **Tarball mode** (`--tarball`): read a packed `.tar.gz` in-memory
//!   without unpacking, report name/version/edition, layer digest,
//!   file count, and vendored deps. Pair with `akua pack` +
//!   `akua push` — operators triage an air-gap-transferred tarball
//!   before pushing it.

use std::io::Write;
use std::path::{Path, PathBuf};

use akua_core::cli_contract::{codes, ExitCode, StructuredError};
use akua_core::{list_options_kcl, package_tar, OptionInfo, PackageKError};
use serde::Serialize;

use crate::contract::{emit_output, Context};

#[derive(Debug, Clone)]
pub enum InspectTarget<'a> {
    Package(&'a Path),
    Tarball(&'a Path),
}

#[derive(Debug, Clone)]
pub struct InspectArgs<'a> {
    pub target: InspectTarget<'a>,
}

/// Discriminated JSON shape. `kind: "package"|"tarball"` carries the
/// variant; consumers parse one body and branch.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InspectOutput {
    Package(PackageInspectBody),
    Tarball(TarballInspectBody),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageInspectBody {
    pub path: PathBuf,
    pub options: Vec<OptionInfo>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TarballInspectBody {
    pub path: PathBuf,
    pub layer_digest: String,
    pub compressed_size_bytes: u64,
    pub uncompressed_size_bytes: u64,
    pub file_count: usize,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub package_edition: Option<String>,
    pub vendored_deps: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Kcl(#[from] PackageKError),

    #[error(transparent)]
    Tarball(#[from] package_tar::PackageTarError),

    #[error("write to stdout failed: {0}")]
    StdoutWrite(#[source] std::io::Error),
}

impl InspectError {
    pub fn to_structured(&self) -> StructuredError {
        match self {
            InspectError::Io { path, source } => {
                let code = if source.kind() == std::io::ErrorKind::NotFound {
                    codes::E_PACKAGE_MISSING
                } else {
                    codes::E_IO
                };
                StructuredError::new(code, source.to_string())
                    .with_path(path.display().to_string())
                    .with_default_docs()
            }
            InspectError::Kcl(e) => {
                StructuredError::new(codes::E_INSPECT_FAIL, e.to_string()).with_default_docs()
            }
            InspectError::Tarball(e) => {
                StructuredError::new(codes::E_INSPECT_FAIL, e.to_string()).with_default_docs()
            }
            InspectError::StdoutWrite(e) => {
                StructuredError::new(codes::E_IO, e.to_string()).with_default_docs()
            }
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            InspectError::Io { source, .. } if source.kind() != std::io::ErrorKind::NotFound => {
                ExitCode::SystemError
            }
            InspectError::StdoutWrite(_) => ExitCode::SystemError,
            _ => ExitCode::UserError,
        }
    }
}

pub fn run<W: Write>(
    ctx: &Context,
    args: &InspectArgs<'_>,
    stdout: &mut W,
) -> Result<ExitCode, InspectError> {
    let output = match &args.target {
        InspectTarget::Package(path) => inspect_package(path)?,
        InspectTarget::Tarball(path) => inspect_tarball(path)?,
    };

    emit_output(stdout, ctx, &output, |w| write_text(w, &output))
        .map_err(InspectError::StdoutWrite)?;
    Ok(ExitCode::Success)
}

fn inspect_package(path: &Path) -> Result<InspectOutput, InspectError> {
    if !path.exists() {
        return Err(InspectError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found", path.display()),
            ),
        });
    }
    let options = list_options_kcl(path)?;
    Ok(InspectOutput::Package(PackageInspectBody {
        path: path.to_path_buf(),
        options,
    }))
}

fn inspect_tarball(path: &Path) -> Result<InspectOutput, InspectError> {
    let bytes = std::fs::read(path).map_err(|source| InspectError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let insp = package_tar::inspect(&bytes)?;
    Ok(InspectOutput::Tarball(TarballInspectBody {
        path: path.to_path_buf(),
        layer_digest: insp.layer_digest,
        compressed_size_bytes: insp.compressed_size_bytes,
        uncompressed_size_bytes: insp.uncompressed_size_bytes,
        file_count: insp.file_count,
        package_name: insp.package_name,
        package_version: insp.package_version,
        package_edition: insp.package_edition,
        vendored_deps: insp.vendored_deps,
    }))
}

fn write_text<W: Write>(w: &mut W, output: &InspectOutput) -> std::io::Result<()> {
    match output {
        InspectOutput::Package(body) => write_package_text(w, body),
        InspectOutput::Tarball(body) => write_tarball_text(w, body),
    }
}

fn write_package_text<W: Write>(w: &mut W, body: &PackageInspectBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.path.display())?;
    if body.options.is_empty() {
        writeln!(w, "  (no options)")?;
        return Ok(());
    }
    writeln!(w, "  options:")?;
    for o in &body.options {
        let required = if o.required { " [required]" } else { "" };
        let ty = if o.r#type.is_empty() {
            String::new()
        } else {
            format!(": {}", o.r#type)
        };
        let default = o
            .default
            .as_deref()
            .map(|d| format!(" = {d}"))
            .unwrap_or_default();
        writeln!(w, "    - {}{}{}{}", o.name, ty, default, required)?;
        if let Some(help) = &o.help {
            writeln!(w, "        {help}")?;
        }
    }
    Ok(())
}

fn write_tarball_text<W: Write>(w: &mut W, body: &TarballInspectBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.path.display())?;
    if let (Some(name), Some(ver)) = (&body.package_name, &body.package_version) {
        writeln!(w, "  package   {name} {ver}")?;
    } else {
        writeln!(w, "  package   (no akua.toml in tarball)")?;
    }
    if let Some(edition) = &body.package_edition {
        writeln!(w, "  edition   {edition}")?;
    }
    writeln!(w, "  layer     {}", body.layer_digest)?;
    writeln!(
        w,
        "  size      {} compressed / {} uncompressed",
        body.compressed_size_bytes, body.uncompressed_size_bytes
    )?;
    writeln!(w, "  files     {}", body.file_count)?;
    if !body.vendored_deps.is_empty() {
        writeln!(w, "  vendored: {}", body.vendored_deps.join(", "))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const TYPED_INPUT: &str = r#"
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = []
"#;

    const NO_OPTIONS: &str = r#"
resources = []
"#;

    fn write_pkg(body: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("package.k");
        fs::write(&p, body).unwrap();
        (tmp, p)
    }

    fn minimal_workspace(tmp: &Path) {
        std::fs::write(
            tmp.join("akua.toml"),
            b"[package]\nname = \"inspect-test\"\nversion = \"0.4.2\"\nedition = \"akua.dev/v1alpha1\"\n",
        )
        .unwrap();
        std::fs::write(tmp.join("package.k"), b"resources = []\n").unwrap();
    }

    #[test]
    fn lists_the_canonical_input_option_in_package_mode() {
        let (_tmp, path) = write_pkg(TYPED_INPUT);
        let mut stdout = Vec::new();
        let code = run(
            &Context::human(),
            &InspectArgs {
                target: InspectTarget::Package(&path),
            },
            &mut stdout,
        )
        .expect("run");
        assert_eq!(code, ExitCode::Success);
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("- input"), "{text}");
    }

    #[test]
    fn package_mode_json_carries_kind_discriminator() {
        let (_tmp, path) = write_pkg(TYPED_INPUT);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &InspectArgs {
                target: InspectTarget::Package(&path),
            },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["kind"], "package");
        assert!(parsed["path"].as_str().unwrap().ends_with("package.k"));
        assert_eq!(parsed["options"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn package_with_no_options_reports_empty_list() {
        let (_tmp, path) = write_pkg(NO_OPTIONS);
        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &InspectArgs {
                target: InspectTarget::Package(&path),
            },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert!(parsed["options"].as_array().unwrap().is_empty());
    }

    #[test]
    fn missing_package_surfaces_typed_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.k");
        let err = run(
            &Context::human(),
            &InspectArgs {
                target: InspectTarget::Package(&missing),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.to_structured().code, codes::E_PACKAGE_MISSING);
    }

    #[test]
    fn tarball_mode_reports_package_fields_and_layer_digest() {
        use akua_core::package_tar;
        let tmp = TempDir::new().unwrap();
        minimal_workspace(tmp.path());
        let bytes = package_tar::pack_workspace(tmp.path()).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        std::fs::write(&tar_path, &bytes).unwrap();

        let mut stdout = Vec::new();
        run(
            &Context::json(),
            &InspectArgs {
                target: InspectTarget::Tarball(&tar_path),
            },
            &mut stdout,
        )
        .expect("run");
        let parsed: serde_json::Value =
            serde_json::from_str(String::from_utf8(stdout).unwrap().trim()).unwrap();
        assert_eq!(parsed["kind"], "tarball");
        assert_eq!(parsed["package_name"], "inspect-test");
        assert_eq!(parsed["package_version"], "0.4.2");
        assert_eq!(parsed["package_edition"], "akua.dev/v1alpha1");
        assert!(parsed["layer_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(
            parsed["compressed_size_bytes"].as_u64().unwrap(),
            bytes.len() as u64
        );
    }

    #[test]
    fn tarball_mode_surfaces_text_output_readable_to_humans() {
        use akua_core::package_tar;
        let tmp = TempDir::new().unwrap();
        minimal_workspace(tmp.path());
        let bytes = package_tar::pack_workspace(tmp.path()).unwrap();
        let tar_path = tmp.path().join("p.tgz");
        std::fs::write(&tar_path, &bytes).unwrap();

        let mut stdout = Vec::new();
        run(
            &Context::human(),
            &InspectArgs {
                target: InspectTarget::Tarball(&tar_path),
            },
            &mut stdout,
        )
        .unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("inspect-test 0.4.2"), "{text}");
        assert!(text.contains("sha256:"), "{text}");
        assert!(text.contains("files"), "{text}");
    }

    #[test]
    fn tarball_mode_missing_file_surfaces_typed_io_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("gone.tgz");
        let err = run(
            &Context::human(),
            &InspectArgs {
                target: InspectTarget::Tarball(&missing),
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(err, InspectError::Io { .. }));
        assert_eq!(err.exit_code(), ExitCode::UserError);
    }
}
