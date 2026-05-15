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
        -- sources ("llm" / "gate"); unique-key includes a sequence so
        -- repeated invocations of the same tool inside one turn don't
        -- collide.
        CREATE TABLE IF NOT EXISTS tool_timings (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            project_dir     TEXT NOT NULL,
            request_id      TEXT,
            turn_index      INTEGER,
            invocation_seq  INTEGER NOT NULL,
            timestamp       TEXT NOT NULL,
            step            TEXT,
            caller_kind     TEXT NOT NULL,
            tool_name       TEXT NOT NULL,
            wall_ms         INTEGER NOT NULL,
            exit_code       INTEGER,
            user_identity   TEXT NOT NULL DEFAULT '',
            machine_id      TEXT NOT NULL DEFAULT '',
            record_json     TEXT NOT NULL,
            UNIQUE(project_dir, request_id, turn_index, invocation_seq, tool_name)
        );
        CREATE INDEX IF NOT EXISTS idx_tool_timings_step        ON tool_timings(step);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_caller_kind ON tool_timings(caller_kind);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_tool_name   ON tool_timings(tool_name);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_timestamp   ON tool_timings(timestamp);
        CREATE INDEX IF NOT EXISTS idx_tool_timings_user        ON tool_timings(user_identity);
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
