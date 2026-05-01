//! Parameter sweep coordination.
//!
//! Reads a `sweep.toml` definition, runs the model binary once per
//! configured value, and records every variant as a child run with a
//! shared `parent_run_id`. The sweep driver does NOT apply config
//! overlays to the model itself -- it invokes the model binary with
//! `--run-id <variant>` and `--<parameter> <value>` and lets the model
//! interpret the parameter. Foundation's `ConfigManager::set_config_key`
//! integration is a Phase 4 follow-up tracked in the plan.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::tracking::index::{ExperimentIndex, RunRow};
use crate::tracking::run_recording::{RecordRunOptions, record_run};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepDefinition {
    pub sweep: SweepSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepSection {
    pub name: String,
    pub parameter: String,
    pub values: Vec<toml::Value>,
    pub workload: String,
    /// Path to the model binary (default: `./target/debug/model`). If the
    /// binary is absent the sweep still records runs but skips invocation,
    /// which lets tests exercise the bookkeeping without compiling a
    /// model.
    #[serde(default)]
    pub binary: Option<String>,
    /// Extra CLI arguments passed to every invocation.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SweepResults {
    pub parent_run_id: String,
    pub child_run_ids: Vec<String>,
}

/// Load a sweep definition from `path`.
pub fn load(path: &Path) -> Result<SweepDefinition> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::TomlParse {
        path: path.to_path_buf(),
        source,
    })
}

/// Execute the sweep: record a parent run, iterate values, invoke the
/// model per variant, record each variant as a child run.
pub fn run(
    project_dir: &Path,
    dot_sim_flow: &Path,
    definition: &SweepDefinition,
) -> Result<SweepResults> {
    let parent = record_run(
        project_dir,
        dot_sim_flow,
        &RecordRunOptions {
            description: format!("sweep-{}", definition.sweep.name),
            workload: Some(definition.sweep.workload.clone()),
            sweep_parameter: Some(definition.sweep.parameter.clone()),
            tags: vec!["sweep-parent".to_string()],
            ..Default::default()
        },
    )?;

    let binary_path = resolve_binary(project_dir, definition.sweep.binary.as_deref());

    let mut child_ids = Vec::with_capacity(definition.sweep.values.len());
    for value in &definition.sweep.values {
        let value_str = value_to_string(value);
        let child = record_run(
            project_dir,
            dot_sim_flow,
            &RecordRunOptions {
                description: format!(
                    "sweep-{}-{}",
                    definition.sweep.name,
                    value_str.trim_matches('"')
                ),
                workload: Some(definition.sweep.workload.clone()),
                parent_run_id: Some(parent.run_id.clone()),
                sweep_parameter: Some(definition.sweep.parameter.clone()),
                sweep_value: Some(value_str.clone()),
                tags: vec!["sweep-child".to_string()],
                ..Default::default()
            },
        )?;
        invoke_model_binary(&binary_path, &child.run_id, &definition.sweep, &value_str);
        child_ids.push(child.run_id);
    }

    Ok(SweepResults {
        parent_run_id: parent.run_id,
        child_run_ids: child_ids,
    })
}

/// List the child runs of a sweep parent.
pub fn results(dot_sim_flow: &Path, parent_run_id: &str) -> Result<Vec<RunRow>> {
    let index = ExperimentIndex::open(dot_sim_flow)?;
    index.list_runs(&crate::tracking::index::RunFilter {
        parent_run_id: Some(parent_run_id.to_string()),
        ..Default::default()
    })
}

fn resolve_binary(project_dir: &Path, configured: Option<&str>) -> PathBuf {
    if let Some(p) = configured {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            return path;
        }
        return project_dir.join(path);
    }
    project_dir.join("target/debug").join("model")
}

fn invoke_model_binary(binary: &Path, run_id: &str, sweep: &SweepSection, value: &str) {
    if !binary.exists() {
        // Bookkeeping-only sweep: the binary is absent (e.g. tests, or
        // the user has not yet built the model). Leave the run row in
        // place but do not attempt to invoke anything.
        return;
    }
    let mut cmd = Command::new(binary);
    cmd.arg("--run-id").arg(run_id);
    let mut arg_name = String::from("--");
    for ch in sweep.parameter.chars() {
        if ch == '.' || ch == '_' {
            arg_name.push('-');
        } else {
            arg_name.push(ch);
        }
    }
    cmd.arg(&arg_name).arg(value.trim_matches('"'));
    for extra in &sweep.extra_args {
        cmd.arg(extra);
    }
    let _ = cmd.status();
}

fn value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_parses_sweep_toml() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("sweep.toml");
        std::fs::write(
            &path,
            r#"
[sweep]
name = "buffer-depth"
parameter = "noc.router.buffer_depth"
values = [4, 8, 16]
workload = "throughput-stress"
"#,
        )
        .unwrap();
        let def = load(&path).unwrap();
        assert_eq!(def.sweep.values.len(), 3);
        assert_eq!(def.sweep.parameter, "noc.router.buffer_depth");
    }

    #[test]
    fn run_records_parent_and_children() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        let dot = project.join(".sim-flow");
        std::fs::create_dir_all(&dot).unwrap();

        let def = SweepDefinition {
            sweep: SweepSection {
                name: "buffer-depth".into(),
                parameter: "buffer_depth".into(),
                values: vec![toml::Value::Integer(4), toml::Value::Integer(8)],
                workload: "tp".into(),
                binary: Some("./definitely-not-present".into()),
                extra_args: vec![],
            },
        };
        let out = run(&project, &dot, &def).unwrap();
        assert_eq!(out.child_run_ids.len(), 2);
        let index = ExperimentIndex::open(&dot).unwrap();
        let children = results(&dot, &out.parent_run_id).unwrap();
        assert_eq!(children.len(), 2);
        for child in &children {
            assert_eq!(child.sweep_parameter.as_deref(), Some("buffer_depth"));
            assert_eq!(
                child.parent_run_id.as_deref(),
                Some(out.parent_run_id.as_str())
            );
        }
        // Parent row is tagged as a sweep parent.
        assert_eq!(index.count_runs().unwrap(), 3);
    }
}
