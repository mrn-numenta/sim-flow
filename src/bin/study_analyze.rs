//! Analyzer for the model-robustness study captures.
//!
//! See `tools/sim-flow/docs/brainstorming/model-robustness-study.md`.
//!
//! Walks a study root of the shape produced by
//! `scripts/run-robustness-study.sh`:
//!
//! ```text
//! <study-root>/
//!   <model-slug>/
//!     trials.jsonl
//!     trial-NN/
//!       protocol.jsonl    <-- per-trial JSONL capture
//!       ...
//! ```
//!
//! For each `protocol.jsonl`, replays the event stream and detects
//! the anomaly kinds enumerated in the brainstorming doc that have
//! unambiguous protocol signatures (most failure caps live in
//! `Diagnostic` messages with stable substrings; truncations show
//! up as `llm-error`; runaway-loop trips emit a specific
//! diagnostic and SessionEnd reason). Writes:
//!
//! - `<trial-dir>/anomalies.jsonl`    one event per detected anomaly
//! - `<trial-dir>/summary.json`       per-trial roll-up
//! - `<study-root>/summary.json`      aggregate across all trials + models
//! - `<study-root>/summary.md`        human-readable rendering of above
//!
//! Detection is deliberately conservative: anomalies that need
//! cross-turn drift analysis or semantic file inspection (e.g.
//! `milestone-rows-flipped-early`, `wrong-step-critique`) are left
//! out of v1. The diagnostic-substring matches catch the cap
//! firings + transport errors, which is where the Phase 0/0b/0c
//! signal landed.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn main() {
    let args: Args = match Args::parse(std::env::args().collect()) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Err(err) = run(&args) {
        eprintln!("study_analyze: {err}");
        std::process::exit(1);
    }
}

struct Args {
    study_root: PathBuf,
}

