//! SQLite index at `.sim-flow/experiments.db` with the schema from
//! `docs/architecture/ai-flow/04-experiment-tracking.md`.
//!
//! Schema is applied idempotently on open so existing projects pick up new
//! tables as the plan evolves. Migrations that *change* existing columns
//! go through an explicit version bump (see `SCHEMA_VERSION`).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{Error, Result};

pub const EXPERIMENTS_DB: &str = "experiments.db";
pub const SCHEMA_VERSION: u32 = 1;

pub fn experiments_db_path(dot_sim_flow: &Path) -> PathBuf {
    dot_sim_flow.join(EXPERIMENTS_DB)
}

pub struct ExperimentIndex {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    pub id: i64,
    pub run_id: String,
    pub timestamp: String,
    pub git_commit: String,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub config_fingerprint: String,
    pub manifest_path: Option<String>,
    pub workload: Option<String>,
    pub candidate: Option<String>,
    pub study: Option<String>,
    pub metrics_summary: Option<String>,
    pub parent_run_id: Option<String>,
    pub sweep_parameter: Option<String>,
    pub sweep_value: Option<String>,
    pub tags: Option<String>,
    pub notes: Option<String>,
    pub lifecycle: String,
}

#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub workload: Option<String>,
    pub candidate: Option<String>,
    pub study: Option<String>,
    pub parent_run_id: Option<String>,
    pub limit: Option<usize>,
}

impl ExperimentIndex {
    pub fn open(dot_sim_flow: &Path) -> Result<Self> {
        std::fs::create_dir_all(dot_sim_flow).map_err(|source| Error::Io {
            path: dot_sim_flow.to_path_buf(),
            source,
        })?;
        let path = experiments_db_path(dot_sim_flow);
        Self::open_path(&path)
    }

    pub fn open_path(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(wrap_sqlite)?;
        let index = Self { conn };
        index.apply_schema()?;
        Ok(index)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(wrap_sqlite)?;
        let index = Self { conn };
        index.apply_schema()?;
        Ok(index)
    }

    fn apply_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS runs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT NOT NULL UNIQUE,
                    timestamp TEXT NOT NULL,
                    git_commit TEXT NOT NULL,
                    git_branch TEXT,
                    git_dirty INTEGER NOT NULL DEFAULT 0,
                    config_fingerprint TEXT NOT NULL,
                    manifest_path TEXT,
                    workload TEXT,
                    candidate TEXT,
                    study TEXT,
                    metrics_summary TEXT,
                    parent_run_id TEXT,
                    sweep_parameter TEXT,
                    sweep_value TEXT,
                    tags TEXT,
                    notes TEXT,
                    lifecycle TEXT NOT NULL DEFAULT 'active'
                );

                CREATE INDEX IF NOT EXISTS runs_by_workload ON runs(workload);
                CREATE INDEX IF NOT EXISTS runs_by_candidate ON runs(candidate);
                CREATE INDEX IF NOT EXISTS runs_by_study ON runs(study);
                CREATE INDEX IF NOT EXISTS runs_by_parent ON runs(parent_run_id);

                CREATE TABLE IF NOT EXISTS baselines (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    run_id TEXT NOT NULL REFERENCES runs(run_id),
                    timestamp TEXT NOT NULL,
                    notes TEXT
                );

                CREATE TABLE IF NOT EXISTS ppa_estimates (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT NOT NULL REFERENCES runs(run_id),
                    level INTEGER NOT NULL,
                    technology_node TEXT NOT NULL,
                    area_estimate REAL,
                    power_estimate REAL,
                    timing_met INTEGER,
                    source TEXT,
                    timestamp TEXT NOT NULL
                );
                "#,
            )
            .map_err(wrap_sqlite)?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<u32> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(wrap_sqlite)?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    /// Allocate the next sequence number. Numbers are monotonic per-DB; the
    /// sequence does not recycle when rows are deleted.
    pub fn next_sequence(&self) -> Result<u32> {
        let max: Option<String> = self
            .conn
            .query_row(
                "SELECT run_id FROM runs ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(wrap_sqlite)?;
        let current = max.as_deref().and_then(parse_sequence_prefix).unwrap_or(0);
        Ok(current + 1)
    }

    pub fn insert_run(&self, row: &RunRow) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO runs (
                    run_id, timestamp, git_commit, git_branch, git_dirty,
                    config_fingerprint, manifest_path, workload, candidate,
                    study, metrics_summary, parent_run_id, sweep_parameter,
                    sweep_value, tags, notes, lifecycle
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17
                )"#,
                params![
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
                ],
            )
            .map_err(wrap_sqlite)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>> {
        self.conn
            .query_row(
                "SELECT id, run_id, timestamp, git_commit, git_branch, git_dirty, \
                 config_fingerprint, manifest_path, workload, candidate, study, \
                 metrics_summary, parent_run_id, sweep_parameter, sweep_value, \
                 tags, notes, lifecycle \
                 FROM runs WHERE run_id = ?1",
                params![run_id],
                row_to_run,
            )
            .optional()
            .map_err(wrap_sqlite)
    }

    pub fn list_runs(&self, filter: &RunFilter) -> Result<Vec<RunRow>> {
        let mut sql = String::from(
            "SELECT id, run_id, timestamp, git_commit, git_branch, git_dirty, \
             config_fingerprint, manifest_path, workload, candidate, study, \
             metrics_summary, parent_run_id, sweep_parameter, sweep_value, \
             tags, notes, lifecycle FROM runs WHERE 1 = 1",
        );
        let mut args: Vec<String> = Vec::new();
        if let Some(w) = &filter.workload {
            sql.push_str(" AND workload = ?");
            args.push(w.clone());
        }
        if let Some(c) = &filter.candidate {
            sql.push_str(" AND candidate = ?");
            args.push(c.clone());
        }
        if let Some(s) = &filter.study {
            sql.push_str(" AND study = ?");
            args.push(s.clone());
        }
        if let Some(p) = &filter.parent_run_id {
            sql.push_str(" AND parent_run_id = ?");
            args.push(p.clone());
        }
        sql.push_str(" ORDER BY id DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        let mut stmt = self.conn.prepare(&sql).map_err(wrap_sqlite)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), row_to_run)
            .map_err(wrap_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(wrap_sqlite)?);
        }
        Ok(out)
    }

    pub fn count_runs(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
            .map_err(wrap_sqlite)?;
        Ok(n as usize)
    }

    pub fn update_metrics_summary(&self, run_id: &str, metrics_json: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE runs SET metrics_summary = ?1 WHERE run_id = ?2",
                params![metrics_json, run_id],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    pub fn insert_baseline(
        &self,
        name: &str,
        run_id: &str,
        timestamp: &str,
        notes: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO baselines (name, run_id, timestamp, notes) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![name, run_id, timestamp, notes],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

    pub fn baseline_run_id(&self, name: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT run_id FROM baselines WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .map_err(wrap_sqlite)
    }

    pub fn list_baselines(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, run_id, timestamp FROM baselines ORDER BY id")
            .map_err(wrap_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(wrap_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(wrap_sqlite)?);
        }
        Ok(out)
    }
}

fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        timestamp: row.get(2)?,
        git_commit: row.get(3)?,
        git_branch: row.get(4)?,
        git_dirty: row.get::<_, i64>(5)? != 0,
        config_fingerprint: row.get(6)?,
        manifest_path: row.get(7)?,
        workload: row.get(8)?,
        candidate: row.get(9)?,
        study: row.get(10)?,
        metrics_summary: row.get(11)?,
        parent_run_id: row.get(12)?,
        sweep_parameter: row.get(13)?,
        sweep_value: row.get(14)?,
        tags: row.get(15)?,
        notes: row.get(16)?,
        lifecycle: row.get(17)?,
    })
}

