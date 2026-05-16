//! DDL for the per-user ledger plus the `meta` key/value bootstrap.
//!
//! `apply_schema` is idempotent: every `CREATE TABLE` / `CREATE INDEX`
//! uses `IF NOT EXISTS`, and the `meta.schema_version` row is written
//! via `INSERT OR REPLACE` so future inline migrations bumping the
//! version write through cleanly. `ensure_machine_id` provisions
//! `meta.machine_id` on first init and returns the stable value on
//! subsequent opens.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

use super::util::wrap_sqlite;

/// Bump when an inline migration changes existing columns / indexes.
/// Adding new tables can be done idempotently via `CREATE TABLE IF NOT
/// EXISTS` without a version bump.
pub const SCHEMA_VERSION: u32 = 1;

pub(super) const META_KEY_SCHEMA_VERSION: &str = "schema_version";
pub(super) const META_KEY_MACHINE_ID: &str = "machine_id";

pub(super) fn apply_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Bugs mirror. One row per BugRecord; INSERT OR REPLACE on
        -- UNIQUE(project_dir, bug_id) so re-mirroring an updated bug
        -- wins. `record_json` holds the full BugRecord serialization so
        -- new fields show up here without a schema change.
        CREATE TABLE IF NOT EXISTS bugs (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir    TEXT NOT NULL,
            bug_id         TEXT NOT NULL,
            opened_at      TEXT NOT NULL,
            step           TEXT,
            category       TEXT,
            status         TEXT,
            user_identity  TEXT NOT NULL DEFAULT '',
            machine_id     TEXT NOT NULL DEFAULT '',
            record_json    TEXT NOT NULL,
            UNIQUE(project_dir, bug_id)
        );
        CREATE INDEX IF NOT EXISTS idx_bugs_step       ON bugs(step);
        CREATE INDEX IF NOT EXISTS idx_bugs_category   ON bugs(category);
        CREATE INDEX IF NOT EXISTS idx_bugs_status     ON bugs(status);
        CREATE INDEX IF NOT EXISTS idx_bugs_opened_at  ON bugs(opened_at);
        CREATE INDEX IF NOT EXISTS idx_bugs_user       ON bugs(user_identity);

        -- LLM metrics mirror. One row per RequestLlmResponse round-trip;
        -- INSERT OR IGNORE on the triple identifying a turn so re-mirror
        -- passes (e.g. via `db backfill`) are idempotent.
        CREATE TABLE IF NOT EXISTS llm_metrics (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir     TEXT NOT NULL,
            request_id      TEXT NOT NULL,
            turn_index      INTEGER NOT NULL,
            timestamp       TEXT NOT NULL,
            step            TEXT,
            kind            TEXT,
            backend         TEXT,
            model           TEXT,
            wall_ms         INTEGER,
            tokens_in       INTEGER,
            tokens_out      INTEGER,
            user_identity   TEXT NOT NULL DEFAULT '',
            machine_id      TEXT NOT NULL DEFAULT '',
            record_json     TEXT NOT NULL,
            UNIQUE(project_dir, request_id, turn_index)
        );
        CREATE INDEX IF NOT EXISTS idx_llm_metrics_step      ON llm_metrics(step);
        CREATE INDEX IF NOT EXISTS idx_llm_metrics_kind      ON llm_metrics(kind);
        CREATE INDEX IF NOT EXISTS idx_llm_metrics_backend   ON llm_metrics(backend);
        CREATE INDEX IF NOT EXISTS idx_llm_metrics_timestamp ON llm_metrics(timestamp);
        CREATE INDEX IF NOT EXISTS idx_llm_metrics_user      ON llm_metrics(user_identity);

        -- Tool timings (LLM-invoked tools + gate-driven shell checks).
        -- One row per invocation. `caller_kind` distinguishes the two
        -- sources ("llm" / "gate"). No UNIQUE on this table -- gate
        -- invocations don't have a stable identity (no request_id /
        -- turn_index), and constructing a synthetic discriminator that
        -- works across re-mirror passes is fragile. The live writer
        -- writes each row exactly once; `db backfill` will use a
        -- file-offset tracker in `meta` (one row per JSONL source) to
        -- skip already-imported lines instead of relying on row-level
        -- dedup.
        CREATE TABLE IF NOT EXISTS tool_timings (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir     TEXT NOT NULL,
            request_id      TEXT,
            turn_index      INTEGER,
            timestamp       TEXT NOT NULL,
            step            TEXT,
            caller_kind     TEXT NOT NULL,
            tool_name       TEXT NOT NULL,
            wall_ms         INTEGER NOT NULL,
            exit_code       INTEGER,
            user_identity   TEXT NOT NULL DEFAULT '',
            machine_id      TEXT NOT NULL DEFAULT '',
            record_json     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_tool_timings_step        ON tool_timings(step);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_caller_kind ON tool_timings(caller_kind);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_tool_name   ON tool_timings(tool_name);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_timestamp   ON tool_timings(timestamp);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_user        ON tool_timings(user_identity);

        -- Experiments mirror: same shape as the per-project
        -- `experiments.db` tables plus `project_dir`, `user_identity`,
        -- `machine_id` so cross-project / cross-machine queries can
        -- attribute rows.  Each table uses `INSERT OR REPLACE` on a
        -- `(project_dir, natural-key)` so updates (e.g. when a run's
        -- `metrics_summary` lands after the row was first inserted)
        -- cleanly replace the prior snapshot.
        CREATE TABLE IF NOT EXISTS experiment_runs (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir        TEXT NOT NULL,
            run_id             TEXT NOT NULL,
            timestamp          TEXT NOT NULL,
            git_commit         TEXT NOT NULL,
            git_branch         TEXT,
            git_dirty          INTEGER NOT NULL DEFAULT 0,
            config_fingerprint TEXT NOT NULL,
            manifest_path      TEXT,
            workload           TEXT,
            candidate          TEXT,
            study              TEXT,
            metrics_summary    TEXT,
            parent_run_id      TEXT,
            sweep_parameter    TEXT,
            sweep_value        TEXT,
            tags               TEXT,
            notes              TEXT,
            lifecycle          TEXT NOT NULL DEFAULT 'active',
            user_identity      TEXT NOT NULL DEFAULT '',
            machine_id         TEXT NOT NULL DEFAULT '',
            UNIQUE(project_dir, run_id)
        );
        CREATE INDEX IF NOT EXISTS idx_exp_runs_workload  ON experiment_runs(workload);
        CREATE INDEX IF NOT EXISTS idx_exp_runs_candidate ON experiment_runs(candidate);
        CREATE INDEX IF NOT EXISTS idx_exp_runs_study     ON experiment_runs(study);
        CREATE INDEX IF NOT EXISTS idx_exp_runs_timestamp ON experiment_runs(timestamp);
        CREATE INDEX IF NOT EXISTS idx_exp_runs_user      ON experiment_runs(user_identity);

        CREATE TABLE IF NOT EXISTS experiment_baselines (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir   TEXT NOT NULL,
            name          TEXT NOT NULL,
            run_id        TEXT NOT NULL,
            timestamp     TEXT NOT NULL,
            notes         TEXT,
            user_identity TEXT NOT NULL DEFAULT '',
            machine_id    TEXT NOT NULL DEFAULT '',
            UNIQUE(project_dir, name)
        );
        CREATE INDEX IF NOT EXISTS idx_exp_baselines_run  ON experiment_baselines(run_id);
        CREATE INDEX IF NOT EXISTS idx_exp_baselines_user ON experiment_baselines(user_identity);

        CREATE TABLE IF NOT EXISTS experiment_ppa_estimates (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir     TEXT NOT NULL,
            run_id          TEXT NOT NULL,
            level           INTEGER NOT NULL,
            technology_node TEXT NOT NULL,
            area_estimate   REAL,
            power_estimate  REAL,
            timing_met      INTEGER,
            source          TEXT,
            timestamp       TEXT NOT NULL,
            user_identity   TEXT NOT NULL DEFAULT '',
            machine_id      TEXT NOT NULL DEFAULT '',
            UNIQUE(project_dir, run_id, level, technology_node, source)
        );
        CREATE INDEX IF NOT EXISTS idx_exp_ppa_run  ON experiment_ppa_estimates(run_id);
        CREATE INDEX IF NOT EXISTS idx_exp_ppa_tech ON experiment_ppa_estimates(technology_node);
        CREATE INDEX IF NOT EXISTS idx_exp_ppa_user ON experiment_ppa_estimates(user_identity);
        "#,
    )
    .map_err(wrap_sqlite)?;

    // Persist the current schema version. Idempotent: `INSERT OR
    // REPLACE` so a future inline migration bumping the version writes
    // through cleanly.
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_KEY_SCHEMA_VERSION, SCHEMA_VERSION.to_string()],
    )
    .map_err(wrap_sqlite)?;
    Ok(())
}

pub(super) fn ensure_machine_id(conn: &Connection) -> Result<String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_KEY_MACHINE_ID],
            |row| row.get(0),
        )
        .optional()
        .map_err(wrap_sqlite)?;
    if let Some(id) = existing.filter(|s| !s.trim().is_empty()) {
        return Ok(id);
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params![META_KEY_MACHINE_ID, new_id],
    )
    .map_err(wrap_sqlite)?;
    Ok(new_id)
}