impl Args {
    fn parse(argv: Vec<String>) -> std::result::Result<Self, String> {
        let mut study_root: Option<PathBuf> = None;
        let mut iter = argv.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--study-root" => study_root = iter.next().map(PathBuf::from),
                "--help" | "-h" => {
                    println!(
                        "usage: study_analyze --study-root <PATH>\n\
                         \n\
                         Walks <PATH> looking for `<model-slug>/trial-*/protocol.jsonl` \
                         captures produced by `scripts/run-robustness-study.sh`. Emits \
                         per-trial `anomalies.jsonl` + `summary.json` next to each \
                         capture, and an aggregate `summary.json` + `summary.md` at \
                         the study root."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
        }
        Ok(Self {
            study_root: study_root.ok_or_else(|| "--study-root is required".to_string())?,
        })
    }
}

fn run(args: &Args) -> Result<(), String> {
    if !args.study_root.is_dir() {
        return Err(format!(
            "study root {} is not a directory",
            args.study_root.display()
        ));
    }
    // Find every protocol.jsonl at depth >= 2 under the study root
    // (`<model>/trial-NN/protocol.jsonl`). Some study roots in the
    // wild also nest under a job dir, so we walk shallowly: up to
    // 3 levels of directories above the file.
    let captures = find_captures(&args.study_root);
    if captures.is_empty() {
        return Err(format!(
            "no protocol.jsonl files found under {}",
            args.study_root.display()
        ));
    }
    eprintln!(
        "study_analyze: {} capture(s) under {}",
        captures.len(),
        args.study_root.display()
    );

    let mut trial_summaries: Vec<TrialSummary> = Vec::new();
    for capture in &captures {
        match analyze_capture(capture) {
            Ok(summary) => {
                if let Err(err) = write_trial_outputs(capture, &summary) {
                    eprintln!(
                        "  {}: write per-trial outputs failed: {err}",
                        capture.display()
                    );
                }
                trial_summaries.push(summary);
            }
            Err(err) => {
                eprintln!("  {}: SKIPPED ({err})", capture.display());
            }
        }
    }

    let study = StudySummary::from_trials(&trial_summaries);
    write_study_outputs(&args.study_root, &study)?;

    println!(
        "study_analyze: {} trials analyzed across {} model(s); see {}",
        trial_summaries.len(),
        study.per_model.len(),
        args.study_root.join("summary.md").display()
    );
    Ok(())
}

fn find_captures(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    walk(root, 0, 4, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, max_depth, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("protocol.jsonl") {
            out.push(path);
        }
    }
}

// -------------------------------------------------------------------
// Per-trial analysis.
// -------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialSummary {
    /// Absolute path to the protocol.jsonl this summary describes.
    pub capture: PathBuf,
    /// Model id from the run-start meta line.
    pub model: String,
    /// Backend label (`openai-compat`, `claude`, `anthropic`, ...).
    pub backend: Option<String>,
    /// Trial seed from the run-start meta (when the driver set
    /// `SIM_FLOW_SEED`).
    pub seed: Option<u32>,
    /// Wall clock from the run-end meta if present (milliseconds).
    pub wall_ms: Option<u64>,
    /// Last step the orchestrator advanced into (`DM0`, `DM2cd`, ...);
    /// `None` when no advance happened.
    pub last_advance: Option<String>,
    /// Classification of the terminator (which cap / error stopped
    /// the run). `None` if the trial completed cleanly.
    pub terminator: Option<TerminatorKind>,
    /// Per-anomaly-kind counts.
    pub anomalies: BTreeMap<String, u32>,
    /// Total `request-llm-response` events (one per LLM dispatch).
    pub n_llm_requests: u32,
    /// Total `artifact-written` events.
    pub n_artifact_writes: u32,
    /// Total `tool-invoked` events.
    pub n_tool_invocations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminatorKind {
    /// Work session burned `max_auto_iters` consecutive turns
    /// without producing any fenced artifact-write block. The
    /// canonical "model is reading + considering without
    /// committing" pattern; the post-Phase-0c fence-fix work
    /// targets this. Orchestrator message:
    /// `"... exceeded max_auto_iters (N) without producing an
    /// artifact ..."`.
    WorkNoArtifact,
    /// Work session DID produce artifacts but the structural
    /// gate stayed dirty across `max_auto_iters` retries. Distinct
    /// from `WorkNoArtifact`: the model is writing, just not
    /// writing what the gate wants. Orchestrator message:
    /// `"... exceeded max_auto_iters (N); switching to interactive.
    /// Last gate failures: ..."`. Often surfaces on milestone-walk
    /// steps where the task rows stay `- [ ]` even though source
    /// files landed.
    WorkGateStillDirty,
    CritiqueIterCap,
    CritiqueNoProgress,
    CargoTestNoProgress,
    RunawayLoop,
    PhaseIterCap,
    HostClosed,
    Other,
}

impl TerminatorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkNoArtifact => "work-no-artifact",
            Self::WorkGateStillDirty => "work-gate-still-dirty",
            Self::CritiqueIterCap => "critique-iter-cap",
            Self::CritiqueNoProgress => "critique-no-progress",
            Self::CargoTestNoProgress => "cargo-test-no-progress",
            Self::RunawayLoop => "runaway-loop",
            Self::PhaseIterCap => "phase-iter-cap",
            Self::HostClosed => "host-closed",
            Self::Other => "other",
        }
    }
}

/// One anomaly record persisted into `<trial-dir>/anomalies.jsonl`.
#[derive(Debug, Clone, Serialize)]
struct AnomalyRecord {
    kind: &'static str,
    /// Best-effort step id (when the message includes one).
    step: Option<String>,
    /// Original event timestamp (unix ms, from the capture envelope).
    ts_ms: u64,
    /// Truncated snippet of the matched event so a human can scan
    /// the file without consulting the source protocol.jsonl.
    snippet: String,
}

