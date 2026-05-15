//! Named-report catalog over the per-user global DB.
//!
//! Each report is a parameterized SQL query that produces
//! `(columns, rows)` -- the same shape as
//! [`GlobalDb::query_read_only`](crate::__internal::global_db::GlobalDb::query_read_only)
//! -- so the CLI can render any report through the existing text-table
//! / JSON pipeline.
//!
//! The catalog stays in one file by design: adding a report is one
//! function here plus one new variant on
//! [`crate::cli::DbReportKind`]. When the catalog grows past
//! ~20 reports or starts wanting to share SQL fragments we'll factor
//! into a `db_reports/` module; for v1 a flat file keeps everything
//! visible.
//!
//! Common filters (`--project` substring, `--step`, `--limit`) are
//! handled inline per report -- the SQL builders accept the same
//! [`ReportFilters`] struct so the CLI doesn't need to know which
//! report supports which filter.

use rusqlite::ToSql;
use serde_json::Value;

use crate::__internal::global_db::GlobalDb;
use crate::{Error, Result};

/// Catalog of named reports the library knows how to run. The binary
/// CLI defines a `clap::ValueEnum` wrapper that converts to this so
/// the library doesn't depend on clap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    BugsByStep,
    BugsByCategory,
    BugsRecent,
    BugsOpen,
    LlmTimeByStep,
    LlmTimeByBackend,
    LlmTimeByKind,
    ToolTimeByTool,
    ToolTimeByStep,
    GateTimeByStep,
    ExperimentsRecent,
}

impl ReportKind {
    /// Kebab-case slug used in CLI args and JSON output. Stable wire
    /// shape -- downstream readers depend on these names.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::BugsByStep => "bugs-by-step",
            Self::BugsByCategory => "bugs-by-category",
            Self::BugsRecent => "bugs-recent",
            Self::BugsOpen => "bugs-open",
            Self::LlmTimeByStep => "llm-time-by-step",
            Self::LlmTimeByBackend => "llm-time-by-backend",
            Self::LlmTimeByKind => "llm-time-by-kind",
            Self::ToolTimeByTool => "tool-time-by-tool",
            Self::ToolTimeByStep => "tool-time-by-step",
            Self::GateTimeByStep => "gate-time-by-step",
            Self::ExperimentsRecent => "experiments-recent",
        }
    }
}

/// Filters shared across reports. Each report applies whichever
/// filters make sense for its underlying table; unsupported filters
/// are silently ignored (intentional -- the CLI shouldn't need a
/// report-by-report table of which filters apply).
#[derive(Debug, Clone, Default)]
pub struct ReportFilters {
    /// Substring match against the row's `project_dir` column.
    pub project: Option<String>,
    /// Exact-match against the row's `step` column.
    pub step: Option<String>,
    /// Per-report row cap. `None` falls back to the report's own
    /// sensible default.
    pub limit: Option<usize>,
}

/// Run a named report against `db`, returning the result columns and
/// rows. Each cell is a `serde_json::Value` -- same shape as
/// `query_read_only` -- so the CLI can hand the result straight to
/// `render_text_table` or `serde_json::to_string_pretty`.
pub fn run_report(
    db: &mut GlobalDb,
    kind: ReportKind,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    match kind {
        ReportKind::BugsByStep => bugs_by_step(db, filters),
        ReportKind::BugsByCategory => bugs_by_category(db, filters),
        ReportKind::BugsRecent => bugs_recent(db, filters),
        ReportKind::BugsOpen => bugs_open(db, filters),
        ReportKind::LlmTimeByStep => llm_time_by_step(db, filters),
        ReportKind::LlmTimeByBackend => llm_time_by_backend(db, filters),
        ReportKind::LlmTimeByKind => llm_time_by_kind(db, filters),
        ReportKind::ToolTimeByTool => tool_time_by_tool(db, filters),
        ReportKind::ToolTimeByStep => tool_time_by_step(db, filters),
        ReportKind::GateTimeByStep => gate_time_by_step(db, filters),
        ReportKind::ExperimentsRecent => experiments_recent(db, filters),
    }
}

// ─── Report builders ──────────────────────────────────────────────────────

fn bugs_by_step(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT step, category, count(*) AS count \
         FROM bugs WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step, category ORDER BY step, count DESC");
    run_typed(db, &sql, args)
}

fn bugs_by_category(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT category, count(*) AS count, MAX(opened_at) AS most_recent \
         FROM bugs WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY category ORDER BY count DESC");
    run_typed(db, &sql, args)
}

