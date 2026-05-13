//! Project-local bug log.
//!
//! Each sim-flow project gets a `.sim-flow/bug-log.jsonl` file that
//! captures every bug the agent investigates in the course of
//! running the flow: what was wrong, what hypotheses the agent
//! tried, which fix attempts it ran, and what the final resolution
//! was. The log is append-only across sessions and across flows so
//! a project's history accumulates over time -- this is the raw
//! data the operator mines for systemic issues ("agent keeps
//! tripping on tarpaulin coverage" -> migrate to llvm-cov; "DM3c
//! stress tests always fail with 0.5/cycle" -> document the
//! framework's tick contract).
//!
//! Storage shape: one JSON record per line, schema documented on
//! [`BugRecord`]. The orchestrator maintains an in-memory "open
//! bug stack" (most-recently-opened bug is the implicit target for
//! `declare_hypothesis` / `declare_fix` / `resolve_bug` calls);
//! that stack is seeded at session start from the on-disk log.
//!
//! Best-effort everywhere: I/O failures don't abort an LLM turn,
//! they just drop the log update. The bug log is metadata; the
//! authoritative state is the project's gate flags + critique
//! files + source artifacts.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One bug entry. Mutated in place as the agent appends events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugRecord {
    /// Stable id of the form `bug-NNN`, monotonically allocated
    /// across the project's history. NOT reused after resolution.
    pub id: String,
    /// ISO-8601 timestamp the bug was opened.
    pub opened_at: String,
    /// ISO-8601 timestamp the bug was resolved; `None` while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// Step id at the time of opening (e.g. `DM3c`).
    pub step: String,
    /// Specific milestone the bug surfaced under, when applicable
    /// (e.g. `test-milestone-03-stress.md`). `None` for non-walk
    /// steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// Coarse-grained classification. Operator-tunable taxonomy;
    /// the orchestrator only validates that something was passed.
    pub category: String,
    /// One- or two-sentence summary of what's wrong.
    pub issue: String,
    /// Lifecycle events appended over the bug's lifetime.
    #[serde(default)]
    pub events: Vec<BugEvent>,
    /// Final resolution narrative; `None` while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// `open` | `resolved` | `manual`. `manual` is set when the
    /// auto loop bails and the operator takes over; the bug stays
    /// in the log so the trail isn't lost.
    pub status: String,
}

/// One event in a bug's lifecycle. Variant via `kind`; the rest of
/// the fields are interpreted per-kind. Untagged so the jsonl is
/// easy to read with `jq`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugEvent {
    pub ts: String,
    pub kind: String, // "hypothesis" | "fix_attempt" | "expectation_nudge" | "note"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn bug_log_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".sim-flow").join("bug-log.jsonl")
}

/// Load every bug record from the project's log. Returns an empty
/// `Vec` when the log doesn't exist yet (first bug of the project
/// will create it). Records that fail to parse are skipped with a
/// warning to stderr; we never let a corrupt line abort the load
/// because the log accumulates across many sessions and a bad
/// write from a partial flush shouldn't lock new bugs out.
pub fn load_all(project_dir: &Path) -> Vec<BugRecord> {
    let path = bug_log_path(project_dir);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<BugRecord>(trimmed) {
            Ok(rec) => out.push(rec),
            Err(err) => {
                eprintln!(
                    "sim-flow: bug-log.jsonl line {} skipped (parse error: {err})",
                    i + 1,
                );
            }
        }
    }
    out
}

