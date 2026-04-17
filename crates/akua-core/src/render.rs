//! Helm render — shells to the `helm` binary to fetch dependencies and render
//! an umbrella chart into Kubernetes manifests.
//!
//! Strategy: write the umbrella chart (Chart.yaml + values.yaml) to a target
//! directory, run `helm dependency update` to pull deps into `charts/`, then
//! `helm template` to render. This offloads all fetching — HTTP Helm repos,
//! OCI registries, auth — to Helm itself rather than reimplementing in Rust.
//!
//! Git sources are **not** handled here. Callers must clone them separately
//! and merge their manifests after Helm render.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::umbrella::UmbrellaChart;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("writing chart files to {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serializing {what}: {source}")]
    Serialize {
        what: &'static str,
        #[source]
        source: serde_yaml::Error,
    },
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
    #[error("`{cmd}` output was not valid UTF-8")]
    NonUtf8Output { cmd: String },
}

/// Configuration for a Helm render.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub helm_bin: PathBuf,
    pub release_name: String,
    pub namespace: String,
    /// Values JSON merged on top of the umbrella's `values.yaml`. Typically
    /// the output of [`apply_install_transforms`].
    pub override_values: Option<serde_json::Value>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            helm_bin: PathBuf::from("helm"),
            release_name: "release".to_string(),
            namespace: "default".to_string(),
            override_values: None,
        }
    }
}

/// Write the umbrella chart to `chart_dir`, resolve dependencies, and render.
///
/// Returns the rendered YAML manifest stream (Helm's `template` output).
///
/// `chart_dir` is created if missing. Existing contents are overwritten.
pub fn render_umbrella(
    chart: &UmbrellaChart,
    chart_dir: &Path,
    opts: &RenderOptions,
) -> Result<String, RenderError> {
    write_umbrella(chart, chart_dir)?;
    helm_dependency_update(chart_dir, &opts.helm_bin)?;
    helm_template(chart_dir, opts)
}

/// Write `Chart.yaml` and `values.yaml` to `chart_dir`. Does not touch
/// `charts/` — that's Helm's job on `dependency update`.
pub fn write_umbrella(chart: &UmbrellaChart, chart_dir: &Path) -> Result<(), RenderError> {
    std::fs::create_dir_all(chart_dir).map_err(|source| RenderError::Write {
        path: chart_dir.to_path_buf(),
        source,
    })?;

    let chart_yaml =
        serde_yaml::to_string(&chart.chart_yaml).map_err(|source| RenderError::Serialize {
            what: "Chart.yaml",
            source,
        })?;
    let values_yaml =
        serde_yaml::to_string(&chart.values).map_err(|source| RenderError::Serialize {
            what: "values.yaml",
            source,
        })?;

    write(chart_dir.join("Chart.yaml"), chart_yaml.as_bytes())?;
    write(chart_dir.join("values.yaml"), values_yaml.as_bytes())?;
    Ok(())
}

fn write(path: PathBuf, bytes: &[u8]) -> Result<(), RenderError> {
    std::fs::write(&path, bytes).map_err(|source| RenderError::Write { path, source })
}

fn helm_dependency_update(chart_dir: &Path, helm_bin: &Path) -> Result<(), RenderError> {
    let output = Command::new(helm_bin)
        .args(["dependency", "update", "--skip-refresh"])
        .arg(chart_dir)
        .output()
        .map_err(|source| RenderError::Spawn {
            cmd: format!("{} dependency update", helm_bin.display()),
            source,
        })?;
    if !output.status.success() {
        return Err(RenderError::HelmFailed {
            cmd: "helm dependency update".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

fn helm_template(chart_dir: &Path, opts: &RenderOptions) -> Result<String, RenderError> {
    let mut cmd = Command::new(&opts.helm_bin);
    cmd.arg("template")
        .arg(&opts.release_name)
        .arg(chart_dir)
        .args(["--namespace", &opts.namespace]);

    // The overrides file is written outside the chart dir so it never ships
    // with `helm package` or pollutes subsequent renders.
    let overrides_file = if let Some(overrides) = &opts.override_values {
        let yaml = serde_yaml::to_string(overrides).map_err(|source| RenderError::Serialize {
            what: "override values",
            source,
        })?;
        let file = tempfile::Builder::new()
            .prefix("akua-overrides-")
            .suffix(".yaml")
            .tempfile()
            .map_err(|source| RenderError::Write {
                path: PathBuf::from("<tempfile>"),
                source,
            })?;
        std::fs::write(file.path(), yaml).map_err(|source| RenderError::Write {
            path: file.path().to_path_buf(),
            source,
        })?;
        cmd.args(["-f"]).arg(file.path());
        Some(file)
    } else {
        None
    };

    let output = cmd.output().map_err(|source| RenderError::Spawn {
        cmd: format!("{} template", opts.helm_bin.display()),
        source,
    })?;
    drop(overrides_file); // explicit: file stays alive across Helm invocation
    if !output.status.success() {
        return Err(RenderError::HelmFailed {
            cmd: "helm template".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| RenderError::NonUtf8Output {
        cmd: "helm template".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{ChartRef, HelmSource};
    use crate::umbrella::build_umbrella_chart;
    use serde_json::json;

    fn make_chart() -> UmbrellaChart {
        let s = HelmSource {
            id: Some("a".to_string()),
            engine: None,
            chart: ChartRef {
                repo_url: "https://charts.example.com".to_string(),
                chart: Some("redis".to_string()),
                target_revision: "7.0.0".to_string(),
                path: None,
            },
            values: Some(json!({"replicaCount": 2})),
        };
        build_umbrella_chart("demo", "0.1.0", &[s]).expect("known engine")
    }

    #[test]
    fn write_umbrella_emits_expected_files() {
        let tmp = tempfile::tempdir().unwrap();
        let chart = make_chart();
        write_umbrella(&chart, tmp.path()).unwrap();

        let chart_yaml = std::fs::read_to_string(tmp.path().join("Chart.yaml")).unwrap();
        assert!(chart_yaml.contains("apiVersion: v2"));
        assert!(chart_yaml.contains("name: demo"));
        assert!(chart_yaml.contains("alias: redis-"));

        let values_yaml = std::fs::read_to_string(tmp.path().join("values.yaml")).unwrap();
        assert!(values_yaml.contains("replicaCount: 2"));
    }

    #[test]
    fn write_umbrella_creates_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a/b/c");
        let chart = make_chart();
        write_umbrella(&chart, &nested).unwrap();
        assert!(nested.join("Chart.yaml").exists());
    }

    #[test]
    fn missing_helm_binary_surfaces_spawn_error() {
        let tmp = tempfile::tempdir().unwrap();
        let chart = make_chart();
        let opts = RenderOptions {
            helm_bin: PathBuf::from("/nonexistent/helm-binary-akua-test"),
            ..Default::default()
        };
        let err = render_umbrella(&chart, tmp.path(), &opts).unwrap_err();
        match err {
            RenderError::Spawn { .. } => {}
            other => panic!("expected Spawn error, got {other:?}"),
        }
    }
}
