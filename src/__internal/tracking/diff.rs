//! Multi-run diff report.
//!
//! Closes the "no multi-run aggregation / diff" gap from
//! `docs/brainstorming/perf-plan-formalization.md`: comparing a plan
//! run on commit A vs commit B previously meant manually loading
//! both CSVs and eyeballing the differences. The `sim-flow diff`
//! command reads two run-ids from `experiments.db`, loads each run's
//! observability manifest, runs the framework's existing
//! `compare_metric_samples` over the two metric sets, and renders a
//! human-readable markdown table of the differences.
//!
//! The diff is *symmetric*: rows present in only one side surface
//! as `[missing]` in the other column. Numeric values format with
//! their delta (`+12.5%`); histogram and boolean values just show
//! both sides without a computed delta.
//!
//! Intended for CI regression-diff use as well as interactive
//! "what changed between baseline and tuned?" inspection. The
//! output is markdown so it can be piped into a PR comment or
//! design-review doc without further formatting.
//!
//! This module is the loader + renderer; the CLI wrapper that
//! invokes it lives in `commands.rs`.

use std::path::Path;

use crate::tracking::index::ExperimentIndex;
use crate::{Error, Result};

/// One row of the rendered diff. `lhs`/`rhs` carry the value's
/// display string (so the renderer doesn't have to reach back into
/// `StatValue` for formatting); `delta` is `Some(...)` when both
/// sides are scalar AND the relative change is well-defined.
#[derive(Debug, Clone)]
pub struct DiffRow {
    pub path: String,
    pub lhs: String,
    pub rhs: String,
    pub delta: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiffReport {
    pub lhs_run_id: String,
    pub rhs_run_id: String,
    pub lhs_metric_count: usize,
    pub rhs_metric_count: usize,
    pub rows: Vec<DiffRow>,
}

/// Build a diff report between two run-ids by loading each side's
/// observability manifest from `experiments.db` and comparing
/// metric maps.
///
/// Returns `Error::Config` if either run is missing, lacks a
/// `manifest_path`, or its manifest can't be opened.
pub fn run(project_dir: &Path, lhs_run_id: &str, rhs_run_id: &str) -> Result<DiffReport> {
    let dot = project_dir.join(".sim-flow");
    let index = ExperimentIndex::open(&dot)?;
    let lhs_samples = load_run_samples(&index, lhs_run_id)?;
    let rhs_samples = load_run_samples(&index, rhs_run_id)?;
    let report = foundation_framework::compare_metric_samples(&lhs_samples, &rhs_samples);
    let rows = report
        .differences
        .into_iter()
        .map(|entry| {
            let lhs_display = display_value(&entry.left);
            let rhs_display = display_value(&entry.right);
            let delta = numeric_delta(&entry.left, &entry.right);
            DiffRow {
                path: entry.path,
                lhs: lhs_display,
                rhs: rhs_display,
                delta,
            }
        })
        .collect();
    Ok(DiffReport {
        lhs_run_id: lhs_run_id.to_string(),
        rhs_run_id: rhs_run_id.to_string(),
        lhs_metric_count: report.left_metric_count,
        rhs_metric_count: report.right_metric_count,
        rows,
    })
}

/// Render `report` as a GitHub-flavored markdown table. Output is
/// stable: rows are in the order returned by the framework's
/// `compare_metric_samples` (sorted by path).
pub fn render_markdown(report: &DiffReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Diff: {} vs {}\n\n",
        report.lhs_run_id, report.rhs_run_id
    ));
    out.push_str(&format!(
        "- LHS metric count: {}\n- RHS metric count: {}\n- Differences: {}\n\n",
        report.lhs_metric_count,
        report.rhs_metric_count,
        report.rows.len()
    ));
    if report.rows.is_empty() {
        out.push_str("_No differences._\n");
        return out;
    }
    out.push_str("| Metric | LHS | RHS | Δ |\n");
    out.push_str("|---|---|---|---|\n");
    for row in &report.rows {
        let delta = row.delta.as_deref().unwrap_or("");
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            row.path, row.lhs, row.rhs, delta
        ));
    }
    out
}

fn load_run_samples(
    index: &ExperimentIndex,
    run_id: &str,
) -> Result<Vec<foundation_framework::StatSample>> {
    let row = index
        .get_run(run_id)?
        .ok_or_else(|| Error::Config(format!("run-id `{run_id}` not in experiments.db")))?;
    let manifest_rel = row.manifest_path.ok_or_else(|| {
        Error::Config(format!(
            "run-id `{run_id}` has no `manifest_path` recorded; cannot diff metrics"
        ))
    })?;
    let reader = foundation_framework::ObservabilityReader::open(&manifest_rel)
        .map_err(|err| Error::Config(format!("opening manifest for `{run_id}`: {err}")))?;
    let query = foundation_framework::Query {
        kinds: None,
        path_include: vec!["**".to_string()],
        path_exclude: Vec::new(),
        time_start: None,
        time_end: None,
        max_records: None,
    };
    foundation_framework::load_stats_samples(&reader, &query)
        .map_err(|err| Error::Config(format!("loading stats for `{run_id}`: {err}")))
}