fn bugs_recent(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let limit = filters.limit.unwrap_or(20);
    let mut sql = String::from(
        "SELECT bug_id, step, category, status, opened_at, project_dir \
         FROM bugs WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(&format!(" ORDER BY opened_at DESC LIMIT {limit}"));
    run_typed(db, &sql, args)
}

fn bugs_open(db: &mut GlobalDb, filters: &ReportFilters) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT bug_id, step, category, status, opened_at, project_dir \
         FROM bugs WHERE status IN ('open','manual')",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" ORDER BY opened_at DESC");
    run_typed(db, &sql, args)
}

fn llm_time_by_step(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT step, SUM(wall_ms) AS total_ms, COUNT(*) AS turns, \
                SUM(tokens_in) AS tokens_in, SUM(tokens_out) AS tokens_out \
         FROM llm_metrics WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step ORDER BY total_ms DESC");
    run_typed(db, &sql, args)
}

fn llm_time_by_backend(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT backend, model, SUM(wall_ms) AS total_ms, COUNT(*) AS turns, \
                SUM(tokens_in) AS tokens_in, SUM(tokens_out) AS tokens_out \
         FROM llm_metrics WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY backend, model ORDER BY total_ms DESC");
    run_typed(db, &sql, args)
}

fn llm_time_by_kind(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT step, kind, SUM(wall_ms) AS total_ms, COUNT(*) AS turns \
         FROM llm_metrics WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step, kind ORDER BY step, total_ms DESC");
    run_typed(db, &sql, args)
}

fn tool_time_by_tool(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT tool_name, caller_kind, SUM(wall_ms) AS total_ms, COUNT(*) AS invocations \
         FROM tool_timings WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY tool_name, caller_kind ORDER BY total_ms DESC");
    run_typed(db, &sql, args)
}

fn tool_time_by_step(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT step, caller_kind, SUM(wall_ms) AS total_ms, COUNT(*) AS invocations \
         FROM tool_timings WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step, caller_kind ORDER BY step, total_ms DESC");
    run_typed(db, &sql, args)
}

