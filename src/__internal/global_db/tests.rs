use rusqlite::params;

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

    let (req, turn, step, kind, backend, wall_ms): (String, i64, String, String, String, i64) = db
        .conn()
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
    assert_eq!(
        rows[0][0],
        serde_json::Value::String("compile_error".into())
    );
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
        .prepare("SELECT caller_kind, tool_name, wall_ms, request_id FROM tool_timings ORDER BY id")
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
