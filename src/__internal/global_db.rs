//! Per-user SQLite ledger at the platform data dir.
//!
//! Mirrors append-only telemetry (`bug-log.jsonl`, `llm-metrics.jsonl`,
//! `tool-timings.jsonl`) across every project a developer runs on this
//! machine. The per-project JSONL files stay authoritative for the live
//! project; this DB is the long-running cross-project ledger.
//!
//! See [`docs/brainstorming/global-database.md`](../../../../docs/brainstorming/global-database.md)
//! for the design, locked-in decisions, and Phase 1 / Phase 1.5
//! sequencing.
//!
//! Path resolution:
//!   macOS:   `~/Library/Application Support/sim-flow/sim-flow.db`
//!   Linux:   `~/.local/share/sim-flow/sim-flow.db` (respects `$XDG_DATA_HOME`)
//!   Windows: `%APPDATA%\sim-flow\data\sim-flow.db`
//!
//! Failure mode is best-effort: opening the DB or any write may fail (disk
//! full, permission denied, IO race); callers receive `None` from
//! [`with_db`] and a single `tracing::warn!` records the cause. Telemetry
//! must never block the simulation, the agent, or a CLI command.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Error, Result};

pub const SIM_FLOW_DB: &str = "sim-flow.db";

/// Bump when an inline migration changes existing columns / indexes.
/// Adding new tables can be done idempotently via `CREATE TABLE IF NOT
/// EXISTS` without a version bump.
pub const SCHEMA_VERSION: u32 = 1;

const META_KEY_SCHEMA_VERSION: &str = "schema_version";
const META_KEY_MACHINE_ID: &str = "machine_id";

/// Resolve the default platform path for the global DB.
///
/// Returns `None` only when `directories` can't resolve a home directory
/// (e.g. `$HOME` unset in a sandboxed environment); in that case the
/// caller should treat the DB as unavailable and skip the mirror.
pub fn default_db_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "sim-flow").map(|dirs| dirs.data_dir().join(SIM_FLOW_DB))
}

/// Best-effort resolver for the per-user identity stamped on each row.
///
/// Resolution order:
///   1. `SIM_FLOW_USER_IDENTITY` env var (operator override).
///   2. `git config --global user.email` (the canonical developer id
///      every contributor already has configured).
///   3. `$USER` env var (Unix) / `$USERNAME` env var (Windows).
///   4. The literal string `"unknown"`.
///
/// Cached at process startup -- a long-running session that changes its
/// git config mid-run still uses the value captured at first call.
pub fn user_identity() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Ok(value) = std::env::var("SIM_FLOW_USER_IDENTITY") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
            if let Some(email) = git_user_email() {
                return email;
            }
            for var in ["USER", "USERNAME", "LOGNAME"] {
                if let Ok(value) = std::env::var(var) {
                    let trimmed = value.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
            "unknown".to_string()
        })
        .clone()
}

