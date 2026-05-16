//! Per-source mirror writers. Each method takes a `project_dir` plus
//! the source record (BugRecord / LlmMetricsRecord / ToolTimingRecord
//! / RunRow) and writes one row to the corresponding table.
//!
//! Idempotency story per table is documented at the method site --
//! `INSERT OR REPLACE` vs `INSERT OR IGNORE` reflects whether the
//! source side is mutable (BugRecord, experiment_runs metrics_summary)
//! or immutable (LlmMetricsRecord, tool_timings) once written.

use std::path::Path;

use rusqlite::params;

use crate::{Error, Result};

use super::GlobalDb;
use super::util::wrap_sqlite;
use super::{project_dir_key, user_identity};

impl GlobalDb {
    /// Mirror one [`LlmMetricsRecord`] from a project's
    /// `logs/llm-metrics.jsonl` into the global ledger.
    ///
    /// `INSERT OR IGNORE` on `UNIQUE(project_dir, request_id,
    /// turn_index)`: the metrics row is immutable once emitted, so a
    /// re-mirror pass (e.g. via `db backfill` over the JSONL tail) is a
    /// no-op rather than re-stamping the row's `id`. This matches the
    /// design's "immutable once written" semantics.
    ///
    /// Indexed columns surface the common filter axes (`timestamp`,
    /// `step`, `kind`, `backend`, `model`, `wall_ms`, token counts);
    /// the full record JSON is stored too so reports get every field
    /// without a schema change when new ones land.
    pub fn record_llm_metric(
        &self,
        project_dir: &Path,
        record: &crate::__internal::session::llm_metrics::LlmMetricsRecord,
    ) -> Result<()> {
        use crate::__internal::session::protocol::SessionKindOut;

        let project_dir = project_dir_key(project_dir);
        let record_json = serde_json::to_string(record)
            .map_err(|e| Error::State(format!("llm_metrics record serialize: {e}")))?;
        let user = user_identity();
        // Stable column-friendly string -- avoids round-tripping the
        // SessionKindOut enum through `serde_json::to_value` for one
        // discriminator.
        let kind_str = match record.kind {
            SessionKindOut::Work => "work",
            SessionKindOut::Critique => "critique",
            SessionKindOut::Qa => "qa",
        };
        self.conn
            .execute(
                r#"INSERT OR IGNORE INTO llm_metrics
                    (project_dir, request_id, turn_index, timestamp, step, kind,
                     backend, model, wall_ms, tokens_in, tokens_out,
                     user_identity, machine_id, record_json)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           ?13, ?14)"#,
                params![
                    project_dir,
                    record.request_id,
                    record.turn_index,
                    record.started_unix.to_string(),
                    record.step,
                    kind_str,
                    record.backend,
                    record.model,
                    record.wall_ms,
                    record.tokens_in,
                    record.tokens_out,
                    user,
                    self.machine_id,
                    record_json,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    /// Mirror one [`ToolTimingRecord`] from a project's
    /// `logs/tool-timings.jsonl` into the global ledger.
    ///
    /// `INSERT` (no UNIQUE collision target): the `tool_timings` table
    /// uses a plain auto-increment id and no row-level dedup. Gate-
    /// driven invocations don't have a stable identity (no request_id,
    /// no turn_index), and a synthetic discriminator that worked
    /// across re-mirror passes would be fragile; `db backfill` will
    /// use a JSONL-offset tracker in `meta` to skip already-imported
    /// lines instead.
    ///
    /// Indexed columns surface the common filter axes (`timestamp`,
    /// `step`, `caller_kind`, `tool_name`); the full record JSON is
    /// stored so reports get every field without a schema change.
    pub fn record_tool_timing(
        &self,
        project_dir: &Path,
        record: &crate::__internal::session::tool_timings::ToolTimingRecord,
    ) -> Result<()> {
        let project_dir = project_dir_key(project_dir);
        let record_json = serde_json::to_string(record)
            .map_err(|e| Error::State(format!("tool_timing record serialize: {e}")))?;
        let user = user_identity();
        self.conn
            .execute(
                r#"INSERT INTO tool_timings
                    (project_dir, request_id, turn_index, timestamp, step, caller_kind,
                     tool_name, wall_ms, exit_code, user_identity, machine_id, record_json)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                params![
                    project_dir,
                    record.request_id,
                    record.turn_index,
                    record.started_unix.to_string(),
                    record.step,
                    record.caller_kind.as_column_str(),
                    record.tool_name,
                    record.wall_ms,
                    record.exit_code,
                    user,
                    self.machine_id,
                    record_json,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    /// Mirror one experiment run from a project's `experiments.db`
    /// `runs` table into the global ledger.
    ///
    /// `INSERT OR REPLACE` on `UNIQUE(project_dir, run_id)`. The
    /// `metrics_summary` column on `runs` is populated *after* the row
    /// is first inserted (the orchestrator updates it once the run
    /// completes), so we re-mirror the row on every `update_metrics_summary`
    /// call -- INSERT OR REPLACE keeps the latest snapshot per
    /// project/run.
    pub fn record_experiment_run(
        &self,
        project_dir: &Path,
        row: &crate::__internal::tracking::index::RunRow,
    ) -> Result<()> {
        let project_dir = project_dir_key(project_dir);
        let user = user_identity();
        self.conn
            .execute(
                r#"INSERT OR REPLACE INTO experiment_runs
                    (project_dir, run_id, timestamp, git_commit, git_branch,
                     git_dirty, config_fingerprint, manifest_path, workload,
                     candidate, study, metrics_summary, parent_run_id,
                     sweep_parameter, sweep_value, tags, notes, lifecycle,
                     user_identity, machine_id)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"#,
                params![
                    project_dir,
                    row.run_id,
                    row.timestamp,
                    row.git_commit,
                    row.git_branch,
                    row.git_dirty as i64,
                    row.config_fingerprint,
                    row.manifest_path,
                    row.workload,
                    row.candidate,
                    row.study,
                    row.metrics_summary,
                    row.parent_run_id,
                    row.sweep_parameter,
                    row.sweep_value,
                    row.tags,
                    row.notes,
                    row.lifecycle,
                    user,
                    self.machine_id,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    /// Mirror one experiment baseline from a project's `experiments.db`
    /// `baselines` table. `INSERT OR REPLACE` on `UNIQUE(project_dir,
    /// name)` so baseline-renaming / re-pointing on a fresh `insert`
    /// (the project-local layer treats the name as a unique key)
    /// replaces the row cleanly.
    pub fn record_experiment_baseline(
        &self,
        project_dir: &Path,
        name: &str,
        run_id: &str,
        timestamp: &str,
        notes: Option<&str>,
    ) -> Result<()> {
        let project_dir = project_dir_key(project_dir);
        let user = user_identity();
        self.conn
            .execute(
                r#"INSERT OR REPLACE INTO experiment_baselines
                    (project_dir, name, run_id, timestamp, notes,
                     user_identity, machine_id)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![
                    project_dir,
                    name,
                    run_id,
                    timestamp,
                    notes,
                    user,
                    self.machine_id,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    /// Mirror one experiment PPA estimate row. `INSERT OR REPLACE` on
    /// `UNIQUE(project_dir, run_id, level, technology_node, source)`.
    /// No live writer exists for `ppa_estimates` yet (the schema is
    /// reserved for the perf-plan pipeline); the global-DB column shape
    /// lands here so the `db backfill` pass can hydrate it once data
    /// starts flowing.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn record_experiment_ppa_estimate(
        &self,
        project_dir: &Path,
        run_id: &str,
        level: i64,
        technology_node: &str,
        area_estimate: Option<f64>,
        power_estimate: Option<f64>,
        timing_met: Option<bool>,
        source: Option<&str>,
        timestamp: &str,
    ) -> Result<()> {
        let project_dir = project_dir_key(project_dir);
        let user = user_identity();
        self.conn
            .execute(
                r#"INSERT OR REPLACE INTO experiment_ppa_estimates
                    (project_dir, run_id, level, technology_node,
                     area_estimate, power_estimate, timing_met, source,
                     timestamp, user_identity, machine_id)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
                params![
                    project_dir,
                    run_id,
                    level,
                    technology_node,
                    area_estimate,
                    power_estimate,
                    timing_met.map(|b| b as i64),
                    source,
                    timestamp,
                    user,
                    self.machine_id,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    /// Mirror one [`BugRecord`] from a project's `bug-log.jsonl` into
    /// the global ledger. Idempotent: `INSERT OR REPLACE` on
    /// `UNIQUE(project_dir, bug_id)` so re-mirroring an updated record
    /// (after `append_event` / `resolve` mutates it in place) replaces
    /// the prior snapshot.
    ///
    /// Indexed columns (`opened_at`, `step`, `category`, `status`) are
    /// extracted from the record so reports can filter without parsing
    /// `record_json`. The full record JSON is stored too so new fields
    /// added to [`BugRecord`] surface in the global DB without a schema
    /// change.
    pub fn record_bug(
        &self,
        project_dir: &Path,
        record: &crate::__internal::bug_log::BugRecord,
    ) -> Result<()> {
        let project_dir = project_dir_key(project_dir);
        let record_json = serde_json::to_string(record)
            .map_err(|e| Error::State(format!("bug record serialize: {e}")))?;
        let user = user_identity();
        self.conn
            .execute(
                r#"INSERT OR REPLACE INTO bugs
                    (project_dir, bug_id, opened_at, step, category, status,
                     user_identity, machine_id, record_json)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    project_dir,
                    record.id,
                    record.opened_at,
                    record.step,
                    record.category,
                    record.status,
                    user,
                    self.machine_id,
                    record_json,
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }
}
