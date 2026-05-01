//! Record a simulation run into the experiments index.
//!
//! Phase 4 records runs that are initiated by the orchestrator (e.g. via
//! `sim-flow sweep`) and exposes a `record-run` CLI for cases where the
//! user invokes a simulation outside `sim-flow run`. Each recorded run
//! gets an `.experiments/<run-id>/` directory with `config.json` and a
//! placeholder `metrics.json`. `.obsv` artifacts stay where the model
//! wrote them; `manifest_path` in the index points to them.

use std::path::{Path, PathBuf};

use crate::template;
use crate::tracking::git_state::GitState;
use crate::tracking::index::{ExperimentIndex, RunRow};
use crate::{Error, Result};

pub const EXPERIMENTS_DIR: &str = ".experiments";

#[derive(Debug, Clone, Default)]
pub struct RecordRunOptions {
    pub description: String,
    pub workload: Option<String>,
    pub candidate: Option<String>,
    pub study: Option<String>,
    pub manifest_path: Option<PathBuf>,
    pub notes: Option<String>,
    pub parent_run_id: Option<String>,
    pub sweep_parameter: Option<String>,
    pub sweep_value: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RecordedRun {
    pub run_id: String,
    pub sequence: u32,
    pub artifact_dir: PathBuf,
    pub git: GitState,
}

/// Record a new run in the experiments index and materialize its
/// per-run artifact directory. The caller supplies the free-form
/// description (e.g. workload name) used to build the run_id suffix.
pub fn record_run(
    project_dir: &Path,
    dot_sim_flow: &Path,
    options: &RecordRunOptions,
) -> Result<RecordedRun> {
    let index = ExperimentIndex::open(dot_sim_flow)?;
    let sequence = index.next_sequence()?;
    let description_slug = slugify(&options.description);
    let run_id = format!("{sequence:03}-{description_slug}");

    let git = GitState::capture(project_dir);
    let timestamp = template::utc_timestamp_now();

    let config_fingerprint = fingerprint_config(dot_sim_flow)?;
    let artifact_dir = project_dir.join(EXPERIMENTS_DIR).join(&run_id);
    std::fs::create_dir_all(&artifact_dir).map_err(|source| Error::Io {
        path: artifact_dir.clone(),
        source,
    })?;

    write_config_snapshot(dot_sim_flow, &artifact_dir)?;
    write_placeholder_metrics(&artifact_dir)?;

    let manifest_path_string = options
        .manifest_path
        .as_ref()
        .map(|p| p.display().to_string());

    let tags = if options.tags.is_empty() {
        None
    } else {
        Some(options.tags.join(","))
    };

    let row = RunRow {
        id: 0,
        run_id: run_id.clone(),
        timestamp,
        git_commit: git.commit.clone(),
        git_branch: git.branch.clone(),
        git_dirty: git.dirty,
        config_fingerprint,
        manifest_path: manifest_path_string,
        workload: options.workload.clone(),
        candidate: options.candidate.clone(),
        study: options.study.clone(),
        metrics_summary: None,
        parent_run_id: options.parent_run_id.clone(),
        sweep_parameter: options.sweep_parameter.clone(),
        sweep_value: options.sweep_value.clone(),
        tags,
        notes: options.notes.clone(),
        lifecycle: "active".to_string(),
    };
    index.insert_run(&row)?;

    Ok(RecordedRun {
        run_id,
        sequence,
        artifact_dir,
        git,
    })
}

fn slugify(description: &str) -> String {
    let mut out = String::with_capacity(description.len());
    for ch in description.chars() {
        let mapped = match ch {
            'a'..='z' | '0'..='9' | '-' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            '_' | ' ' | '.' | '/' => '-',
            _ => continue,
        };
        out.push(mapped);
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "run".to_string()
    } else {
        out
    }
}

/// Compute a cheap stable fingerprint of the orchestrator config file.
/// Full Foundation `ConfigManager` fingerprints live alongside the model's
/// `.obsv` manifest; this fingerprint captures the orchestrator-visible
/// state so two runs with identical sim-flow config produce identical
/// fingerprints.
fn fingerprint_config(dot_sim_flow: &Path) -> Result<String> {
    let config_path = dot_sim_flow.join(crate::config::CONFIG_FILE);
    if !config_path.exists() {
        return Ok("none".to_string());
    }
    let text = std::fs::read_to_string(&config_path).map_err(|source| Error::Io {
        path: config_path.clone(),
        source,
    })?;
    Ok(fnv1a_hex(text.as_bytes()))
}

fn write_config_snapshot(dot_sim_flow: &Path, artifact_dir: &Path) -> Result<()> {
    let src = dot_sim_flow.join(crate::config::CONFIG_FILE);
    let dst = artifact_dir.join("config.toml");
    if src.exists() {
        std::fs::copy(&src, &dst).map_err(|source| Error::Io { path: dst, source })?;
    } else {
        std::fs::write(&dst, b"# no sim-flow config.toml present at record time\n")
            .map_err(|source| Error::Io { path: dst, source })?;
    }
    Ok(())
}

fn write_placeholder_metrics(artifact_dir: &Path) -> Result<()> {
    let path = artifact_dir.join("metrics.json");
    let body = br#"{"version":1,"metrics":{}}"#;
    std::fs::write(&path, body).map_err(|source| Error::Io { path, source })?;
    Ok(())
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slugify_is_reasonable() {
        assert_eq!(slugify("Throughput Stress"), "throughput-stress");
        assert_eq!(slugify("/bad/path/"), "bad-path");
        assert_eq!(slugify(""), "run");
    }

    #[test]
    fn records_a_run_with_artifacts() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        let dot = project.join(".sim-flow");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join("config.toml"), "[client]\nname=\"mock\"\n").unwrap();

        let run = record_run(
            &project,
            &dot,
            &RecordRunOptions {
                description: "throughput stress".into(),
                workload: Some("throughput".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.run_id.starts_with("001-throughput"));
        assert!(run.artifact_dir.exists());
        assert!(run.artifact_dir.join("config.toml").exists());
        assert!(run.artifact_dir.join("metrics.json").exists());

        let index = ExperimentIndex::open(&dot).unwrap();
        assert_eq!(index.count_runs().unwrap(), 1);
        let got = index.get_run(&run.run_id).unwrap().unwrap();
        assert_eq!(got.workload.as_deref(), Some("throughput"));
        assert_ne!(got.config_fingerprint, "none");
    }

    #[test]
    fn fingerprint_is_none_when_config_missing() {
        let tmp = tempdir().unwrap();
        let dot = tmp.path().to_path_buf();
        std::fs::create_dir_all(&dot).unwrap();
        assert_eq!(fingerprint_config(&dot).unwrap(), "none");
    }
}