fn gate_time_by_step(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut sql = String::from(
        "SELECT step, tool_name, SUM(wall_ms) AS total_ms, COUNT(*) AS invocations \
         FROM tool_timings WHERE caller_kind = 'gate'",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    push_step_filter(&mut sql, &mut args, filters);
    sql.push_str(" GROUP BY step, tool_name ORDER BY total_ms DESC");
    run_typed(db, &sql, args)
}

fn experiments_recent(
    db: &mut GlobalDb,
    filters: &ReportFilters,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let limit = filters.limit.unwrap_or(20);
    let mut sql = String::from(
        "SELECT run_id, timestamp, workload, candidate, study, lifecycle, project_dir \
         FROM experiment_runs WHERE 1 = 1",
    );
    let mut args: Vec<String> = Vec::new();
    push_project_filter(&mut sql, &mut args, filters);
    sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT {limit}"));
    run_typed(db, &sql, args)
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

/// Run a parameterized SELECT and map the columns + rows to the same
/// `Vec<Vec<Value>>` shape `query_read_only` returns. Reports use
/// positional placeholders (`?`) so we don't need to round-trip the
/// arg names through the catalog.
fn run_typed(
    db: &mut GlobalDb,
    sql: &str,
    args: Vec<String>,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    use rusqlite::types::ValueRef;
    db.conn_mut()
        .pragma_update(None, "query_only", "ON")
        .map_err(wrap)?;
    let result = (|| -> Result<(Vec<String>, Vec<Vec<Value>>)> {
        let conn = db.conn();
        let mut stmt = conn.prepare(sql).map_err(wrap)?;
        let columns: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let column_count = columns.len();
        let params_refs: Vec<&dyn ToSql> = args.iter().map(|s| s as &dyn ToSql).collect();
        let mut rows_iter = stmt
            .query(rusqlite::params_from_iter(params_refs.iter()))
            .map_err(wrap)?;
        let mut out_rows: Vec<Vec<Value>> = Vec::new();
        while let Some(row) = rows_iter.next().map_err(wrap)? {
            let mut cells: Vec<Value> = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value_ref = row.get_ref(i).map_err(wrap)?;
                let json = match value_ref {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::Number(n.into()),
                    ValueRef::Real(f) => serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .unwrap_or(Value::Null),
                    ValueRef::Text(bytes) => {
                        Value::String(String::from_utf8_lossy(bytes).into_owned())
                    }
                    ValueRef::Blob(_) => Value::String("(blob)".to_string()),
                };
                cells.push(json);
            }
            out_rows.push(cells);
        }
        Ok((columns, out_rows))
    })();
    let _ = db.conn_mut().pragma_update(None, "query_only", "OFF");
    result
}

fn wrap(source: rusqlite::Error) -> Error {
    Error::State(format!("db_reports sqlite error: {source}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::bug_log::BugRecord;
    use crate::__internal::session::llm_metrics::LlmMetricsRecord;
    use crate::__internal::session::protocol::SessionKindOut;
    use crate::__internal::session::tool_timings::{CallerKind, ToolTimingRecord};

    fn seed_db() -> GlobalDb {
        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        for (id, step, category, status) in [
            ("bug-001", "DM0", "compile_error", "resolved"),
            ("bug-002", "DM3c", "test_failure", "resolved"),
            ("bug-003", "DM3c", "test_failure", "open"),
            ("bug-004", "DM4a", "performance", "manual"),
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
                    status: status.into(),
                },
            )
            .expect("record_bug");
        }
        for (request_id, step, kind, wall) in [
            ("req-1", "DM0", SessionKindOut::Work, 1000_u64),
            ("req-2", "DM0", SessionKindOut::Critique, 500),
            ("req-3", "DM3c", SessionKindOut::Work, 4000),
        ] {
            db.record_llm_metric(
                &project_dir,
                &LlmMetricsRecord::from_byte_estimate(
                    1700000000,
                    step,
                    kind,
                    "vllm",
                    Some("qwen3.6"),
                    request_id,
                    1,
                    wall,
                    Some("stop"),
                    100,
                    50,
                ),
            )
            .expect("record_llm_metric");
        }
        for (tool, kind, wall) in [
            ("run_cargo", CallerKind::Llm, 8000_u64),
            ("cargo", CallerKind::Gate, 12000),
            ("run_cargo", CallerKind::Llm, 6000),
        ] {
            db.record_tool_timing(
                &project_dir,
                &ToolTimingRecord {
                    started_unix: 1700000000,
                    step: Some("DM3c".to_string()),
                    caller_kind: kind,
                    tool_name: tool.to_string(),
                    args_summary: String::new(),
                    status: "ok".to_string(),
                    wall_ms: wall,
                    exit_code: Some(0),
                    request_id: None,
                    turn_index: None,
                },
            )
            .expect("record_tool_timing");
        }
        db
    }

    #[test]
    fn bugs_by_step_groups_by_step_and_category() {
        let mut db = seed_db();
        let (cols, rows) =
            run_report(&mut db, ReportKind::BugsByStep, &ReportFilters::default()).expect("ok");
        assert_eq!(cols[0], "step");
        assert_eq!(cols[2], "count");
        assert!(rows.iter().any(|r| r[0] == Value::String("DM0".into())));
        assert!(rows.iter().any(|r| r[0] == Value::String("DM3c".into())));
    }

    #[test]
    fn bugs_open_filters_to_open_or_manual() {
        let mut db = seed_db();
        let (_, rows) =
            run_report(&mut db, ReportKind::BugsOpen, &ReportFilters::default()).expect("ok");
        assert_eq!(rows.len(), 2, "two non-resolved bugs in the fixture");
    }

    #[test]
    fn llm_time_by_step_sums_wall_ms() {
        let mut db = seed_db();
        let (_, rows) = run_report(
            &mut db,
            ReportKind::LlmTimeByStep,
            &ReportFilters::default(),
        )
        .expect("ok");
        // Two steps in the fixture; DM3c has the larger total.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::String("DM3c".into()));
        assert_eq!(rows[0][1], Value::Number(4000_i64.into()));
    }

    #[test]
    fn tool_time_by_tool_groups_and_sums() {
        let mut db = seed_db();
        let (_, rows) = run_report(
            &mut db,
            ReportKind::ToolTimeByTool,
            &ReportFilters::default(),
        )
        .expect("ok");
        // run_cargo (llm) and cargo (gate) -> two distinct rows.
        assert!(
            rows.iter()
                .any(|r| r[0] == Value::String("run_cargo".into()))
        );
        assert!(rows.iter().any(|r| r[0] == Value::String("cargo".into())));
    }

    #[test]
    fn step_filter_narrows_results() {
        let mut db = seed_db();
        let filters = ReportFilters {
            project: None,
            step: Some("DM3c".to_string()),
            limit: None,
        };
        let (_, rows) = run_report(&mut db, ReportKind::BugsByStep, &filters).expect("ok");
        // Both bug-002 and bug-003 are DM3c+test_failure -> one row, count 2.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::String("DM3c".into()));
        assert_eq!(rows[0][2], Value::Number(2_i64.into()));
    }
}
