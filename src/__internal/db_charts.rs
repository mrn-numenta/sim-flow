//! Terminal-rendered chart catalog over the per-user global DB.
//!
//! v1 is intentionally narrow: one named chart per high-value view,
//! each rendered as a horizontal Unicode-bar histogram in the
//! terminal. Future work will hook into the framework's `ChartFamily`
//! / `ChartPrimitive` / SVG renderer; staying terminal-only for now
//! keeps the dependency surface unchanged and matches the way
//! operators actually consume `sim-flow db` output today (shell
//! pipelines, dashboards reading JSON).
//!
//! Each chart produces a `ChartData` -- a label/value series with
//! optional sub-grouping -- which the binary renders via
//! [`render_terminal_chart`].

use rusqlite::ToSql;

use crate::__internal::db_reports::ReportFilters;
use crate::__internal::global_db::GlobalDb;
use crate::{Error, Result};

/// Catalog of named charts the library knows how to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    BugsByStep,
    BugsByCategory,
    LlmTimeByStep,
    LlmTimeByBackend,
    ToolTimeByTool,
}

impl ChartKind {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::BugsByStep => "bugs-by-step",
            Self::BugsByCategory => "bugs-by-category",
            Self::LlmTimeByStep => "llm-time-by-step",
            Self::LlmTimeByBackend => "llm-time-by-backend",
            Self::ToolTimeByTool => "tool-time-by-tool",
        }
    }

    /// Human title for the rendered chart -- the line above the
    /// first bar.
    pub const fn title(self) -> &'static str {
        match self {
            Self::BugsByStep => "Bug count by step",
            Self::BugsByCategory => "Bug count by category",
            Self::LlmTimeByStep => "LLM wall time by step (ms)",
            Self::LlmTimeByBackend => "LLM wall time by backend / model (ms)",
            Self::ToolTimeByTool => "Tool wall time by tool name (ms)",
        }
    }

    /// Unit suffix shown after each bar's value.
    pub const fn unit(self) -> &'static str {
        match self {
            Self::BugsByStep | Self::BugsByCategory => "bugs",
            Self::LlmTimeByStep | Self::LlmTimeByBackend | Self::ToolTimeByTool => "ms",
        }
    }
}

/// One bar in the rendered chart.
#[derive(Debug, Clone)]
pub struct ChartRow {
    pub label: String,
    pub value: f64,
}

/// A whole chart's data, ready to render.
#[derive(Debug, Clone)]
pub struct ChartData {
    pub title: &'static str,
    pub unit: &'static str,
    pub rows: Vec<ChartRow>,
}

/// Build the data series for a named chart against `db`. Filters
/// behave the same as the report catalog -- shared with
/// [`crate::__internal::db_reports`].
pub fn build_chart(
    db: &mut GlobalDb,
    kind: ChartKind,
    filters: &ReportFilters,
) -> Result<ChartData> {
    let (sql, args) = match kind {
        ChartKind::BugsByStep => bugs_by_step_sql(filters),
        ChartKind::BugsByCategory => bugs_by_category_sql(filters),
        ChartKind::LlmTimeByStep => llm_time_by_step_sql(filters),
        ChartKind::LlmTimeByBackend => llm_time_by_backend_sql(filters),
        ChartKind::ToolTimeByTool => tool_time_by_tool_sql(filters),
    };
    let rows = run_label_value(db, &sql, args, filters.limit.unwrap_or(20))?;
    Ok(ChartData {
        title: kind.title(),
        unit: kind.unit(),
        rows,
    })
}

// ─── SQL builders ─────────────────────────────────────────────────────────

fn bugs_by_step_sql(filters: &ReportFilters) -> (String, Vec<String>) {
    let mut sql = String::from("SELECT step AS label, COUNT(*) AS value FROM bugs WHERE 1 = 1");
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step ORDER BY value DESC");
    (sql, args)
}

fn bugs_by_category_sql(filters: &ReportFilters) -> (String, Vec<String>) {
    let mut sql = String::from("SELECT category AS label, COUNT(*) AS value FROM bugs WHERE 1 = 1");
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY category ORDER BY value DESC");
    (sql, args)
}

fn llm_time_by_step_sql(filters: &ReportFilters) -> (String, Vec<String>) {
    let mut sql =
        String::from("SELECT step AS label, SUM(wall_ms) AS value FROM llm_metrics WHERE 1 = 1");
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step ORDER BY value DESC");
    (sql, args)
}

fn llm_time_by_backend_sql(filters: &ReportFilters) -> (String, Vec<String>) {
    let mut sql = String::from(
        "SELECT (backend || '/' || COALESCE(model,'?')) AS label, \
                SUM(wall_ms) AS value \
         FROM llm_metrics WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY backend, model ORDER BY value DESC");
    (sql, args)
}

fn tool_time_by_tool_sql(filters: &ReportFilters) -> (String, Vec<String>) {
    let mut sql = String::from(
        "SELECT (tool_name || ' [' || caller_kind || ']') AS label, \
                SUM(wall_ms) AS value \
         FROM tool_timings WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY tool_name, caller_kind ORDER BY value DESC");
    (sql, args)
}

