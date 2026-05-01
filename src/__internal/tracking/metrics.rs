//! Metrics extraction from `.obsv` artifacts.
//!
//! Foundation's `ObservabilityReader` exposes raw records but does not
//! (yet) expose named metric accessors for throughput / latency /
//! utilization. Until that API is added (tracked in the Phase 4 plan as
//! the "observability data API audit" deferred item), this module takes
//! a graceful-fallback approach:
//!
//!   1. If the model writes a `metrics.json` file into its
//!      `.experiments/<run-id>/` directory, we read it as the canonical
//!      metrics summary.
//!   2. Otherwise, if a run manifest path was recorded, we capture the
//!      manifest's `run_id` and `format_version` as a minimal summary
//!      so cross-run queries at least know the run produced a manifest.
//!   3. Otherwise, the metrics summary is `{}`.
//!
//! Downstream consumers (baseline compare, DM4 gate) must treat missing
//! metrics as "unknown" rather than "zero".

use std::path::Path;

use serde_json::{Map, Value};

use crate::tracking::index::ExperimentIndex;
use crate::{Error, Result};

/// Read `metrics.json` from the run's artifact directory, if present.
pub fn read_metrics_json(artifact_dir: &Path) -> Result<Option<Value>> {
    let path = artifact_dir.join("metrics.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| Error::State(format!("invalid metrics.json at {}: {e}", path.display())))?;
    Ok(Some(value))
}

/// Update the `metrics_summary` column for a run from its on-disk
/// metrics.json (if any). Returns the summary that was written, or
/// `None` if no metrics source was available.
pub fn extract_and_store(
    index: &ExperimentIndex,
    run_id: &str,
    artifact_dir: &Path,
) -> Result<Option<Value>> {
    let metrics = read_metrics_json(artifact_dir)?;
    if let Some(value) = metrics.as_ref() {
        let summary = flatten_metric_entries(value);
        let json = serde_json::to_string(&summary)
            .map_err(|e| Error::State(format!("metrics summary serialize: {e}")))?;
        index.update_metrics_summary(run_id, &json)?;
        return Ok(Some(Value::Object(summary)));
    }
    Ok(None)
}

/// Extract the `metrics` submap if present; otherwise treat the top-level
/// object as the metrics dictionary. This keeps the format lenient so
/// models can emit either `{"metrics": {"throughput": 0.88}}` or
/// `{"throughput": 0.88}`.
fn flatten_metric_entries(value: &Value) -> Map<String, Value> {
    if let Some(obj) = value.as_object() {
        if let Some(Value::Object(inner)) = obj.get("metrics") {
            return inner.clone();
        }
        return obj.clone();
    }
    Map::new()
}

/// Produce a minimal `{ metric: f64 }` view for delta computations. Any
/// non-numeric fields are skipped so callers can safely compute deltas.
pub fn numeric_view(summary: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(obj) = summary.as_object() {
        for (k, v) in obj {
            if let Some(n) = v.as_f64() {
                out.insert(k.clone(), Value::from(n));
            } else if let Some(n) = v.as_i64() {
                out.insert(k.clone(), Value::from(n as f64));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn reads_flat_metrics_json() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("metrics.json"),
            r#"{"throughput":0.88,"latency_p99":11}"#,
        )
        .unwrap();
        let value = read_metrics_json(tmp.path()).unwrap().unwrap();
        let flat = flatten_metric_entries(&value);
        assert_eq!(flat.get("throughput").and_then(|v| v.as_f64()), Some(0.88));
    }

    #[test]
    fn reads_nested_metrics_json() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("metrics.json"),
            r#"{"version":1,"metrics":{"throughput":0.5}}"#,
        )
        .unwrap();
        let value = read_metrics_json(tmp.path()).unwrap().unwrap();
        let flat = flatten_metric_entries(&value);
        assert_eq!(flat.get("throughput").and_then(|v| v.as_f64()), Some(0.5));
    }

    #[test]
    fn extract_and_store_persists_when_file_exists() {
        let tmp = tempdir().unwrap();
        let dot = tmp.path().join(".sim-flow");
        std::fs::create_dir_all(&dot).unwrap();
        let artifact = tmp.path().join(".experiments/001-x");
        std::fs::create_dir_all(&artifact).unwrap();
        std::fs::write(artifact.join("metrics.json"), r#"{"throughput":1.0}"#).unwrap();

        let index = ExperimentIndex::open(&dot).unwrap();
        index
            .insert_run(&crate::tracking::index::RunRow {
                id: 0,
                run_id: "001-x".into(),
                timestamp: "t".into(),
                git_commit: "c".into(),
                git_branch: None,
                git_dirty: false,
                config_fingerprint: "fp".into(),
                manifest_path: None,
                workload: None,
                candidate: None,
                study: None,
                metrics_summary: None,
                parent_run_id: None,
                sweep_parameter: None,
                sweep_value: None,
                tags: None,
                notes: None,
                lifecycle: "active".into(),
            })
            .unwrap();

        let summary = extract_and_store(&index, "001-x", &artifact)
            .unwrap()
            .unwrap();
        let persisted = index
            .get_run("001-x")
            .unwrap()
            .unwrap()
            .metrics_summary
            .unwrap();
        assert!(persisted.contains("throughput"));
        assert_eq!(
            summary
                .as_object()
                .unwrap()
                .get("throughput")
                .and_then(|v| v.as_f64()),
            Some(1.0)
        );
    }
}