/// Canonicalize a project directory into the string form stored in the
/// `project_dir` column on every telemetry row.
///
/// Resolves symlinks and converts to absolute path when possible, so two
/// different shell paths into the same project (one through a symlink,
/// one direct) don't fragment that project's rows across the DB. Falls
/// back to the lexical path string when canonicalization fails (e.g. the
/// directory was deleted between op and mirror) -- the row still lands,
/// just with the un-resolved string as the key.
pub fn project_dir_key(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn git_user_email() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", "--get", "user.email"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let email = String::from_utf8(output.stdout).ok()?;
    let trimmed = email.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Owns the connection to the per-user SQLite ledger. Process-wide
/// singleton accessed via [`with_db`].
pub struct GlobalDb {
    conn: Connection,
    machine_id: String,
}

impl GlobalDb {
    /// Open or create the global DB at `path`. Creates parent dirs,
    /// enables WAL on first init, applies the schema idempotently, and
    /// initializes / reads the per-machine id.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path).map_err(wrap_sqlite)?;
        Self::initialize(conn)
    }

    /// In-memory variant for tests. Schema bootstrap and machine-id
    /// initialization run the same way as on-disk.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(wrap_sqlite)?;
        Self::initialize(conn)
    }

    /// Open the DB at [`default_db_path`].
    pub fn open_default() -> Result<Self> {
        let path = default_db_path().ok_or_else(|| {
            Error::State(
                "global DB unavailable: directories::ProjectDirs returned None (HOME unset?)"
                    .to_string(),
            )
        })?;
        Self::open(&path)
    }

    fn initialize(conn: Connection) -> Result<Self> {
        // Connection-level pragmas. WAL persists in the file header on
        // first set, so subsequent opens inherit it -- the
        // `pragma_update` is cheap on the steady-state path.
        //
        // `synchronous=NORMAL` is the recommended pairing for WAL: fsync
        // only at checkpoint boundaries, durable enough for telemetry,
        // no per-insert syscall.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(wrap_sqlite)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(wrap_sqlite)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(wrap_sqlite)?;
        apply_schema(&conn)?;
        let machine_id = ensure_machine_id(&conn)?;
        Ok(Self { conn, machine_id })
    }

    /// Stable per-machine identifier (UUID v4 generated at first init,
    /// persisted in the `meta` table). Used as a row column on shared-
    /// team data so cross-machine queries can attribute rows to source.
    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    /// Current persisted schema version (always `SCHEMA_VERSION` after a
    /// successful open).
    pub fn schema_version(&self) -> Result<u32> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_KEY_SCHEMA_VERSION],
                |row| row.get(0),
            )
            .optional()
            .map_err(wrap_sqlite)?;
        raw.as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| Error::State("meta.schema_version missing or malformed".to_string()))
    }

    /// Access the underlying connection. Used by writer modules in
    /// follow-up tasks (llm-metrics / tool-timings / experiments
    /// mirrors).
    #[allow(dead_code)] // wired by Phase 1 mirror tasks (metrics / timings / experiments)
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable connection access for transactions.
    #[allow(dead_code)] // wired by Phase 1 mirror tasks
    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// `SELECT count(*) FROM <table>`. Surfaced as a typed method so
    /// the `sim-flow db stats` CLI doesn't need access to the raw
    /// connection.
    pub fn count(&self, table: &str) -> Result<i64> {
        if !is_safe_table_name(table) {
            return Err(Error::State(format!("unsafe table name {table:?}")));
        }
        self.conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(wrap_sqlite)
    }

    /// `SELECT MAX(<column>) FROM <table>` -- latest non-null
    /// timestamp value, or `None` when the table is empty. Surfaced
    /// as a typed method (same rationale as `count`).
    pub fn latest_timestamp(&self, table: &str, column: &str) -> Result<Option<String>> {
        if !is_safe_table_name(table) || !is_safe_table_name(column) {
            return Err(Error::State(format!(
                "unsafe identifier {table:?} / {column:?}"
            )));
        }
        self.conn
            .query_row(&format!("SELECT MAX({column}) FROM {table}"), [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(wrap_sqlite)
    }

    /// Execute a read-only SQL query and return the columns + rows as
    /// JSON values. Used by the `sim-flow db query` CLI.
    ///
    /// Sets `PRAGMA query_only=ON` on the connection before preparing
    /// the statement; INSERT / UPDATE / DELETE / DDL are rejected by
    /// SQLite with a readonly-database error. The pragma is reset on
    /// return so subsequent writers on the same `GlobalDb` (e.g. when
    /// the singleton itself is queried) aren't accidentally locked
    /// out.
    ///
    /// SQLite values are mapped to JSON types: NULL -> `null`,
    /// INTEGER -> `Number`, REAL -> `Number`, TEXT -> `String`, BLOB ->
    /// base64-encoded `String` prefixed with `"base64:"` so the
    /// caller can detect it.
    pub fn query_read_only(
        &mut self,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        use rusqlite::types::ValueRef;
        self.conn
            .pragma_update(None, "query_only", "ON")
            .map_err(wrap_sqlite)?;
        let result = (|| -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
            let mut stmt = self.conn.prepare(sql).map_err(wrap_sqlite)?;
            let columns: Vec<String> =
                stmt.column_names().iter().map(|s| (*s).to_string()).collect();
            let column_count = columns.len();
            let mut rows_iter = stmt.query([]).map_err(wrap_sqlite)?;
            let mut out_rows: Vec<Vec<serde_json::Value>> = Vec::new();
            while let Some(row) = rows_iter.next().map_err(wrap_sqlite)? {
                let mut cells: Vec<serde_json::Value> = Vec::with_capacity(column_count);
                for i in 0..column_count {
                    let value_ref = row.get_ref(i).map_err(wrap_sqlite)?;
                    let json = match value_ref {
                        ValueRef::Null => serde_json::Value::Null,
                        ValueRef::Integer(n) => serde_json::Value::Number(n.into()),
                        ValueRef::Real(f) => serde_json::Number::from_f64(f)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                        ValueRef::Text(bytes) => {
                            serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned())
                        }
                        ValueRef::Blob(bytes) => serde_json::Value::String(format!(
                            "base64:{}",
                            base64_encode_bytes(bytes)
                        )),
                    };
                    cells.push(json);
                }
                out_rows.push(cells);
            }
            Ok((columns, out_rows))
        })();
        // Reset the pragma so subsequent writes via this connection
        // aren't blocked. Errors here are non-fatal -- the connection
        // will be dropped at end of CLI invocation anyway.
        let _ = self.conn.pragma_update(None, "query_only", "OFF");
        result
    }

    /// Persisted per-source byte offset for `db backfill`. Used by
    /// the `tool_timings` import path (which can't dedup via a
    /// UNIQUE index because gate-driven rows have no stable identity)
    /// to skip already-imported lines on re-run.
    ///
    /// The key shape is `backfill_offset:<project_dir>:<source>` so a
    /// future per-source tracker (bugs / metrics offsets, if we ever
    /// want them for performance) can use the same `meta` slot.
    pub fn backfill_offset(&self, project_dir: &Path, source: &str) -> Result<u64> {
        let key = backfill_offset_key(project_dir, source);
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(wrap_sqlite)?;
        Ok(raw.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    /// Persist the byte offset reached by a `db backfill` pass.
    pub fn set_backfill_offset(&self, project_dir: &Path, source: &str, offset: u64) -> Result<()> {
        let key = backfill_offset_key(project_dir, source);
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
                params![key, offset.to_string()],
            )
            .map_err(wrap_sqlite)?;
        Ok(())
    }

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

fn apply_schema(conn: &Connection) -> Result<()> {
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

fn ensure_machine_id(conn: &Connection) -> Result<String> {
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

fn wrap_sqlite(source: rusqlite::Error) -> Error {
    Error::State(format!("global-db sqlite error: {source}"))
}

/// Minimal base64 encoder for BLOB cells in `query_read_only`. Keeps
/// the dependency surface unchanged -- pulling in a base64 crate just
/// for this tiny case isn't worth it.
fn base64_encode_bytes(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        let b2 = bytes[i + 2] as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b0 = bytes[i] as u32;
        out.push(ALPHABET[((b0 >> 2) & 0x3f) as usize] as char);
        out.push(ALPHABET[((b0 << 4) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        out.push(ALPHABET[((b0 >> 2) & 0x3f) as usize] as char);
        out.push(ALPHABET[(((b0 << 4) | (b1 >> 4)) & 0x3f) as usize] as char);
        out.push(ALPHABET[((b1 << 2) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// Defensive guard for the table / column names interpolated into
/// `count()` / `latest_timestamp()` queries. Rusqlite's parameter
/// binding doesn't cover identifiers, and the CLI passes them in from
/// a closed allowlist anyway -- this is belt-and-suspenders so a typo
/// in the allowlist can't produce a query-injection vector.
/// Compose the `meta` key for a per-source backfill offset.
fn backfill_offset_key(project_dir: &Path, source: &str) -> String {
    format!(
        "backfill_offset:{}:{}",
        project_dir_key(project_dir),
        source
    )
}

fn is_safe_table_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ── Process-wide singleton ──────────────────────────────────────────────

enum DbState {
    /// Open and ready to accept writes.
    Open(GlobalDb),
    /// Open was attempted and failed. Skip writes; do not retry, so a
    /// misconfigured environment doesn't spam logs every turn.
    Disabled,
}

static GLOBAL_DB: OnceLock<Mutex<DbState>> = OnceLock::new();

fn state() -> &'static Mutex<DbState> {
    GLOBAL_DB.get_or_init(|| match GlobalDb::open_default() {
        Ok(db) => Mutex::new(DbState::Open(db)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "global DB unavailable; per-entry mirror disabled for this process"
            );
            Mutex::new(DbState::Disabled)
        }
    })
}

/// Run a closure against the global DB if it's currently available.
///
/// Returns `None` when the DB is unavailable (open failed at startup,
/// poisoned mutex from a panicking writer, or the closure itself
/// returned an error). The closure's error -- when it occurs -- is
/// logged via `tracing::warn!` inside this function; callers don't need
/// to log again.
///
/// This is the only sanctioned entry point for telemetry writers. The
/// "best-effort, never block the caller" semantic is enforced here so
/// every call site can be `let _ = global_db::with_db(...)` with no
/// error plumbing.
pub fn with_db<F, T>(f: F) -> Option<T>
where
    F: FnOnce(&mut GlobalDb) -> Result<T>,
{
    let mutex = state();
    let mut guard = match mutex.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("global DB mutex poisoned; mirror skipped for this call");
            // Recover the inner state -- a previous panic poisoned the
            // mutex but the underlying DB is still likely usable. If
            // the recovered state is `Disabled` we still skip; if it's
            // `Open` we proceed.
            poisoned.into_inner()
        }
    };
    let db = match &mut *guard {
        DbState::Open(db) => db,
        DbState::Disabled => return None,
    };
    match f(db) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(error = %e, "global DB operation failed; skipping");
            None
        }
    }
}

/// Test-only: install a pre-built `GlobalDb` (typically `open_in_memory`)
/// as the process singleton, bypassing the lazy `open_default` path.
///
/// Returns `Err` if the singleton has already been initialized -- tests
/// that need an isolated DB should call this before any code path that
/// would touch `with_db`.
#[cfg(test)]
pub fn install_for_test(db: GlobalDb) -> std::result::Result<(), GlobalDb> {
    GLOBAL_DB
        .set(Mutex::new(DbState::Open(db)))
        .map_err(|mutex| match mutex.into_inner().unwrap() {
            DbState::Open(db) => db,
            DbState::Disabled => unreachable!("set() rejects only when already initialized"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_succeeds_and_schema_is_applied() {
        let db = GlobalDb::open_in_memory().expect("open in-memory");
        assert_eq!(db.schema_version().expect("schema version"), SCHEMA_VERSION);
        // All four expected tables are present.
        for table in ["meta", "bugs", "llm_metrics", "tool_timings"] {
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap_or_else(|e| panic!("query sqlite_master for {table}: {e}"));
            assert_eq!(count, 1, "table {table} should exist exactly once");
        }
    }

    #[test]
    fn schema_apply_is_idempotent() {
        let dir = tempdir();
        let path = dir.path().join(SIM_FLOW_DB);
        let first = GlobalDb::open(&path).expect("first open");
        let machine_id_one = first.machine_id().to_string();
        drop(first);

        // Reopen: schema reapply must succeed, machine_id must be stable
        // across reopens.
        let second = GlobalDb::open(&path).expect("second open");
        assert_eq!(
            second.machine_id(),
            machine_id_one.as_str(),
            "machine_id should be stable across reopens"
        );
        assert_eq!(second.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn machine_id_is_a_uuid() {
        let db = GlobalDb::open_in_memory().expect("open");
        let id = db.machine_id();
        // UUID v4 string form is 36 chars including hyphens; parse round-trips.
        assert_eq!(id.len(), 36, "machine_id should be a UUID string: {id:?}");
        uuid::Uuid::parse_str(id).expect("machine_id parses as UUID");
    }

    #[test]
    fn default_db_path_resolves_or_is_none() {
        // We can't assert a specific value (depends on $HOME) but the
        // call must not panic. If a path is returned, the parent dir
        // must be inside a directory named `sim-flow` somewhere in its
        // path components.
        if let Some(path) = default_db_path() {
            assert_eq!(path.file_name().and_then(|s| s.to_str()), Some(SIM_FLOW_DB));
            assert!(
                path.components()
                    .any(|c| c.as_os_str().to_string_lossy().contains("sim-flow")),
                "expected `sim-flow` segment somewhere in {path:?}"
            );
        }
    }

    #[test]
    fn user_identity_is_non_empty() {
        let id = user_identity();
        assert!(!id.is_empty(), "user_identity should never be empty");
    }

    #[test]
    fn project_dir_key_canonicalizes_when_possible() {
        let dir = tempdir();
        // Existing path canonicalizes (absolute, symlinks resolved).
        let key = project_dir_key(dir.path());
        let canonical = dir.path().canonicalize().expect("canonicalize tempdir");
        assert_eq!(key, canonical.to_string_lossy());
        assert!(
            Path::new(&key).is_absolute(),
            "project_dir_key must be absolute when canonicalize succeeds: {key:?}"
        );
    }

    #[test]
    fn project_dir_key_falls_back_when_canonicalize_fails() {
        let nonexistent = Path::new("/this/path/does/not/exist/abc123xyz");
        // Canonicalize fails -> the lexical string still comes back as the
        // key (so the row lands rather than getting dropped on a missing
        // dir). The exact value is the input path's string form.
        assert_eq!(project_dir_key(nonexistent), nonexistent.to_string_lossy());
    }

    #[test]
    fn record_bug_inserts_row_and_round_trips_fields() {
        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        let bug = crate::__internal::bug_log::BugRecord {
            id: "bug-007".to_string(),
            opened_at: "1700000000".to_string(),
            closed_at: None,
            step: "DM3c".to_string(),
            milestone: Some("test-milestone-03-stress.md".to_string()),
            category: "test_flake".to_string(),
            issue: "tarpaulin times out under load".to_string(),
            events: Vec::new(),
            resolution: None,
            status: "open".to_string(),
        };
        db.record_bug(&project_dir, &bug).expect("record_bug");

        let (bug_id, step, category, status, mid): (String, String, String, String, String) = db
            .conn()
            .query_row(
                "SELECT bug_id, step, category, status, machine_id FROM bugs",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .expect("query row");
        assert_eq!(bug_id, "bug-007");
        assert_eq!(step, "DM3c");
        assert_eq!(category, "test_flake");
        assert_eq!(status, "open");
        assert_eq!(mid, db.machine_id(), "machine_id must be stamped on row");
    }

    #[test]
    fn record_llm_metric_inserts_row_and_round_trips_fields() {
        use crate::__internal::session::llm_metrics::LlmMetricsRecord;
        use crate::__internal::session::protocol::SessionKindOut;

        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        let rec = LlmMetricsRecord::from_byte_estimate(
            1700000000,
            "DM0",
            SessionKindOut::Work,
            "vllm",
            Some("qwen3.6"),
            "req-42",
            5,
            12_500,
            Some("stop"),
            4096,
            2048,
        );
        db.record_llm_metric(&project_dir, &rec)
            .expect("record_llm_metric");

        let (req, turn, step, kind, backend, wall_ms): (String, i64, String, String, String, i64) =
            db.conn()
                .query_row(
                    "SELECT request_id, turn_index, step, kind, backend, wall_ms FROM llm_metrics",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .expect("query row");
        assert_eq!(req, "req-42");
        assert_eq!(turn, 5);
        assert_eq!(step, "DM0");
        assert_eq!(kind, "work");
        assert_eq!(backend, "vllm");
        assert_eq!(wall_ms, 12_500);
    }

    #[test]
    fn record_llm_metric_insert_or_ignore_keeps_first_write() {
        use crate::__internal::session::llm_metrics::LlmMetricsRecord;
        use crate::__internal::session::protocol::SessionKindOut;

        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        let first = LlmMetricsRecord::from_byte_estimate(
            1700000000,
            "DM0",
            SessionKindOut::Work,
            "vllm",
            None,
            "req-1",
            1,
            500,
            Some("stop"),
            100,
            50,
        );
        // Same (project_dir, request_id, turn_index) but different wall_ms
        // -- INSERT OR IGNORE must keep the first write.
        let dup = LlmMetricsRecord::from_byte_estimate(
            1700000999,
            "DM0",
            SessionKindOut::Work,
            "vllm",
            None,
            "req-1",
            1,
            9999,
            Some("stop"),
            100,
            50,
        );
        db.record_llm_metric(&project_dir, &first).expect("first");
        db.record_llm_metric(&project_dir, &dup)
            .expect("dup is no-op");

        let row_count: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM llm_metrics", [], |row| row.get(0))
            .expect("count");
        assert_eq!(row_count, 1, "INSERT OR IGNORE should keep one row");
        let wall_ms: i64 = db
            .conn()
            .query_row(
                "SELECT wall_ms FROM llm_metrics WHERE request_id = ?1",
                params!["req-1"],
                |row| row.get(0),
            )
            .expect("wall_ms");
        assert_eq!(wall_ms, 500, "first write's wall_ms must win");
    }

    #[test]
    fn record_experiment_run_inserts_and_replaces_on_repeat() {
        use crate::__internal::tracking::index::RunRow;

        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        let mut row = RunRow {
            id: 0,
            run_id: "run-001-abc".to_string(),
            timestamp: "1700000000".to_string(),
            git_commit: "deadbeef".to_string(),
            git_branch: Some("main".to_string()),
            git_dirty: false,
            config_fingerprint: "abc123".to_string(),
            manifest_path: Some("manifests/cell1.json".to_string()),
            workload: Some("synthetic".to_string()),
            candidate: Some("rgb_toy".to_string()),
            study: Some("baseline_sweep".to_string()),
            metrics_summary: None,
            parent_run_id: None,
            sweep_parameter: None,
            sweep_value: None,
            tags: None,
            notes: None,
            lifecycle: "active".to_string(),
        };
        db.record_experiment_run(&project_dir, &row)
            .expect("first run");

        // INSERT OR REPLACE: re-mirroring with metrics_summary set
        // replaces the row in place; row count stays at 1, the new
        // summary wins.
        row.metrics_summary = Some(r#"{"throughput":3.21}"#.to_string());
        db.record_experiment_run(&project_dir, &row)
            .expect("second run");
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM experiment_runs WHERE run_id = ?1",
                params!["run-001-abc"],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
        let metrics: Option<String> = db
            .conn()
            .query_row(
                "SELECT metrics_summary FROM experiment_runs WHERE run_id = ?1",
                params!["run-001-abc"],
                |r| r.get(0),
            )
            .expect("metrics");
        assert_eq!(metrics.as_deref(), Some(r#"{"throughput":3.21}"#));
    }

    #[test]
    fn count_and_latest_timestamp_match_row_state() {
        use crate::__internal::bug_log::BugRecord;

        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        assert_eq!(db.count("bugs").expect("count empty"), 0);
        assert_eq!(
            db.latest_timestamp("bugs", "opened_at").expect("ts empty"),
            None
        );

        for (i, opened_at) in ["1700000000", "1700000010", "1700000005"]
            .iter()
            .enumerate()
        {
            db.record_bug(
                &project_dir,
                &BugRecord {
                    id: format!("bug-{:03}", i + 1),
                    opened_at: (*opened_at).to_string(),
                    closed_at: None,
                    step: "DM0".to_string(),
                    milestone: None,
                    category: "other".to_string(),
                    issue: "test".to_string(),
                    events: Vec::new(),
                    resolution: None,
                    status: "open".to_string(),
                },
            )
            .expect("record_bug");
        }
        assert_eq!(db.count("bugs").expect("count 3"), 3);
        assert_eq!(
            db.latest_timestamp("bugs", "opened_at").expect("latest ts"),
            Some("1700000010".to_string())
        );
    }

    #[test]
    fn query_read_only_returns_rows_and_rejects_writes() {
        use crate::__internal::bug_log::BugRecord;

        let mut db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        db.record_bug(
            &project_dir,
            &BugRecord {
                id: "bug-001".into(),
                opened_at: "1700000000".into(),
                closed_at: None,
                step: "DM0".into(),
                milestone: None,
                category: "compile_error".into(),
                issue: "x".into(),
                events: Vec::new(),
                resolution: None,
                status: "open".into(),
            },
        )
        .expect("record_bug");

        let (cols, rows) = db
            .query_read_only("SELECT category, count(*) FROM bugs GROUP BY category")
            .expect("read query");
        assert_eq!(cols, vec!["category".to_string(), "count(*)".to_string()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::Value::String("compile_error".into()));
        assert_eq!(rows[0][1], serde_json::Value::Number(1i64.into()));

        // PRAGMA query_only blocks writes inside the closure.
        let err = db
            .query_read_only("DELETE FROM bugs")
            .expect_err("write should be rejected");
        assert!(
            format!("{err}").contains("readonly"),
            "expected readonly-rejection, got: {err}"
        );

        // After the closure the connection is back to read-write so
        // subsequent writers on the singleton aren't locked out.
        db.record_bug(
            &project_dir,
            &BugRecord {
                id: "bug-002".into(),
                opened_at: "1700000010".into(),
                closed_at: None,
                step: "DM0".into(),
                milestone: None,
                category: "compile_error".into(),
                issue: "y".into(),
                events: Vec::new(),
                resolution: None,
                status: "open".into(),
            },
        )
        .expect("post-query write should succeed");
    }

    #[test]
    fn count_rejects_unsafe_table_names() {
        let db = GlobalDb::open_in_memory().expect("open");
        let err = db
            .count("bugs; DROP TABLE bugs;--")
            .expect_err("should reject");
        assert!(format!("{err}").contains("unsafe table name"));
    }

    #[test]
    fn record_experiment_baseline_replaces_on_same_name() {
        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        db.record_experiment_baseline(
            &project_dir,
            "best_known",
            "run-001-abc",
            "1700000000",
            Some("initial pin"),
        )
        .expect("first");
        db.record_experiment_baseline(
            &project_dir,
            "best_known",
            "run-007-xyz",
            "1700000999",
            Some("re-pinned after sweep"),
        )
        .expect("second");

        let (run_id, notes): (String, Option<String>) = db
            .conn()
            .query_row(
                "SELECT run_id, notes FROM experiment_baselines WHERE name = ?1",
                params!["best_known"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("query");
        assert_eq!(run_id, "run-007-xyz");
        assert_eq!(notes.as_deref(), Some("re-pinned after sweep"));
    }

    #[test]
    fn record_tool_timing_round_trips_llm_and_gate_kinds() {
        use crate::__internal::session::tool_timings::{CallerKind, ToolTimingRecord};

        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();

        let llm = ToolTimingRecord {
            started_unix: 1_700_000_000,
            step: Some("DM3c".to_string()),
            caller_kind: CallerKind::Llm,
            tool_name: "run_cargo".to_string(),
            args_summary: "test --quiet".to_string(),
            status: "ok".to_string(),
            wall_ms: 4_200,
            exit_code: Some(0),
            request_id: Some("req-1".to_string()),
            turn_index: Some(2),
        };
        let gate = ToolTimingRecord {
            started_unix: 1_700_000_100,
            step: Some("DM3c".to_string()),
            caller_kind: CallerKind::Gate,
            tool_name: "cargo".to_string(),
            args_summary: "clippy --all-targets --quiet".to_string(),
            status: "ok".to_string(),
            wall_ms: 12_500,
            exit_code: Some(0),
            request_id: None,
            turn_index: None,
        };
        db.record_tool_timing(&project_dir, &llm)
            .expect("record llm");
        db.record_tool_timing(&project_dir, &gate)
            .expect("record gate");

        let rows: Vec<(String, String, i64, Option<String>)> = db
            .conn()
            .prepare(
                "SELECT caller_kind, tool_name, wall_ms, request_id FROM tool_timings ORDER BY id",
            )
            .expect("prep")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .expect("query")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("collect");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (
                "llm".to_string(),
                "run_cargo".to_string(),
                4_200,
                Some("req-1".to_string())
            )
        );
        assert_eq!(
            rows[1],
            ("gate".to_string(), "cargo".to_string(), 12_500, None)
        );
    }

    #[test]
    fn record_bug_insert_or_replace_keeps_latest_snapshot() {
        let db = GlobalDb::open_in_memory().expect("open");
        let project_dir = std::env::temp_dir();
        let mut bug = crate::__internal::bug_log::BugRecord {
            id: "bug-001".to_string(),
            opened_at: "1700000000".to_string(),
            closed_at: None,
            step: "DM2d".to_string(),
            milestone: None,
            category: "wire_up".to_string(),
            issue: "port-name typo".to_string(),
            events: Vec::new(),
            resolution: None,
            status: "open".to_string(),
        };
        db.record_bug(&project_dir, &bug).expect("first write");

        // Mutate (resolve) and re-mirror; the unique key on (project_dir,
        // bug_id) must keep exactly one row carrying the latest state.
        bug.status = "resolved".to_string();
        bug.closed_at = Some("1700000010".to_string());
        bug.resolution = Some("renamed port; gate green".to_string());
        db.record_bug(&project_dir, &bug).expect("second write");

        let row_count: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM bugs WHERE bug_id = ?1",
                params!["bug-001"],
                |row| row.get(0),
            )
            .expect("count rows");
        assert_eq!(row_count, 1, "INSERT OR REPLACE should keep one row");
        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM bugs WHERE bug_id = ?1",
                params!["bug-001"],
                |row| row.get(0),
            )
            .expect("status");
        assert_eq!(status, "resolved", "latest snapshot must win");
    }

    // ─── Test helpers ─────────────────────────────────────────────────

    fn tempdir() -> TempDir {
        let path =
            std::env::temp_dir().join(format!("sim-flow-global-db-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create tempdir");
        TempDir { path }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