fn analyze_capture(path: &Path) -> Result<TrialSummary, String> {
    let file = File::open(path).map_err(|err| format!("open: {err}"))?;
    let reader = BufReader::new(file);
    let mut model = String::new();
    let mut backend: Option<String> = None;
    let mut seed: Option<u32> = None;
    let mut wall_ms: Option<u64> = None;
    let mut last_advance: Option<String> = None;
    let mut anomalies: BTreeMap<String, u32> = BTreeMap::new();
    let mut anomaly_records: Vec<AnomalyRecord> = Vec::new();
    let mut terminator: Option<TerminatorKind> = None;
    let mut n_llm_requests: u32 = 0;
    let mut n_artifact_writes: u32 = 0;
    let mut n_tool_invocations: u32 = 0;

    // Per-turn accumulator for the `wrong-fence-info-string`
    // detector. We can't make the call until we know whether the
    // current turn produced any `artifact-written` event -- the
    // pattern only counts when assistant text contains a fence
    // opening with a language tag AND no artifact landed. Reset
    // on every `request-llm-response`; finalize when the next one
    // arrives (or at EOF).
    let mut turn_text = String::new();
    let mut turn_wrote_artifact = false;
    let mut turn_step: Option<String> = None;
    let mut turn_ts_ms: u64 = 0;

    let bump = |anomalies: &mut BTreeMap<String, u32>, kind: &str| {
        *anomalies.entry(kind.to_string()).or_insert(0) += 1;
    };

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let envelope: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let dir = envelope.get("dir").and_then(|v| v.as_str()).unwrap_or("");
        let ts_ms = envelope.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
        let event = envelope.get("event").cloned().unwrap_or_default();
        let event_kind = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
        let meta_kind = event.get("kind").and_then(|v| v.as_str()).unwrap_or("");

        // Meta lines (run-start / run-end) carry trial-level
        // params we surface in the summary.
        if dir == "meta" {
            match meta_kind {
                "run-start" => {
                    model = event
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    backend = event
                        .get("backend")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    seed = event.get("seed").and_then(|v| v.as_u64()).map(|n| n as u32);
                }
                "run-end" => {
                    wall_ms = event.get("wall_ms").and_then(|v| v.as_u64());
                }
                _ => {}
            }
            continue;
        }

        match (dir, event_kind) {
            ("out", "request-llm-response") => {
                // Finalize the previous turn before starting a new one.
                if !turn_text.is_empty() && !turn_wrote_artifact && has_lang_tag_fence(&turn_text) {
                    bump(&mut anomalies, "wrong-fence-info-string");
                    anomaly_records.push(AnomalyRecord {
                        kind: "wrong-fence-info-string",
                        step: turn_step.clone(),
                        ts_ms: turn_ts_ms,
                        snippet: trim_snippet(&turn_text),
                    });
                }
                turn_text.clear();
                turn_wrote_artifact = false;
                turn_step.clone_from(&last_advance);
                turn_ts_ms = ts_ms;
                n_llm_requests += 1;
            }
            ("in", "llm-chunk") => {
                if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                    turn_text.push_str(text);
                }
            }
            ("out", "artifact-written") => {
                n_artifact_writes += 1;
                turn_wrote_artifact = true;
            }
            ("out", "tool-invoked") => {
                n_tool_invocations += 1;
                let name = event.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if status == "error" && name == "edit_file" {
                    bump(&mut anomalies, "edit-file-stale-old-string");
                    anomaly_records.push(AnomalyRecord {
                        kind: "edit-file-stale-old-string",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(
                            event
                                .get("args_summary")
                                .and_then(|v| v.as_str())
                                .unwrap_or(""),
                        ),
                    });
                } else if status == "error" && name == "write_file" {
                    bump(&mut anomalies, "write-file-error");
                    anomaly_records.push(AnomalyRecord {
                        kind: "write-file-error",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(
                            event
                                .get("args_summary")
                                .and_then(|v| v.as_str())
                                .unwrap_or(""),
                        ),
                    });
                }
            }
            ("out", "state-advanced") => {
                last_advance = event.get("to").and_then(|v| v.as_str()).map(str::to_string);
            }
            ("out", "diagnostic") => {
                let level = event.get("level").and_then(|v| v.as_str()).unwrap_or("");
                let msg = event.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if level == "warning" && msg.contains("salvaged critique JSON") {
                    bump(&mut anomalies, "bare-json-no-fence");
                    anomaly_records.push(AnomalyRecord {
                        kind: "bare-json-no-fence",
                        step: extract_step(msg),
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                } else if level == "info" && msg.contains("salvaged critique JSON") {
                    bump(&mut anomalies, "bare-json-no-fence-expected");
                    anomaly_records.push(AnomalyRecord {
                        kind: "bare-json-no-fence-expected",
                        step: extract_step(msg),
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                } else if level == "warning" && msg.contains("LLM returned no content") {
                    bump(&mut anomalies, "empty-response");
                    anomaly_records.push(AnomalyRecord {
                        kind: "empty-response",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                } else if level == "warning" && msg.contains("Loop guard warning") {
                    bump(&mut anomalies, "loop-guard-warning");
                    anomaly_records.push(AnomalyRecord {
                        kind: "loop-guard-warning",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                } else if level == "error" {
                    let cause = classify_terminator(msg);
                    if let Some(kind) = cause {
                        // Capture the FIRST flow-fatal terminator
                        // (a session can also emit a follow-up
                        // "flipping to manual mode" diagnostic
                        // that we don't want to overwrite the
                        // real cause with).
                        if terminator.is_none() {
                            terminator = Some(kind);
                        }
                        bump(&mut anomalies, kind.as_str());
                        anomaly_records.push(AnomalyRecord {
                            kind: kind_static(kind),
                            step: extract_step(msg),
                            ts_ms,
                            snippet: trim_snippet(msg),
                        });
                    }
                }
            }
            ("in", "llm-error") => {
                let msg = event.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if msg.contains("truncated at max_tokens") || msg.contains("stop_reason=max_tokens")
                {
                    bump(&mut anomalies, "llm-truncated-at-max-tokens");
                    anomaly_records.push(AnomalyRecord {
                        kind: "llm-truncated-at-max-tokens",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                } else {
                    bump(&mut anomalies, "llm-error-other");
                    anomaly_records.push(AnomalyRecord {
                        kind: "llm-error-other",
                        step: None,
                        ts_ms,
                        snippet: trim_snippet(msg),
                    });
                }
            }
            _ => {}
        }
    }

    // Flush the last turn (no follow-up `request-llm-response`
    // arrived, so the in-loop finalizer above never fired for it).
    if !turn_text.is_empty() && !turn_wrote_artifact && has_lang_tag_fence(&turn_text) {
        bump(&mut anomalies, "wrong-fence-info-string");
        anomaly_records.push(AnomalyRecord {
            kind: "wrong-fence-info-string",
            step: turn_step.clone(),
            ts_ms: turn_ts_ms,
            snippet: trim_snippet(&turn_text),
        });
    }

    // Stash the anomaly records alongside the summary so
    // `write_trial_outputs` can serialize them; we don't surface
    // them on the summary type itself (kept small + flat for the
    // aggregate roll-up).
    {
        let trial_dir = path
            .parent()
            .ok_or_else(|| "capture has no parent dir".to_string())?;
        write_anomalies_jsonl(&trial_dir.join("anomalies.jsonl"), &anomaly_records)?;
    }

    Ok(TrialSummary {
        capture: path.to_path_buf(),
        model: if model.is_empty() {
            "?".to_string()
        } else {
            model
        },
        backend,
        seed,
        wall_ms,
        last_advance,
        terminator,
        anomalies,
        n_llm_requests,
        n_artifact_writes,
        n_tool_invocations,
    })
}

/// True iff the assistant text contains a fenced block whose
/// info-string is a language tag (`markdown`, `json`, `toml`,
/// `rust`, `yaml`, `html`, `text`) rather than the canonical
/// relative path required by the artifact-write convention.
///
/// This is the failure mode the work-stall investigation surfaced:
/// qwen3.6 with thinking disabled often emits
///
/// ```text
/// ```markdown
/// # File content...
/// ```
/// ```
///
/// instead of `` ```docs/<step>/<file>.md ``. The orchestrator's
/// `extract_artifacts` rejects the language-tag info-string and
/// silently drops the body; the model believes the file landed.
///
/// Caller pairs this with a "no `artifact-written` this turn"
/// check before counting an anomaly: a turn that contains
/// fenced sample code in prose AND also writes a real artifact
/// is fine. The combo "fence-with-lang-tag AND no artifact
/// landed" is the actual stall signature.
fn has_lang_tag_fence(text: &str) -> bool {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            let tag = rest.trim();
            if matches!(
                tag,
                "markdown"
                    | "md"
                    | "json"
                    | "toml"
                    | "rust"
                    | "rs"
                    | "yaml"
                    | "yml"
                    | "html"
                    | "text"
                    | "txt"
            ) {
                return true;
            }
        }
    }
    false
}

fn classify_terminator(msg: &str) -> Option<TerminatorKind> {
    if msg.contains("identical responses") {
        Some(TerminatorKind::RunawayLoop)
    } else if msg.contains("made no progress for") && msg.contains("blocker count") {
        Some(TerminatorKind::CritiqueNoProgress)
    } else if msg.contains("hit no-progress cap") && msg.contains("cargo-test runs") {
        Some(TerminatorKind::CargoTestNoProgress)
    } else if msg.contains("without producing an artifact") {
        Some(TerminatorKind::WorkNoArtifact)
    } else if msg.contains("exceeded max_auto_iters") && msg.contains("Last gate failures:") {
        // Distinct from `WorkNoArtifact`: the work session DID
        // produce artifacts (otherwise the gate-failures branch
        // doesn't run), but the structural gate stayed dirty.
        // Surfaces e.g. when milestone task rows stay `- [ ]`
        // even though the source files landed.
        Some(TerminatorKind::WorkGateStillDirty)
    } else if msg.contains("critique still has") && msg.contains("after") && msg.contains("retries")
    {
        Some(TerminatorKind::CritiqueIterCap)
    } else if msg.contains("phase exceeded") && msg.contains("iterations") {
        Some(TerminatorKind::PhaseIterCap)
    } else if msg.contains("host disconnected") || msg.contains("HostClosed") {
        Some(TerminatorKind::HostClosed)
    } else {
        None
    }
}

fn kind_static(k: TerminatorKind) -> &'static str {
    // Re-borrow as a static so AnomalyRecord can hold it. The enum's
    // `as_str` already returns &'static str.
    k.as_str()
}

fn extract_step(msg: &str) -> Option<String> {
    // Diagnostic messages emitted by auto.rs follow the format
    // `auto: <STEP> <verb> ...` (or `Advance: <STEP> ...`). Pull
    // the second whitespace-separated token when the first is
    // `auto:` / `Advance:`.
    let mut parts = msg.split_whitespace();
    let head = parts.next()?;
    if head == "auto:" || head == "Advance:" {
        parts.next().map(|s| s.trim_end_matches(':').to_string())
    } else {
        None
    }
}

fn trim_snippet(s: &str) -> String {
    let one_line = s.replace(['\n', '\r'], " ");
    let mut iter = one_line.chars();
    let head: String = iter.by_ref().take(280).collect();
    if iter.next().is_some() {
        format!("{head}...")
    } else {
        one_line
    }
}

// -------------------------------------------------------------------
// Per-trial outputs.
// -------------------------------------------------------------------

fn write_anomalies_jsonl(path: &Path, records: &[AnomalyRecord]) -> Result<(), String> {
    let file = File::create(path).map_err(|err| format!("create {}: {err}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for r in records {
        let line = serde_json::to_string(r).map_err(|err| format!("serialize anomaly: {err}"))?;
        writer
            .write_all(line.as_bytes())
            .map_err(|err| format!("write anomaly: {err}"))?;
        writer
            .write_all(b"\n")
            .map_err(|err| format!("write newline: {err}"))?;
    }
    writer.flush().ok();
    Ok(())
}

fn write_trial_outputs(capture: &Path, summary: &TrialSummary) -> Result<(), String> {
    let trial_dir = capture
        .parent()
        .ok_or_else(|| "capture has no parent dir".to_string())?;
    let summary_path = trial_dir.join("summary.json");
    let body =
        serde_json::to_string_pretty(summary).map_err(|err| format!("serialize summary: {err}"))?;
    std::fs::write(&summary_path, body)
        .map_err(|err| format!("write {}: {err}", summary_path.display()))?;
    Ok(())
}

// -------------------------------------------------------------------
// Aggregate (per-model + per-study).
// -------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudySummary {
    /// Per-model aggregates, keyed by the model id from the
    /// run-start meta.
    pub per_model: BTreeMap<String, ModelSummary>,
    /// Total trials analyzed.
    pub trials: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub trials: usize,
    /// Histogram of last-advance values (one bucket per step that
    /// any trial reached, plus `(none)` for trials that didn't
    /// advance).
    pub advance_depth: BTreeMap<String, u32>,
    /// Histogram of terminator kinds.
    pub terminator: BTreeMap<String, u32>,
    /// Per-anomaly-kind median + max across trials.
    pub anomalies: BTreeMap<String, AnomalyStats>,
    /// Wall-time aggregates across trials with `wall_ms` set.
    pub wall_ms: WallStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyStats {
    pub median: u32,
    pub max: u32,
    pub trials_affected: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WallStats {
    pub min_ms: Option<u64>,
    pub median_ms: Option<u64>,
    pub max_ms: Option<u64>,
}

impl StudySummary {
    fn from_trials(trials: &[TrialSummary]) -> Self {
        let mut by_model: BTreeMap<String, Vec<&TrialSummary>> = BTreeMap::new();
        for t in trials {
            by_model.entry(t.model.clone()).or_default().push(t);
        }
        let per_model = by_model
            .into_iter()
            .map(|(model, ts)| (model, ModelSummary::from_trials(&ts)))
            .collect();
        Self {
            per_model,
            trials: trials.len(),
        }
    }
}

impl ModelSummary {
    fn from_trials(trials: &[&TrialSummary]) -> Self {
        let mut advance_depth: BTreeMap<String, u32> = BTreeMap::new();
        let mut terminator: BTreeMap<String, u32> = BTreeMap::new();
        let mut anomaly_buckets: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        let mut wall_samples: Vec<u64> = Vec::new();
        for t in trials {
            let depth_key = t
                .last_advance
                .clone()
                .unwrap_or_else(|| "(none)".to_string());
            *advance_depth.entry(depth_key).or_insert(0) += 1;
            let term_key = t
                .terminator
                .map(|k| k.as_str().to_string())
                .unwrap_or_else(|| "(none)".to_string());
            *terminator.entry(term_key).or_insert(0) += 1;
            for (kind, count) in &t.anomalies {
                anomaly_buckets
                    .entry(kind.clone())
                    .or_default()
                    .push(*count);
            }
            if let Some(w) = t.wall_ms {
                wall_samples.push(w);
            }
        }
        let mut anomalies: BTreeMap<String, AnomalyStats> = BTreeMap::new();
        for (kind, mut counts) in anomaly_buckets {
            counts.sort_unstable();
            let median = counts[counts.len() / 2];
            let max = *counts.iter().max().unwrap_or(&0);
            let trials_affected = counts.len() as u32;
            anomalies.insert(
                kind,
                AnomalyStats {
                    median,
                    max,
                    trials_affected,
                },
            );
        }
        wall_samples.sort_unstable();
        let wall_ms = if wall_samples.is_empty() {
            WallStats::default()
        } else {
            WallStats {
                min_ms: wall_samples.first().copied(),
                median_ms: Some(wall_samples[wall_samples.len() / 2]),
                max_ms: wall_samples.last().copied(),
            }
        };
        Self {
            trials: trials.len(),
            advance_depth,
            terminator,
            anomalies,
            wall_ms,
        }
    }
}

fn write_study_outputs(study_root: &Path, study: &StudySummary) -> Result<(), String> {
    let json_path = study_root.join("summary.json");
    let body = serde_json::to_string_pretty(study)
        .map_err(|err| format!("serialize study summary: {err}"))?;
    std::fs::write(&json_path, body)
        .map_err(|err| format!("write {}: {err}", json_path.display()))?;
    let md_path = study_root.join("summary.md");
    let md = render_study_markdown(study);
    std::fs::write(&md_path, md).map_err(|err| format!("write {}: {err}", md_path.display()))?;
    Ok(())
}

fn render_study_markdown(study: &StudySummary) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Study summary (study_analyze)\n\nTrials analyzed: {}.\n\n",
        study.trials
    ));
    for (model, m) in &study.per_model {
        out.push_str(&format!("## Model `{model}`\n\n"));
        out.push_str(&format!("- Trials: {}\n", m.trials));
        if let (Some(min), Some(med), Some(max)) =
            (m.wall_ms.min_ms, m.wall_ms.median_ms, m.wall_ms.max_ms)
        {
            out.push_str(&format!("- Wall ms: min {min}, median {med}, max {max}\n"));
        }
        out.push_str("\n### Advance depth\n\n");
        out.push_str("| step | trials |\n|---|---|\n");
        for (step, n) in &m.advance_depth {
            out.push_str(&format!("| {step} | {n} |\n"));
        }
        out.push_str("\n### Terminators\n\n");
        out.push_str("| kind | trials |\n|---|---|\n");
        for (kind, n) in &m.terminator {
            out.push_str(&format!("| {kind} | {n} |\n"));
        }
        if !m.anomalies.is_empty() {
            out.push_str("\n### Anomalies\n\n");
            out.push_str("| kind | median | max | trials_affected |\n|---|---|---|---|\n");
            for (kind, stats) in &m.anomalies {
                out.push_str(&format!(
                    "| {kind} | {} | {} | {} |\n",
                    stats.median, stats.max, stats.trials_affected
                ));
            }
        }
        out.push('\n');
    }
    out
}
