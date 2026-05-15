//! Per-LLM-request structured metrics emitter.
//!
//! Appends one JSON object per `RequestLlmResponse` round-trip to
//! `<project>/.sim-flow/logs/llm-metrics.jsonl`. The orchestrator
//! calls [`LlmMetricsLog::record`] from its turn-end handler with
//! whatever it knows (step, kind, backend/model, wall time, byte
//! counts, finish reason) -- no protocol change needed.
//!
//! Token counts are *byte-based estimates* by default: the
//! orchestrator's host-mediated path (extension / external host) does
//! not see the model server's `usage` payload, so we approximate by
//! `bytes / 4` for both prompt and completion. Backends that have
//! exact counts (in-process `TerminalHost` agents that record
//! `LlmCallMetrics`) can pass them in explicitly via the `tokens_in`
//! / `tokens_out` fields on the [`LlmMetricsRecord`]; that path is
//! wired today only for the in-process flow.
//!
//! The file is JSONL so it's grep / jq / pandas friendly without a
//! schema migration step. `sim-flow metrics` (a separate CLI
//! subcommand) reads it back and renders aggregates.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::session::protocol::SessionKindOut;

const METRICS_FILE: &str = "llm-metrics.jsonl";
/// Conservative byte-to-token heuristic used when an exact `usage`
/// payload is unavailable. The 4 bytes/token ratio is a long-running
/// rule of thumb for English text + structured prompts; tighter than
/// 3 (which over-counts) and looser than 5 (which under-counts on
/// code-heavy turns). Real usage payloads, when available, override
/// the estimate.
const BYTES_PER_TOKEN_EST: f64 = 4.0;

/// One row appended to `llm-metrics.jsonl`. Stable wire shape -- new
/// fields must be `Option` or have a serde default so older
/// readers stay parseable. Fields are deliberately flat (no nested
/// objects) so jq / awk can pull individual columns without
/// flattening boilerplate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMetricsRecord {
    /// Unix epoch seconds when the LLM turn STARTED (caller measures
    /// elapsed via `wall_ms` field below). UTC; integer precision
    /// is fine for analytics, sub-second timing lives in `wall_ms`.
    pub started_unix: u64,
    /// Project step id, e.g. `DM0`, `DM2cd`.
    pub step: String,
    /// Sub-session kind. Serializes as `work` / `critique` / `qa`.
    pub kind: SessionKindOut,
    /// Backend selector the orchestrator handed to the host
    /// (`vllm`, `anthropic`, `ollama`, `openai-compat`, ...). With
    /// per-kind routing enabled this differs across rows in the
    /// same step.
    pub backend: String,
    /// Model identifier handed to the backend (when set). `None`
    /// when the backend uses a server-default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-session unique request id, matches the
    /// `RequestLlmResponse.request_id` on the protocol wire.
    pub request_id: String,
    /// 1-based turn index within the current sub-session. Useful
    /// for spotting "turn 1 is always slow" prefill patterns.
    pub turn_index: u32,
    /// Wall-clock milliseconds the orchestrator spent waiting for
    /// the host's `LlmEnd` after emitting `RequestLlmResponse`.
    /// Includes streaming + tool-call assembly + protocol
    /// round-trip; NOT pure model compute (use the host's own
    /// metrics for that).
    pub wall_ms: u64,
    /// `LlmEnd.stop_reason` verbatim when the turn ended cleanly;
    /// `"error"` when an `LlmError` came back instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Total bytes in the prompt sent to the model this turn (sum
    /// of serialized message content, excluding the small JSON
    /// envelope around each message). The orchestrator measures
    /// this on the request side so the figure is exact regardless
    /// of which host runs the dispatch.
    pub prompt_bytes: u64,
    /// Total bytes in the assistant's response this turn (UTF-8
    /// length of the streamed text).
    pub completion_bytes: u64,
    /// Estimated tokens in (bytes / 4). Replaced by the model
    /// server's `usage.prompt_tokens` when available.
    pub tokens_in: u64,
    /// Estimated tokens out (bytes / 4). Replaced by the model
    /// server's `usage.completion_tokens` when available.
    pub tokens_out: u64,
    /// True when the model server returned exact `usage` counts.
    /// False when `tokens_in` / `tokens_out` are byte-based
    /// estimates. Lets a viewer distinguish "this row is
    /// approximate" from "this row is the server's truth."
    pub tokens_exact: bool,
}

impl LlmMetricsRecord {
    /// Byte-estimated record. Use when the host doesn't know exact
    /// token counts (the JSONL / socket transport carries no
    /// `usage` payload). For backends that DO know exact counts,
    /// prefer the `with_exact_usage` chainer below.
    #[allow(clippy::too_many_arguments)]
    pub fn from_byte_estimate(
        started_unix: u64,
        step: &str,
        kind: SessionKindOut,
        backend: &str,
        model: Option<&str>,
        request_id: &str,
        turn_index: u32,
        wall_ms: u64,
        finish_reason: Option<&str>,
        prompt_bytes: u64,
        completion_bytes: u64,
    ) -> Self {
        Self {
            started_unix,
            step: step.to_string(),
            kind,
            backend: backend.to_string(),
            model: model.map(String::from),
            request_id: request_id.to_string(),
            turn_index,
            wall_ms,
            finish_reason: finish_reason.map(String::from),
            prompt_bytes,
            completion_bytes,
            tokens_in: (prompt_bytes as f64 / BYTES_PER_TOKEN_EST).round() as u64,
            tokens_out: (completion_bytes as f64 / BYTES_PER_TOKEN_EST).round() as u64,
            tokens_exact: false,
        }
    }

