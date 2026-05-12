//! Live prompt-quality scenarios.
//!
//! Sibling to `anomaly_repro.rs`. Where that suite catches
//! orchestrator-side regressions with MockAgent (sub-second, no
//! API), this suite measures PROMPT-quality outcomes against a
//! real backend so you can A/B prompt changes without burning a
//! full K=3 robustness run.
//!
//! ## How to run
//!
//! All tests in this file are `#[ignore]`'d by default. To run
//! one against vLLM/qwen3.6 on `localhost:8012`:
//!
//! ```bash
//! cargo test --test prompt_repro_live -- --ignored work_no_artifact_dm0
//! ```
//!
//! To target a different backend / endpoint:
//!
//! ```bash
//! SIM_FLOW_BACKEND=anthropic \
//! SIM_FLOW_MODEL=claude-opus-4-7 \
//!   cargo test --test prompt_repro_live -- --ignored
//! ```
//!
//! Defaults:
//! - `SIM_FLOW_BACKEND` -- "openai-compat"
//! - `SIM_FLOW_BASE_URL` -- "http://localhost:8012/v1"
//! - `SIM_FLOW_MODEL` -- "qwen3.6"
//! - `SIM_FLOW_TOOL_MODE` -- pass-through (test reads what you set)
//! - `SIM_FLOW_DISABLE_THINKING` -- pass-through
//!
//! ## Iteration loop
//!
//! 1. A K=3 captures an anomaly (e.g. `work-no-artifact` 2/3
//!    trials).
//! 2. Add a scenario here that reproduces the failing case on a
//!    minimal fixture. Run it; confirm it FAILS on the current
//!    prompts (red).
//! 3. Tweak the prompts / conventions / templates.
//! 4. Re-run the scenario; confirm it PASSES (green).
//! 5. The full K=3 study verifies the fix holds across multiple
//!    seeds and edge cases.
//!
//! Each scenario reports the model's behavior in detail (turn
//! count, tool-call counts, artifact paths written, anomaly
//! events detected) so even a "failed" run is informative.
//!
//! ## Scope
//!
//! These tests are slow (10s to a few minutes per scenario) and
//! cost LLM tokens / time. Keep each scenario tight: 1-3 step
//! sub-sessions, low max_auto_iters, no full DM0->DM4b walks.
//! The full walk lives in the K=3 study harness.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

// -------------------------------------------------------------------
// Test runner config (env-driven).
// -------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LiveConfig {
    backend: String,
    base_url: String,
    model: String,
    /// "native" or unset/"fenced". Pass-through to the orchestrator
    /// env. Distinct from the SIM_FLOW_TOOL_MODE env var the host
    /// reads, because the test launches `e2e_auto` as a subprocess
    /// and the orchestrator inherits the env we set on the Command.
    tool_mode: Option<String>,
    /// "1" / unset. Pass-through.
    disable_thinking: Option<String>,
}

impl LiveConfig {
    fn from_env() -> Self {
        Self {
            backend: std::env::var("SIM_FLOW_BACKEND").unwrap_or_else(|_| "openai-compat".into()),
            base_url: std::env::var("SIM_FLOW_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8012/v1".into()),
            model: std::env::var("SIM_FLOW_MODEL").unwrap_or_else(|_| "qwen3.6".into()),
            tool_mode: std::env::var("SIM_FLOW_TOOL_MODE").ok(),
            disable_thinking: std::env::var("SIM_FLOW_DISABLE_THINKING").ok(),
        }
    }
}

fn foundation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn sim_flow_bin() -> PathBuf {
    foundation_root().join("target/debug/sim-flow")
}

fn e2e_auto_bin() -> PathBuf {
    foundation_root().join("target/debug/e2e_auto")
}

fn smoke_spec_path() -> PathBuf {
    foundation_root().join("tools/sim-flow/src/bin/dm_flow_smoke_spec.md")
}