fn wrap_sqlite(e: rusqlite::Error) -> Error {
    Error::State(format!("sqlite error: {e}"))
}

fn parse_sequence_prefix(run_id: &str) -> Option<u32> {
    let end = run_id.find('-').unwrap_or(run_id.len());
    run_id[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row(run_id: &str) -> RunRow {
        RunRow {
            id: 0,
            run_id: run_id.to_string(),
            timestamp: "2026-04-21T00:00:00Z".into(),
            git_commit: "deadbeef".into(),
            git_branch: Some("main".into()),
            git_dirty: false,
            config_fingerprint: "fp".into(),
            manifest_path: None,
            workload: Some("throughput-stress".into()),
            candidate: None,
            study: None,
            metrics_summary: None,
            parent_run_id: None,
            sweep_parameter: None,
            sweep_value: None,
            tags: None,
            notes: None,
            lifecycle: "active".into(),
        }
    }

    #[test]
    fn schema_applies_and_version_reports() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        assert_eq!(index.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn insert_and_retrieve_run() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        index.insert_run(&sample_row("001-throughput")).unwrap();
        let got = index.get_run("001-throughput").unwrap().unwrap();
        assert_eq!(got.workload.as_deref(), Some("throughput-stress"));
    }

    #[test]
    fn next_sequence_increments() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        assert_eq!(index.next_sequence().unwrap(), 1);
        index.insert_run(&sample_row("001-a")).unwrap();
        assert_eq!(index.next_sequence().unwrap(), 2);
        index.insert_run(&sample_row("042-b")).unwrap();
        assert_eq!(index.next_sequence().unwrap(), 43);
    }

    #[test]
    fn filter_by_workload() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        let mut a = sample_row("001-a");
        a.workload = Some("a".into());
        let mut b = sample_row("002-b");
        b.workload = Some("b".into());
        index.insert_run(&a).unwrap();
        index.insert_run(&b).unwrap();
        let only_a = index
            .list_runs(&RunFilter {
                workload: Some("a".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].run_id, "001-a");
    }

    #[test]
    fn baseline_lifecycle() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        index.insert_run(&sample_row("001-a")).unwrap();
        index
            .insert_baseline("v1", "001-a", "now", Some("initial"))
            .unwrap();
        assert_eq!(
            index.baseline_run_id("v1").unwrap().as_deref(),
            Some("001-a")
        );
        let list = index.list_baselines().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "v1");
    }

    #[test]
    fn update_metrics_summary_persists() {
        let index = ExperimentIndex::open_in_memory().unwrap();
        index.insert_run(&sample_row("001-a")).unwrap();
        index
            .update_metrics_summary("001-a", "{\"throughput\":0.88}")
            .unwrap();
        let got = index.get_run("001-a").unwrap().unwrap();
        assert_eq!(
            got.metrics_summary.as_deref(),
            Some("{\"throughput\":0.88}")
        );
    }
}