    /// Override the byte-based token estimate with exact counts
    /// from the model server's `usage` payload. Called when the
    /// host attached `LlmEnd.usage`. `prompt_bytes` /
    /// `completion_bytes` are kept on the row because they're
    /// useful for spotting tokenizer pathologies (e.g. a 50k-byte
    /// prompt that costs 200k tokens points at a bad
    /// `chat_template_kwargs.enable_thinking` config or a
    /// rendering bug); the `tokens_*` fields become the
    /// authoritative cost number with `tokens_exact: true`.
    pub fn with_exact_usage(mut self, prompt_tokens: u64, completion_tokens: u64) -> Self {
        self.tokens_in = prompt_tokens;
        self.tokens_out = completion_tokens;
        self.tokens_exact = true;
        self
    }
}

/// Append-only writer keyed to a project's metrics file. The writer
/// opens the file lazily on the first `record` call so a session
/// that emits zero turns (e.g. one that fails the handshake) leaves
/// no empty file behind.
pub struct LlmMetricsLog {
    project_dir: PathBuf,
    path: PathBuf,
    handle: Mutex<Option<File>>,
}

impl LlmMetricsLog {
    /// Construct the logger for a given project's `.sim-flow/` dir.
    /// Does not touch the filesystem.
    pub fn for_project(project_dir: &Path) -> Self {
        let path = project_dir
            .join(".sim-flow")
            .join("logs")
            .join(METRICS_FILE);
        Self {
            project_dir: project_dir.to_path_buf(),
            path,
            handle: Mutex::new(None),
        }
    }

    /// Append one record. The first call creates `<project>/.sim-flow/logs/`
    /// and the file if either is missing. Filesystem errors are
    /// suppressed (we'd rather lose a metrics row than abort a real
    /// session); the failure is logged at WARN via `tracing` so it
    /// surfaces in the debug log when enabled.
    pub fn record(&self, rec: &LlmMetricsRecord) {
        let line = match serde_json::to_string(rec) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(target: "sim_flow::metrics", error = %err, "failed to serialize llm metrics record");
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
                tracing::warn!(target: "sim_flow::metrics", error = %err, "could not create llm-metrics directory");
                return;
            }
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => *guard = Some(f),
                Err(err) => {
                    tracing::warn!(target: "sim_flow::metrics", error = %err, path = %self.path.display(), "could not open llm-metrics file");
                    return;
                }
            }
        }
        let file = guard.as_mut().expect("opened above");
        if let Err(err) = writeln!(file, "{line}") {
            tracing::warn!(target: "sim_flow::metrics", error = %err, "llm-metrics write failed");
        }
        // Best-effort mirror to the per-user global DB. Failure logs a
        // `tracing::warn!` inside `with_db` and never aborts the caller
        // -- the project-local JSONL is authoritative.
        let _ = crate::__internal::global_db::with_db(|db| {
            db.record_llm_metric(&self.project_dir, rec)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_record_roundtrips_serde() {
        let rec = LlmMetricsRecord::from_byte_estimate(
            1700000000,
            "DM0",
            SessionKindOut::Work,
            "vllm",
            Some("qwen3.6"),
            "lr-1",
            3,
            12_500,
            Some("stop"),
            4096,
            2048,
        );
        assert_eq!(rec.tokens_in, 1024);
        assert_eq!(rec.tokens_out, 512);
        assert!(!rec.tokens_exact);
        let line = serde_json::to_string(&rec).unwrap();
        let parsed: LlmMetricsRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.step, "DM0");
        assert_eq!(parsed.tokens_in, 1024);
    }

    #[test]
    fn record_appends_one_line_per_call() {
        let tmp = tempfile::tempdir().unwrap();
        let log = LlmMetricsLog::for_project(tmp.path());
        for i in 0..3 {
            log.record(&LlmMetricsRecord::from_byte_estimate(
                1700000000 + i,
                "DM0",
                SessionKindOut::Work,
                "vllm",
                Some("qwen3.6"),
                &format!("lr-{i}"),
                i as u32 + 1,
                100,
                Some("stop"),
                200,
                100,
            ));
        }
        let body = std::fs::read_to_string(tmp.path().join(".sim-flow/logs").join(METRICS_FILE))
            .expect("metrics file exists");
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            let _: LlmMetricsRecord = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn record_with_missing_parent_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        // No `.sim-flow/` yet.
        let log = LlmMetricsLog::for_project(tmp.path());
        log.record(&LlmMetricsRecord::from_byte_estimate(
            1700000000,
            "DM0",
            SessionKindOut::Critique,
            "anthropic",
            Some("claude-3-5-sonnet-latest"),
            "lr-1",
            1,
            500,
            Some("stop"),
            50,
            10,
        ));
        assert!(
            tmp.path()
                .join(".sim-flow/logs")
                .join(METRICS_FILE)
                .exists()
        );
    }
}