/// Create a fresh model project (via `sim-flow new model`) in a
/// tempdir and return the project root. Matches what
/// `run-robustness-study.sh` does per-trial so the fixture matches
/// the live K=3 environment.
fn setup_fresh_project(tmp: &tempfile::TempDir) -> PathBuf {
    let project_parent = tmp.path();
    let project_name = "proj";
    let foundation = foundation_root();
    let sim_models = foundation
        .parent()
        .expect("workspace parent")
        .join("sim-models");
    let output = Command::new(sim_flow_bin())
        .arg("--foundation-root")
        .arg(&foundation)
        .arg("new")
        .arg("model")
        .arg(project_name)
        .arg("--destination")
        .arg(project_parent)
        .arg("--library-path")
        .arg(&sim_models)
        .arg("--skip-cargo-check")
        .output()
        .expect("sim-flow binary must be built (cargo build -p sim-flow --bins)");
    if !output.status.success() {
        panic!(
            "sim-flow new model failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    project_parent.join(project_name)
}

/// Outcome of a live scenario run: enough detail to either assert
/// pass/fail or print a useful diagnostic when iterating on
/// prompts. The unit tests below pin specific fields; richer
/// scenarios can match on `protocol_jsonl` or `stderr_tail` for
/// fine-grained signals.
#[derive(Debug)]
struct LiveRun {
    /// Did `e2e_auto` itself exit 0? (Note: the harness exits 0
    /// only when the walk reaches DM4b -- most scenarios won't,
    /// and that's fine. Use `wrote_any_of` / `protocol_anomaly`
    /// for actual scenario assertions.)
    exit_success: bool,
    /// Wall clock in ms.
    wall_ms: u128,
    /// Path to the captured protocol JSONL (for ad-hoc grepping
    /// in tests that want fine-grained checks).
    protocol_jsonl: PathBuf,
    /// Tail of e2e_auto's stderr (last ~2 KB). Includes the
    /// orchestrator's stderr writes and the harness's diagnostics.
    stderr_tail: String,
}

impl LiveRun {
    /// True if any of the supplied project-relative paths exist
    /// after the run. Useful for "did the model write the
    /// artifact?" assertions.
    fn wrote_any_of(&self, project: &Path, paths: &[&str]) -> Vec<String> {
        paths
            .iter()
            .filter(|p| project.join(p).exists())
            .map(|p| (*p).to_string())
            .collect()
    }

    /// Count `llm-end` events in the captured JSONL whose
    /// `tool_calls` field is non-empty. Useful as a "did the
    /// model use the native tool path?" signal in native-mode
    /// scenarios.
    fn native_tool_call_turns(&self) -> usize {
        std::fs::read_to_string(&self.protocol_jsonl)
            .map(|s| {
                s.lines()
                    .filter(|line| line.contains(r#""event":"llm-end""#))
                    .filter(|line| line.contains(r#""tool_calls":[{"#))
                    .count()
            })
            .unwrap_or(0)
    }
}

/// Run `e2e_auto` against `project` with the supplied options.
/// Returns when the subprocess exits (success OR failure) or the
/// wall-clock budget runs out. The scenario's caps
/// (`max_auto_iters`, etc.) are what bound the cost; the wall
/// budget is a hard ceiling for runaway dispatches.
fn run_scenario(project: &Path, cfg: &LiveConfig, opts: &ScenarioOpts) -> LiveRun {
    let protocol_jsonl = project.join(".sim-flow/scenario-protocol.jsonl");
    let stderr_log = project.join(".sim-flow/scenario-stderr.log");
    let stdout_log = project.join(".sim-flow/scenario-stdout.log");
    std::fs::create_dir_all(protocol_jsonl.parent().unwrap()).unwrap();
    let started = std::time::Instant::now();
    let mut cmd = Command::new(e2e_auto_bin());
    cmd.arg("--foundation-root")
        .arg(foundation_root())
        .arg("--project-dir")
        .arg(project)
        .arg("--backend")
        .arg(&cfg.backend)
        .arg("--model")
        .arg(&cfg.model)
        .arg("--spec")
        .arg(smoke_spec_path())
        .arg("--no-watch-socket")
        .arg("--capture-jsonl")
        .arg(&protocol_jsonl)
        .arg("--max-auto-iters")
        .arg(opts.max_auto_iters.to_string())
        .arg("--max-critique-iters")
        .arg(opts.max_critique_iters.to_string())
        .arg("--max-llm-requests")
        .arg(opts.max_llm_requests.to_string());
    if !cfg.base_url.is_empty() && cfg.base_url != "n/a" {
        cmd.arg("--base-url").arg(&cfg.base_url);
    }
    if let Some(m) = &cfg.tool_mode {
        cmd.env("SIM_FLOW_TOOL_MODE", m);
    }
    if let Some(t) = &cfg.disable_thinking {
        cmd.env("SIM_FLOW_DISABLE_THINKING", t);
    }
    let stdout_file = std::fs::File::create(&stdout_log).unwrap();
    let stderr_file = std::fs::File::create(&stderr_log).unwrap();
    cmd.stdout(stdout_file).stderr(stderr_file);
    let status = cmd.status().expect("e2e_auto must be built");
    let wall_ms = started.elapsed().as_millis();
    let stderr_full = std::fs::read_to_string(&stderr_log).unwrap_or_default();
    let stderr_tail = stderr_full
        .chars()
        .rev()
        .take(2048)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    LiveRun {
        exit_success: status.success(),
        wall_ms,
        protocol_jsonl,
        stderr_tail,
    }
}

/// Per-scenario caps. Keep these tight so each scenario costs
/// seconds, not minutes.
struct ScenarioOpts {
    max_auto_iters: u32,
    max_critique_iters: u32,
    max_llm_requests: u32,
}

impl ScenarioOpts {
    fn tight() -> Self {
        Self {
            // 3 work-side turns is enough to land docs/spec.md
            // when the prompt is working; a fail means the model
            // never wrote within budget.
            max_auto_iters: 3,
            // Skip critique entirely for work-side scenarios.
            max_critique_iters: 0,
            max_llm_requests: 6,
        }
    }
}

// -------------------------------------------------------------------
// Scenario: work_no_artifact_dm0
//
// Anomaly target: `work-no-artifact` -- model burns max_auto_iters
// consecutive turns reading + thinking without ever writing the
// step's artifact. 12/21 vLLM trials in the model-robustness study
// were affected.
//
// Setup: fresh project at DM0; smoke spec is small enough to
// summarize in one turn.
// Caps: max_auto_iters=3 (tight enough that a stalling model
// fails fast, loose enough that a working prompt has room).
// Pass: docs/spec.md exists OR docs/spec/01-*.md exists after
// the run.
// Fail: neither artifact landed; the model never wrote.
// -------------------------------------------------------------------

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn work_no_artifact_dm0_writes_spec_within_3_turns() {
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    let run = run_scenario(&project, &cfg, &ScenarioOpts::tight());
    eprintln!(
        "wall_ms={} exit={} native_tool_calls={}",
        run.wall_ms,
        run.exit_success,
        run.native_tool_call_turns()
    );

    let candidate_paths = ["docs/spec.md", "docs/spec/01-overview.md"];
    let wrote = run.wrote_any_of(&project, &candidate_paths);

    if wrote.is_empty() {
        // Failure: model didn't write anything in 3 turns. Print
        // a useful diagnostic so the operator iterating on prompts
        // can see what the model DID do.
        let listing = std::fs::read_dir(project.join("docs"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|_| "<docs/ missing>".into());
        panic!(
            "FAIL: DM0 wrote nothing in 3 work turns. \
             native_tool_call_turns={}, docs/ contents=[{}].\n\
             stderr tail (last 2KB):\n{}",
            run.native_tool_call_turns(),
            listing,
            run.stderr_tail,
        );
    }

    eprintln!("PASS: DM0 wrote {wrote:?}");
    // Soft signal: in native mode, expect >0 native tool-call turns.
    // We don't assert this strictly -- the test still passes if the
    // model used the fenced path -- but we surface the count so the
    // operator notices if a native-mode run is silently falling
    // back to fenced.
    if cfg.tool_mode.as_deref() == Some("native") && run.native_tool_call_turns() == 0 {
        eprintln!(
            "WARN: SIM_FLOW_TOOL_MODE=native but 0 turns emitted native tool_calls. \
             The model is using the fenced path -- check prompts."
        );
    }

    // Sanity: didn't exceed a wall-clock budget (90s per scenario
    // gives plenty of headroom even with API latency).
    assert!(
        Duration::from_millis(run.wall_ms as u64) < Duration::from_secs(120),
        "scenario should finish in under 2 minutes; got {} ms",
        run.wall_ms
    );
}

// -------------------------------------------------------------------
// Future scenarios (sketch). Each takes the same `setup + run +
// assert` shape; add them as the K=3 study surfaces new anomalies
// that prompt changes might address.
//
// edit_file_stale_repro
//   setup: pre-write `docs/foo.md` with content "alpha\nbeta\n";
//          set up DM step that asks the agent to edit
//          `docs/foo.md` and replace "missing-substring" with "X".
//   caps: max_auto_iters=4.
//   pass: model either reads-then-fixes OR gracefully errors and
//         asks for help without retry-storming.
//   fail: model retries the same stale edit_file 3+ times before
//         giving up.
//
// critique_no_progress_repro
//   setup: pre-write `docs/spec.md` with 3 deliberate flaws (e.g.
//          missing clock frequency, missing technology node,
//          missing gate budget). Pre-write DM0 critique JSON with
//          all 3 blockers.
//   caps: max_critique_iters=4.
//   pass: blocker count decreases on each retry (model fixes a
//         flaw, runs critique, blocker count drops).
//   fail: all 3 blockers re-appear unchanged on retry 2+.
//
// edit_file_old_string_mismatch_recovery
//   setup: pre-write a file; tell the agent the file says X when
//          it actually says Y.
//   caps: max_auto_iters=3.
//   pass: model re-reads before editing, or recovers from
//         edit_file error within 2 turns.
//
// -------------------------------------------------------------------
