//! Baseline management: create, list, compare.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::template;
use crate::tracking::index::ExperimentIndex;
use crate::tracking::metrics;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct BaselineRecord {
    pub name: String,
    pub run_id: String,
    pub timestamp: String,
}

/// Metric-by-metric delta between two runs. `None` deltas mean the metric
/// was missing or non-numeric on one side.
#[derive(Debug, Clone, Serialize)]
pub struct BaselineDelta {
    pub baseline_run_id: String,
    pub current_run_id: String,
    pub entries: Vec<DeltaEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeltaEntry {
    pub metric: String,
    pub baseline: Option<f64>,
    pub current: Option<f64>,
    pub delta: Option<f64>,
    pub delta_pct: Option<f64>,
}

pub fn create(
    dot_sim_flow: &Path,
    name: &str,
    run_id: Option<&str>,
    notes: Option<&str>,
) -> Result<BaselineRecord> {
    let index = ExperimentIndex::open(dot_sim_flow)?;
    let resolved_run_id = match run_id {
        Some(id) => id.to_string(),
        None => {
            index
                .list_runs(&crate::tracking::index::RunFilter {
                    limit: Some(1),
                    ..Default::default()
                })?
                .into_iter()
                .next()
                .ok_or_else(|| Error::State("no runs recorded; cannot create baseline".into()))?
                .run_id
        }
    };
    if index.get_run(&resolved_run_id)?.is_none() {
        return Err(Error::State(format!(
            "run {resolved_run_id} not found in experiments index"
        )));
    }
    let timestamp = template::utc_timestamp_now();
    index.insert_baseline(name, &resolved_run_id, &timestamp, notes)?;
    Ok(BaselineRecord {
        name: name.to_string(),
        run_id: resolved_run_id,
        timestamp,
    })
}

pub fn list(dot_sim_flow: &Path) -> Result<Vec<BaselineRecord>> {
    let index = ExperimentIndex::open(dot_sim_flow)?;
    Ok(index
        .list_baselines()?
        .into_iter()
        .map(|(name, run_id, timestamp)| BaselineRecord {
            name,
            run_id,
            timestamp,
        })
        .collect())
}

pub fn compare(
    dot_sim_flow: &Path,
    name: &str,
    current_run_id: Option<&str>,
) -> Result<BaselineDelta> {
    let index = ExperimentIndex::open(dot_sim_flow)?;
    let baseline_run_id = index
        .baseline_run_id(name)?
        .ok_or_else(|| Error::State(format!("baseline {name} not found")))?;
    let current_id = match current_run_id {
        Some(id) => id.to_string(),
        None => {
            index
                .list_runs(&crate::tracking::index::RunFilter {
                    limit: Some(1),
                    ..Default::default()
                })?
                .into_iter()
                .next()
                .ok_or_else(|| Error::State("no runs recorded; cannot compare".into()))?
                .run_id
        }
    };
    if current_id == baseline_run_id {
        return Err(Error::State(format!(
            "current run ({current_id}) is the baseline; nothing to compare"
        )));
    }
    let baseline_summary = parse_summary(&index, &baseline_run_id)?;
    let current_summary = parse_summary(&index, &current_id)?;

    let baseline_view = metrics::numeric_view(&baseline_summary);
    let current_view = metrics::numeric_view(&current_summary);
    let mut keys: Vec<String> = baseline_view
        .keys()
        .chain(current_view.keys())
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();

    let mut entries = Vec::with_capacity(keys.len());
    for key in keys {
        let b = baseline_view.get(&key).and_then(|v| v.as_f64());
        let c = current_view.get(&key).and_then(|v| v.as_f64());
        let delta = match (b, c) {
            (Some(bv), Some(cv)) => Some(cv - bv),
            _ => None,
        };
        let delta_pct = match (b, delta) {
            (Some(bv), Some(d)) if bv != 0.0 => Some((d / bv) * 100.0),
            _ => None,
        };
        entries.push(DeltaEntry {
            metric: key,
            baseline: b,
            current: c,
            delta,
            delta_pct,
        });
    }

    Ok(BaselineDelta {
        baseline_run_id,
        current_run_id: current_id,
        entries,
    })
}

fn parse_summary(index: &ExperimentIndex, run_id: &str) -> Result<Value> {
    let row = index
        .get_run(run_id)?
        .ok_or_else(|| Error::State(format!("run {run_id} not found")))?;
    match row.metrics_summary {
        Some(text) => serde_json::from_str(&text)
            .map_err(|e| Error::State(format!("parsing metrics_summary for {run_id}: {e}"))),
        None => Ok(Value::Object(Default::default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::index::RunRow;
    use tempfile::tempdir;

    fn sample_row(run_id: &str, summary: Option<&str>) -> RunRow {
        RunRow {
            id: 0,
            run_id: run_id.into(),
            timestamp: "t".into(),
            git_commit: "c".into(),
            git_branch: None,
            git_dirty: false,
            config_fingerprint: "fp".into(),
            manifest_path: None,
            workload: None,
            candidate: None,
            study: None,
            metrics_summary: summary.map(|s| s.into()),
            parent_run_id: None,
            sweep_parameter: None,
            sweep_value: None,
            tags: None,
            notes: None,
            lifecycle: "active".into(),
        }
    }

    #[test]
    fn create_and_list_baseline() {
        let tmp = tempdir().unwrap();
        let dot = tmp.path().to_path_buf();
        let index = ExperimentIndex::open(&dot).unwrap();
        index.insert_run(&sample_row("001-a", None)).unwrap();
        let record = create(&dot, "v1", Some("001-a"), None).unwrap();
        assert_eq!(record.run_id, "001-a");
        let list = list(&dot).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn compare_reports_delta() {
        let tmp = tempdir().unwrap();
        let dot = tmp.path().to_path_buf();
        let index = ExperimentIndex::open(&dot).unwrap();
        index
            .insert_run(&sample_row(
                "001-a",
                Some(r#"{"throughput":0.80,"latency_p99":12}"#),
            ))
            .unwrap();
        index
            .insert_run(&sample_row(
                "002-b",
                Some(r#"{"throughput":0.88,"latency_p99":11}"#),
            ))
            .unwrap();
        index.insert_baseline("v1", "001-a", "t", None).unwrap();

        let delta = compare(&dot, "v1", Some("002-b")).unwrap();
        assert_eq!(delta.baseline_run_id, "001-a");
        assert_eq!(delta.current_run_id, "002-b");
        let tp = delta
            .entries
            .iter()
            .find(|e| e.metric == "throughput")
            .unwrap();
        assert!(tp.delta.unwrap() > 0.07 && tp.delta.unwrap() < 0.09);
        let lp = delta
            .entries
            .iter()
            .find(|e| e.metric == "latency_p99")
            .unwrap();
        assert_eq!(lp.delta, Some(-1.0));
    }

    #[test]
    fn compare_with_missing_metric_is_graceful() {
        let tmp = tempdir().unwrap();
        let dot = tmp.path().to_path_buf();
        let index = ExperimentIndex::open(&dot).unwrap();
        index
            .insert_run(&sample_row("001-a", Some(r#"{"throughput":0.80}"#)))
            .unwrap();
        index
            .insert_run(&sample_row("002-b", Some(r#"{"latency_p99":11}"#)))
            .unwrap();
        index.insert_baseline("v1", "001-a", "t", None).unwrap();

        let delta = compare(&dot, "v1", Some("002-b")).unwrap();
        let tp = delta
            .entries
            .iter()
            .find(|e| e.metric == "throughput")
            .unwrap();
        assert_eq!(tp.delta, None);
        let lp = delta
            .entries
            .iter()
            .find(|e| e.metric == "latency_p99")
            .unwrap();
        assert_eq!(lp.delta, None);
    }
}
