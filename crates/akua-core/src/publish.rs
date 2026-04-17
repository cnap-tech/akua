//! Publish a built chart directory to an OCI registry.
//!
//! Uses Helm 3.8+'s native OCI support (`helm package` + `helm push`). No
//! separate oras dep needed — we already require `helm` for render.
//!
//! Flow:
//!
//! ```text
//!   chart_dir/  ──helm package──►  chart_dir.tgz  ──helm push──►  oci://registry/ns
//!                                                                           │
//!                                                                           ▼
//!                                                        returns sha256 digest
//! ```
//!
//! The caller provides the registry *namespace* (e.g.,
//! `oci://ghcr.io/acme/charts`). Helm derives the final repository name from
//! the chart's `Chart.yaml` (`name`). Pushing chart `my-app@1.0.0` to
//! `oci://ghcr.io/acme/charts` lands as `oci://ghcr.io/acme/charts/my-app:1.0.0`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("running `{cmd}`: {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{cmd}` exited with status {status}:\n{stderr}")]
    HelmFailed {
        cmd: String,
        status: i32,
        stderr: String,
    },
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no .tgz tarball found in {dir}")]
    NoTarball { dir: PathBuf },
    #[error("could not parse digest from `helm push` output:\n{output}")]
    DigestNotFound { output: String },
}

#[derive(Debug, Clone)]
pub struct PublishOptions {
    pub helm_bin: PathBuf,
    /// OCI namespace URL, e.g., `oci://ghcr.io/acme/charts`.
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct PublishOutcome {
    /// Pushed reference, including the chart name derived from Chart.yaml.
    pub pushed_ref: String,
    /// OCI digest of the manifest (`sha256:...`).
    pub digest: String,
}

/// Package the chart dir to a tarball and push to `opts.target`.
pub fn publish_chart(
    chart_dir: &Path,
    opts: &PublishOptions,
) -> Result<PublishOutcome, PublishError> {
    let tmp = tempfile::tempdir().map_err(|source| PublishError::Read {
        path: PathBuf::from("<tempdir>"),
        source,
    })?;

    // `helm package` writes <name>-<version>.tgz into -d <dir>.
    let package_out = Command::new(&opts.helm_bin)
        .arg("package")
        .arg(chart_dir)
        .arg("-d")
        .arg(tmp.path())
        .output()
        .map_err(|source| PublishError::Spawn {
            cmd: format!("{} package", opts.helm_bin.display()),
            source,
        })?;
    if !package_out.status.success() {
        return Err(PublishError::HelmFailed {
            cmd: "helm package".to_string(),
            status: package_out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&package_out.stderr).into_owned(),
        });
    }

    let tarball = find_tarball(tmp.path())?;

    let push_out = Command::new(&opts.helm_bin)
        .arg("push")
        .arg(&tarball)
        .arg(&opts.target)
        .output()
        .map_err(|source| PublishError::Spawn {
            cmd: format!("{} push", opts.helm_bin.display()),
            source,
        })?;
    if !push_out.status.success() {
        return Err(PublishError::HelmFailed {
            cmd: "helm push".to_string(),
            status: push_out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&push_out.stderr).into_owned(),
        });
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&push_out.stdout),
        String::from_utf8_lossy(&push_out.stderr)
    );
    parse_push_output(&combined, &opts.target, &tarball)
}

fn find_tarball(dir: &Path) -> Result<PathBuf, PublishError> {
    let entries = std::fs::read_dir(dir).map_err(|source| PublishError::Read {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("tgz") {
            return Ok(path);
        }
    }
    Err(PublishError::NoTarball {
        dir: dir.to_path_buf(),
    })
}

/// `helm push` prints lines like:
///
/// ```text
/// Pushed: ghcr.io/acme/charts/my-app:1.0.0
/// Digest: sha256:abc...
/// ```
fn parse_push_output(
    output: &str,
    target: &str,
    tarball: &Path,
) -> Result<PublishOutcome, PublishError> {
    let mut pushed_ref: Option<String> = None;
    let mut digest: Option<String> = None;
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("Pushed:") {
            pushed_ref = Some(rest.trim().to_string());
        } else if let Some(rest) = line.trim().strip_prefix("Digest:") {
            digest = Some(rest.trim().to_string());
        }
    }

    let pushed_ref = pushed_ref.unwrap_or_else(|| derive_ref(target, tarball));
    let digest = digest.ok_or_else(|| PublishError::DigestNotFound {
        output: output.to_string(),
    })?;
    Ok(PublishOutcome { pushed_ref, digest })
}

/// Fallback: derive the pushed reference from target + tarball filename when
/// Helm's stdout format changes.
fn derive_ref(target: &str, tarball: &Path) -> String {
    let stem = tarball
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    format!("{target}/{stem}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_helm_push_output() {
        let out = "Pushed: ghcr.io/acme/charts/my-app:1.0.0\nDigest: sha256:abc123\n";
        let outcome =
            parse_push_output(out, "oci://ghcr.io/acme/charts", Path::new("my-app-1.0.0.tgz"))
                .unwrap();
        assert_eq!(outcome.pushed_ref, "ghcr.io/acme/charts/my-app:1.0.0");
        assert_eq!(outcome.digest, "sha256:abc123");
    }

    #[test]
    fn missing_digest_errors() {
        let out = "Pushed: somewhere\n"; // no Digest: line
        let err = parse_push_output(out, "oci://r", Path::new("x.tgz")).unwrap_err();
        assert!(matches!(err, PublishError::DigestNotFound { .. }));
    }

    #[test]
    fn falls_back_to_derived_ref_when_pushed_line_absent() {
        let out = "Digest: sha256:xyz\n";
        let outcome =
            parse_push_output(out, "oci://ghcr.io/acme/charts", Path::new("my-app-1.0.0.tgz"))
                .unwrap();
        assert_eq!(outcome.pushed_ref, "oci://ghcr.io/acme/charts/my-app-1.0.0");
    }
}