// ─── Helpers ──────────────────────────────────────────────────────────────

fn push_project_filter(sql: &mut String, args: &mut Vec<String>, filters: &ReportFilters) {
    if let Some(p) = filters.project.as_deref() {
        sql.push_str(" AND project_dir LIKE ?");
        args.push(format!("%{p}%"));
    }
}

fn push_step_filter(sql: &mut String, args: &mut Vec<String>, filters: &ReportFilters) {
    if let Some(s) = filters.step.as_deref() {
        sql.push_str(" AND step = ?");
        args.push(s.to_string());
    }
}

/// Run a `(label, value)` query and collect a bounded number of rows
/// for the bar chart. `value` is read as f64 so SUMs that overflow
/// i64 stay representable; integer counts come back exact since they
/// fit in f64 mantissa for any practical sim-flow corpus size.
fn run_label_value(
    db: &mut GlobalDb,
    sql: &str,
    args: Vec<String>,
    limit: usize,
) -> Result<Vec<ChartRow>> {
    db.conn_mut()
        .pragma_update(None, "query_only", "ON")
        .map_err(wrap)?;
    let result = (|| -> Result<Vec<ChartRow>> {
        let conn = db.conn();
        let mut stmt = conn.prepare(sql).map_err(wrap)?;
        let params_refs: Vec<&dyn ToSql> = args.iter().map(|s| s as &dyn ToSql).collect();
        let mut rows_iter = stmt
            .query(rusqlite::params_from_iter(params_refs.iter()))
            .map_err(wrap)?;
        let mut out: Vec<ChartRow> = Vec::new();
        while let Some(row) = rows_iter.next().map_err(wrap)? {
            if out.len() >= limit {
                break;
            }
            let label: Option<String> = row.get(0).map_err(wrap)?;
            let value: f64 = row.get::<_, f64>(1).map_err(wrap)?;
            out.push(ChartRow {
                label: label.unwrap_or_else(|| "(unknown)".to_string()),
                value,
            });
        }
        Ok(out)
    })();
    let _ = db.conn_mut().pragma_update(None, "query_only", "OFF");
    result
}

fn wrap(source: rusqlite::Error) -> Error {
    Error::State(format!("db_charts sqlite error: {source}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::bug_log::BugRecord;
    use crate::__internal::session::llm_metrics::LlmMetricsRecord;
    use crate::__internal::session::protocol::SessionKindOut;

    fn seed_db() -> GlobalDb {
        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        for (id, step, category) in [
            ("bug-001", "DM0", "compile_error"),
            ("bug-002", "DM3c", "test_failure"),
            ("bug-003", "DM3c", "test_failure"),
        ] {
            db.record_bug(
                &project_dir,
                &BugRecord {
                    id: id.into(),
                    opened_at: "1700000000".into(),
                    closed_at: None,
                    step: step.into(),
                    milestone: None,
                    category: category.into(),
                    issue: "x".into(),
                    events: Vec::new(),
                    resolution: None,
                    status: "open".into(),
                },
            )
            .expect("record_bug");
        }
        for (req, step, wall) in [
            ("req-1", "DM0", 1000_u64),
            ("req-2", "DM0", 500),
            ("req-3", "DM3c", 4000),
        ] {
            db.record_llm_metric(
                &project_dir,
                &LlmMetricsRecord::from_byte_estimate(
                    1700000000,
                    step,
                    SessionKindOut::Work,
                    "vllm",
                    Some("qwen3.6"),
                    req,
                    1,
                    wall,
                    Some("stop"),
                    100,
                    50,
                ),
            )
            .expect("record_llm_metric");
        }
        db
    }

    #[test]
    fn bugs_by_step_chart_orders_by_count_desc() {
        let mut db = seed_db();
        let data = build_chart(&mut db, ChartKind::BugsByStep, &ReportFilters::default())
            .expect("build_chart");
        assert_eq!(data.title, ChartKind::BugsByStep.title());
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.rows[0].label, "DM3c");
        assert_eq!(data.rows[0].value, 2.0);
        assert_eq!(data.rows[1].label, "DM0");
        assert_eq!(data.rows[1].value, 1.0);
    }

    #[test]
    fn llm_time_by_step_chart_uses_sum_wall_ms() {
        let mut db = seed_db();
        let data = build_chart(&mut db, ChartKind::LlmTimeByStep, &ReportFilters::default())
            .expect("build_chart");
        assert_eq!(data.unit, "ms");
        // DM3c has the larger total (4000) -> first bar.
        assert_eq!(data.rows[0].label, "DM3c");
        assert_eq!(data.rows[0].value, 4000.0);
        assert_eq!(data.rows[1].label, "DM0");
        assert_eq!(data.rows[1].value, 1500.0);
    }

    #[test]
    fn limit_filter_caps_row_count() {
        let mut db = seed_db();
        let filters = ReportFilters {
            project: None,
            step: None,
            limit: Some(1),
        };
        let data = build_chart(&mut db, ChartKind::BugsByStep, &filters).expect("build_chart");
        assert_eq!(data.rows.len(), 1);
    }
}