/// Persist the full set of records back to disk, replacing the
/// file contents atomically. Called after any append / mutate so
/// the on-disk view always matches the in-memory state. Failures
/// are surfaced via `Result` so the caller can decide whether to
/// swallow them.
pub fn save_all(project_dir: &Path, records: &[BugRecord]) -> std::io::Result<()> {
    let path = bug_log_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    for rec in records {
        let line = serde_json::to_string(rec).map_err(std::io::Error::other)?;
        writeln!(file, "{line}")?;
    }
    file.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Allocate the next `bug-NNN` id given the existing record set.
/// Scans for the maximum numeric suffix and returns max+1. Returns
/// `bug-001` for an empty set.
pub fn next_id(records: &[BugRecord]) -> String {
    let max = records
        .iter()
        .filter_map(|r| r.id.strip_prefix("bug-"))
        .filter_map(|s| s.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("bug-{:03}", max + 1)
}

/// Convenience: append a new open bug to the project's log,
/// allocate an id, save. Returns the new id on success.
pub fn open(
    project_dir: &Path,
    step: &str,
    milestone: Option<&str>,
    category: &str,
    issue: &str,
) -> std::io::Result<String> {
    let mut records = load_all(project_dir);
    let id = next_id(&records);
    let now = now_iso();
    records.push(BugRecord {
        id: id.clone(),
        opened_at: now,
        closed_at: None,
        step: step.to_string(),
        milestone: milestone.map(String::from),
        category: category.to_string(),
        issue: issue.to_string(),
        events: Vec::new(),
        resolution: None,
        status: "open".to_string(),
    });
    save_all(project_dir, &records)?;
    Ok(id)
}

/// Append an event to the bug with `id`. No-op (returns Ok) when
/// the bug is missing or already resolved -- the caller is a
/// best-effort logger, not a state machine.
pub fn append_event(project_dir: &Path, id: &str, event: BugEvent) -> std::io::Result<()> {
    let mut records = load_all(project_dir);
    if let Some(rec) = records.iter_mut().find(|r| r.id == id)
        && rec.status == "open"
    {
        rec.events.push(event);
        return save_all(project_dir, &records);
    }
    Ok(())
}

/// Close a bug with a resolution narrative. Sets `status` to
/// `"resolved"` (or `"manual"` when `status_override` is set --
/// used by the auto-loop bail path to record that the operator
/// took over). No-op when the bug isn't found.
pub fn resolve(
    project_dir: &Path,
    id: &str,
    resolution: &str,
    status_override: Option<&str>,
) -> std::io::Result<()> {
    let mut records = load_all(project_dir);
    if let Some(rec) = records.iter_mut().find(|r| r.id == id) {
        rec.status = status_override.unwrap_or("resolved").to_string();
        rec.closed_at = Some(now_iso());
        rec.resolution = Some(resolution.to_string());
        return save_all(project_dir, &records);
    }
    Ok(())
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Minimal ISO-8601 form; we don't pull in chrono for one
    // timestamp. The orchestrator's existing
    // `protocol::default_timestamp` uses the same shape so logs
    // line up across files.
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_record_and_assigns_id() {
        let tmp = tempfile::tempdir().unwrap();
        let id = open(
            tmp.path(),
            "DM3c",
            Some("test-milestone-03-stress.md"),
            "framework",
            "stress tests fail at 0.5/cycle",
        )
        .unwrap();
        assert_eq!(id, "bug-001");
        let records = load_all(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].step, "DM3c");
        assert_eq!(records[0].status, "open");
        assert!(records[0].closed_at.is_none());
    }

    #[test]
    fn next_id_increments_across_records() {
        let tmp = tempfile::tempdir().unwrap();
        let a = open(tmp.path(), "DM3c", None, "test", "first").unwrap();
        let b = open(tmp.path(), "DM3c", None, "test", "second").unwrap();
        let c = open(tmp.path(), "DM3c", None, "test", "third").unwrap();
        assert_eq!(a, "bug-001");
        assert_eq!(b, "bug-002");
        assert_eq!(c, "bug-003");
    }

    #[test]
    fn append_event_records_hypothesis_and_fix_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let id = open(tmp.path(), "DM3c", None, "framework", "x").unwrap();
        append_event(
            tmp.path(),
            &id,
            BugEvent {
                ts: "1".into(),
                kind: "hypothesis".into(),
                rationale: Some("framework halves throughput".into()),
                outcome: None,
                message: None,
            },
        )
        .unwrap();
        append_event(
            tmp.path(),
            &id,
            BugEvent {
                ts: "2".into(),
                kind: "fix_attempt".into(),
                rationale: Some("bumped injector rate".into()),
                outcome: Some("failed".into()),
                message: None,
            },
        )
        .unwrap();
        let records = load_all(tmp.path());
        assert_eq!(records[0].events.len(), 2);
        assert_eq!(records[0].events[0].kind, "hypothesis");
        assert_eq!(records[0].events[1].kind, "fix_attempt");
    }

    #[test]
    fn append_event_to_resolved_bug_is_silently_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let id = open(tmp.path(), "DM3c", None, "framework", "x").unwrap();
        resolve(tmp.path(), &id, "fixed by raising rate", None).unwrap();
        append_event(
            tmp.path(),
            &id,
            BugEvent {
                ts: "1".into(),
                kind: "hypothesis".into(),
                rationale: Some("oops too late".into()),
                outcome: None,
                message: None,
            },
        )
        .unwrap();
        let records = load_all(tmp.path());
        assert_eq!(records[0].status, "resolved");
        assert!(records[0].events.is_empty());
    }

    #[test]
    fn resolve_sets_status_and_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let id = open(tmp.path(), "DM3c", None, "framework", "x").unwrap();
        resolve(tmp.path(), &id, "raised injector to 1/cycle", None).unwrap();
        let records = load_all(tmp.path());
        assert_eq!(records[0].status, "resolved");
        assert!(records[0].closed_at.is_some());
        assert_eq!(
            records[0].resolution.as_deref(),
            Some("raised injector to 1/cycle")
        );
    }

    #[test]
    fn manual_status_override_preserves_trail() {
        let tmp = tempfile::tempdir().unwrap();
        let id = open(tmp.path(), "DM3c", None, "framework", "x").unwrap();
        resolve(
            tmp.path(),
            &id,
            "auto bailed; operator to investigate",
            Some("manual"),
        )
        .unwrap();
        let records = load_all(tmp.path());
        assert_eq!(records[0].status, "manual");
    }

    #[test]
    fn load_skips_corrupt_lines_without_aborting() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        std::fs::write(
            tmp.path().join(".sim-flow/bug-log.jsonl"),
            r#"{"id":"bug-001","opened_at":"1","step":"DM3c","category":"x","issue":"y","status":"open"}
not-valid-json
{"id":"bug-002","opened_at":"2","step":"DM3c","category":"x","issue":"z","status":"open"}
"#,
        )
        .unwrap();
        let records = load_all(tmp.path());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "bug-001");
        assert_eq!(records[1].id, "bug-002");
    }
}
