//! Per-invocation tool timings for both LLM-driven tool calls and
//! gate-driven shell checks. Mirrors [`llm_metrics`](super::llm_metrics)
//! in shape and lifecycle:
//!
//! - Append-only JSONL at `<proj>/.sim-flow/logs/tool-timings.jsonl`.
//! - Process-lifetime `ToolTimingsLog` opens the file lazily on first
//!   `record`; a session that runs zero tools leaves no empty file.
//! - Every `record` call also fires the best-effort mirror to the
//!   per-user global DB via [`global_db::with_db`].
//!
//! Closes the "where is wall-clock time actually going?" gap that's
//! otherwise invisible: `llm_metrics` shows time spent waiting on the
//! model, but a turn that calls `run_cargo` and waits 4 minutes for
//! the build to finish currently folds those 4 minutes into the
//! agent's wall time with no breakdown. Gate-passing time during
//! step advance is the same dark spot. The schema's `caller_kind`
//! discriminator (`"llm"` / `"gate"`) lets reports separate the
//! agent-driven cost from the orchestrator-driven cost.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const TIMINGS_FILE: &str = "tool-timings.jsonl";

/// Whether the invocation was triggered by the LLM (via the
/// orchestrator's tool dispatcher) or by the gate machinery (a
/// `GateCheck::Shell` invocation during step evaluation).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CallerKind {
    Llm,
    Gate,
}

impl CallerKind {
    pub fn as_column_str(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Gate => "gate",
        }
    }
}

/// One row appended to `tool-timings.jsonl`. Stable wire shape -- new
/// fields must be `Option` or have a serde default so older readers
/// stay parseable. Fields are deliberately flat (no nested objects)
/// so jq / awk can pull individual columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTimingRecord {
    /// Unix epoch seconds when the tool invocation STARTED.
    pub started_unix: u64,
    /// Project step id at the time of the invocation. `None` for
    /// invocations outside the orchestrator's per-step session loop
    /// (rare; only the bootstrap path can hit this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// LLM-driven (orchestrator's tool dispatcher) vs gate-driven
    /// (`GateCheck::Shell`).
    pub caller_kind: CallerKind,
    /// Name of the tool / shell command. For LLM tools, this is the
    /// tool slug (`run_cargo`, `write_file`, ...). For gate shells,
    /// this is the binary name (`cargo`, `grep`, `sh`, ...).
    pub tool_name: String,
    /// Best-effort short summary of the args. For LLM tools, the
    /// orchestrator's existing `tool_args_summary` formatter. For
    /// gate shells, the joined `args` slice.
    #[serde(default)]
    pub args_summary: String,
    /// `"ok"` for success, `"error"` for failure, or any other tool-
    /// specific status string.
    pub status: String,
    /// Wall-clock milliseconds spent inside the tool / shell.
    pub wall_ms: u64,
    /// Process exit code when the tool was a subprocess (gate
    /// shells, `run_cargo`, ...). `None` for tools that didn't shell
    /// out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// LLM request_id when this invocation happened during a
    /// tool-call turn. `None` for gate-driven invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// LLM turn index when this invocation happened during a
    /// tool-call turn. `None` for gate-driven invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u32>,
}

/// Append-only writer keyed to a project's tool-timings file. Same
/// lazy-open / best-effort failure semantics as
/// [`super::llm_metrics::LlmMetricsLog`].
pub struct ToolTimingsLog {
    project_dir: PathBuf,
    path: PathBuf,
    handle: Mutex<Option<File>>,
}

impl ToolTimingsLog {
    /// Construct the logger for a given project's `.sim-flow/` dir.
    /// Does not touch the filesystem.
    pub fn for_project(project_dir: &Path) -> Self {
        let path = project_dir
            .join(".sim-flow")
            .join("logs")
            .join(TIMINGS_FILE);
        Self {
            project_dir: project_dir.to_path_buf(),
            path,
            handle: Mutex::new(None),
        }
    }

    /// Append one record. The first call creates
    /// `<project>/.sim-flow/logs/` and the file if either is missing.
    /// Filesystem errors are suppressed (a missed timing row never
    /// aborts a real session); the failure is logged at WARN via
    /// `tracing` so it surfaces in the debug log when enabled.
    pub fn record(&self, rec: &ToolTimingRecord) {
        let line = match serde_json::to_string(rec) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    target: "sim_flow::tool_timings",
                    error = %err,
                    "failed to serialize tool timing record"
                );
                return;
            }
        };
        let mut guard = match self.handle.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            if let Some(parent) = self.path.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(
                    target: "sim_flow::tool_timings",
                    error = %err,
                    "could not create tool-timings directory"
                );
                return;
            }
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => *guard = Some(f),
                Err(err) => {
                    tracing::warn!(
                        target: "sim_flow::tool_timings",
                        error = %err,
                        path = %self.path.display(),
                        "could not open tool-timings file"
                    );
                    return;
                }
            }
        }
        let file = guard.as_mut().expect("opened above");
        if let Err(err) = writeln!(file, "{line}") {
            tracing::warn!(
                target: "sim_flow::tool_timings",
                error = %err,
                "tool-timings write failed"
            );
        }
        // Best-effort mirror to the per-user global DB. Failure logs a
        // `tracing::warn!` inside `with_db` and never aborts the caller
        // -- the project-local JSONL is authoritative.
        let _ = crate::__internal::global_db::with_db(|db| {
            db.record_tool_timing(&self.project_dir, rec)
        });
    }
}

/// Convenience for the now-unix seconds the writer stamps on records.
pub fn now_unix_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_one_line_per_call() {
        let tmp = tempfile::tempdir().unwrap();
        let log = ToolTimingsLog::for_project(tmp.path());
        for i in 0..3 {
            log.record(&ToolTimingRecord {
                started_unix: 1_700_000_000 + i,
                step: Some("DM0".to_string()),
                caller_kind: CallerKind::Llm,
                tool_name: format!("tool_{i}"),
                args_summary: format!("arg_{i}"),
                status: "ok".to_string(),
                wall_ms: 100,
                exit_code: None,
                request_id: Some(format!("req-{i}")),
                turn_index: Some(i as u32 + 1),
            });
        }
        let body =
            std::fs::read_to_string(tmp.path().join(".sim-flow/logs").join(TIMINGS_FILE)).unwrap();
        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got {body:?}");
        for line in &lines {
            // Round-trips back through serde -- the on-disk shape is
            // exactly what we ship in the global DB's record_json.
            let parsed: ToolTimingRecord = serde_json::from_str(line).expect("parse");
            assert_eq!(parsed.status, "ok");
        }
    }

    #[test]
    fn caller_kind_serializes_lowercase() {
        let llm = serde_json::to_string(&CallerKind::Llm).unwrap();
        let gate = serde_json::to_string(&CallerKind::Gate).unwrap();
        assert_eq!(llm, "\"llm\"");
        assert_eq!(gate, "\"gate\"");
    }
}
