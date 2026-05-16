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
//!
//! Submodules group the implementation:
//!   - [`schema`] -- DDL + meta bootstrap + machine-id provisioning
//!   - [`writers`] -- per-source mirror writers (record_*)
//!   - [`queries`] -- count / latest_timestamp / read-only query / backfill offsets
//!   - [`util`] -- shared error / encoding / identifier helpers

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use directories::ProjectDirs;
use rusqlite::Connection;

use crate::{Error, Result};

mod queries;
mod schema;
mod util;
mod writers;

#[cfg(test)]
mod tests;

pub use schema::SCHEMA_VERSION;

use schema::{apply_schema, ensure_machine_id};
use util::wrap_sqlite;

pub const SIM_FLOW_DB: &str = "sim-flow.db";

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
    pub(super) conn: Connection,
    pub(super) machine_id: String,
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
        use rusqlite::{OptionalExtension, params};
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![schema::META_KEY_SCHEMA_VERSION],
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