fn display_value(value: &Option<foundation_framework::StatValue>) -> String {
    use foundation_framework::StatValue;
    match value {
        None => "_missing_".to_string(),
        Some(StatValue::U64(v)) => v.to_string(),
        Some(StatValue::I64(v)) => v.to_string(),
        Some(StatValue::F64(v)) => format!("{v:.4}"),
        Some(StatValue::Bool(v)) => v.to_string(),
        Some(StatValue::Histogram(hist)) => format!("histogram({} samples)", hist.total_samples),
    }
}

fn numeric_delta(
    lhs: &Option<foundation_framework::StatValue>,
    rhs: &Option<foundation_framework::StatValue>,
) -> Option<String> {
    let lhs_num = as_f64(lhs)?;
    let rhs_num = as_f64(rhs)?;
    let diff = rhs_num - lhs_num;
    if lhs_num == 0.0 {
        return Some(format!("{diff:+.4}"));
    }
    let pct = (diff / lhs_num) * 100.0;
    Some(format!("{diff:+.4} ({pct:+.2}%)"))
}

fn as_f64(value: &Option<foundation_framework::StatValue>) -> Option<f64> {
    use foundation_framework::StatValue;
    match value {
        Some(StatValue::U64(v)) => Some(*v as f64),
        Some(StatValue::I64(v)) => Some(*v as f64),
        Some(StatValue::F64(v)) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_markdown_for_empty_diff() {
        let report = DiffReport {
            lhs_run_id: "a".into(),
            rhs_run_id: "b".into(),
            lhs_metric_count: 3,
            rhs_metric_count: 3,
            rows: Vec::new(),
        };
        let md = render_markdown(&report);
        assert!(md.contains("# Diff: a vs b"));
        assert!(md.contains("_No differences._"));
    }

    #[test]
    fn render_markdown_includes_each_row_and_delta() {
        let report = DiffReport {
            lhs_run_id: "baseline".into(),
            rhs_run_id: "tuned".into(),
            lhs_metric_count: 1,
            rhs_metric_count: 1,
            rows: vec![
                DiffRow {
                    path: "perf.latency.p99".into(),
                    lhs: "50".into(),
                    rhs: "30".into(),
                    delta: Some("-20.0000 (-40.00%)".into()),
                },
                DiffRow {
                    path: "perf.dropped".into(),
                    lhs: "_missing_".into(),
                    rhs: "5".into(),
                    delta: None,
                },
            ],
        };
        let md = render_markdown(&report);
        assert!(md.contains("`perf.latency.p99`"));
        assert!(md.contains("-40.00%"));
        assert!(md.contains("`perf.dropped`"));
    }

    #[test]
    fn numeric_delta_handles_zero_lhs_without_panicking() {
        // Avoid div-by-zero when lhs is 0 -- still report absolute
        // delta but skip the percentage.
        use foundation_framework::StatValue;
        let delta = numeric_delta(&Some(StatValue::U64(0)), &Some(StatValue::U64(5)));
        assert!(delta.is_some(), "delta should still be reported");
        let s = delta.unwrap();
        assert!(s.contains("+5"), "expected +5 in delta: {s}");
        assert!(!s.contains('%'), "no percent for div-by-zero: {s}");
    }

    #[test]
    fn numeric_delta_for_non_scalar_returns_none() {
        use foundation_framework::{HistogramValue, StatValue};
        let hist = StatValue::Histogram(HistogramValue {
            total_samples: 10,
            buckets: Vec::new(),
        });
        assert!(numeric_delta(&Some(hist.clone()), &Some(hist)).is_none());
        assert!(
            numeric_delta(&Some(StatValue::Bool(true)), &Some(StatValue::Bool(false))).is_none()
        );
    }

    #[test]
    fn display_value_formats_each_stat_kind() {
        use foundation_framework::{HistogramValue, StatValue};
        assert_eq!(display_value(&None), "_missing_");
        assert_eq!(display_value(&Some(StatValue::U64(42))), "42");
        assert_eq!(display_value(&Some(StatValue::I64(-3))), "-3");
        assert_eq!(display_value(&Some(StatValue::F64(1.5))), "1.5000");
        assert_eq!(display_value(&Some(StatValue::Bool(true))), "true");
        let hist = HistogramValue {
            total_samples: 7,
            buckets: Vec::new(),
        };
        assert_eq!(
            display_value(&Some(StatValue::Histogram(hist))),
            "histogram(7 samples)"
        );
    }
}
