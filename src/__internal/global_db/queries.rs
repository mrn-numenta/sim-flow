//! Read-side and `meta`-table operations: row counts, latest
//! timestamps, the CLI `db query` entry point, and the backfill
//! offset getter/setter. Kept distinct from `writers.rs` so the
//! shape of one side doesn't bleed into the other when adding
//! columns.

use std::path::Path;

use rusqlite::{OptionalExtension, params};

use crate::{Error, Result};

use super::GlobalDb;
use super::util::{backfill_offset_key, base64_encode_bytes, is_safe_table_name, wrap_sqlite};

impl GlobalDb {
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
            let columns: Vec<String> = stmt
                .column_names()
                .iter()
                .map(|s| (*s).to_string())
                .collect();
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
}
