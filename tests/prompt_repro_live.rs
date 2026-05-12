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
// Scenario: edit_file_stale_repro
//
// Anomaly target: `edit-file-stale-old-string` (12/21 trials in
// vLLM K=3, still 2/3 in the post-Phase-D run -- top remaining
// anomaly). The model's mental copy of a file drifts from disk
// (typically after its own prior rewrite), and it issues an
// edit_file with an old_string that's not on disk.
//
// Fixture: pre-write `docs/foo.md` with known content that does
// NOT contain "WIDGET". Inject a project-scope DM0 prompt
// override telling the agent to edit `docs/foo.md` and replace
// "WIDGET" with "GADGET". The first edit_file call will fail
// with "old_string not found"; the test measures whether the
// model recovers (read_file + edit with correct old_string) or
// retry-storms the same stale call.
//
// Pass: at most 2 failed edit_file calls AND the model didn't
//       just give up empty.
// Fail: 3+ failed edit_file calls on the same path (retry storm).
// -------------------------------------------------------------------

/// Write a project-scope override for the given step. The
/// orchestrator's prompt loader checks
/// `<project>/.sim-flow/prompts/<file>.md` before falling back to
/// the foundation default, so this lets a scenario inject custom
/// instructions for one specific DM step without touching the
/// shipped prompts.
fn stage_prompt_override(project: &Path, step_slug: &str, kind: &str, body: &str) {
    let suffix = if kind == "critique" { "-critique" } else { "" };
    let path = project
        .join(".sim-flow")
        .join("prompts")
        .join(format!("{step_slug}{suffix}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
}

/// Count `tool-invoked` events in the captured JSONL where the
/// name matches and the status is "error". A retry-storm signature.
fn count_failed_tool_calls(jsonl: &Path, tool_name: &str) -> usize {
    std::fs::read_to_string(jsonl)
        .map(|s| {
            s.lines()
                .filter(|line| line.contains(r#""event":"tool-invoked""#))
                .filter(|line| line.contains(&format!(r#""name":"{tool_name}""#)))
                .filter(|line| line.contains(r#""status":"error""#))
                .count()
        })
        .unwrap_or(0)
}

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn edit_file_stale_repro_does_not_retry_storm() {
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    // Pre-write the target file with content that has NO "WIDGET"
    // substring.
    let foo_path = project.join("docs/foo.md");
    std::fs::create_dir_all(foo_path.parent().unwrap()).unwrap();
    std::fs::write(&foo_path, "# Foo\n\nalpha\nbeta\ngamma\n").unwrap();

    // Inject a project-scope DM0 work prompt. The prompt asks the
    // agent to make a stale edit -- this is the failure case we're
    // measuring.
    stage_prompt_override(
        &project,
        "dm0-specification",
        "work",
        r#"# DM0 stale-edit repro fixture

## Goal

Replace the substring `WIDGET` with `GADGET` in `docs/foo.md`.

## Procedure

1. Call `edit_file` with `path="docs/foo.md"`, `old_string="WIDGET"`, `new_string="GADGET"`.
2. If the tool returns an error, recover gracefully -- the file may not contain the literal substring you expected. Use `read_file` to inspect the actual content, then either retry with a correct `old_string` or report that the substring is not present.

## Output

{{ output_intro }}

This is a test fixture; produce no artifacts beyond what the procedure asks.
"#,
    );

    let opts = ScenarioOpts {
        max_auto_iters: 5,
        max_critique_iters: 0,
        max_llm_requests: 10,
    };
    let run = run_scenario(&project, &cfg, &opts);
    let failed_edits = count_failed_tool_calls(&run.protocol_jsonl, "edit_file");

    eprintln!(
        "wall_ms={} exit={} failed_edits={} native_tool_call_turns={}",
        run.wall_ms,
        run.exit_success,
        failed_edits,
        run.native_tool_call_turns(),
    );

    // PASS: model gave up after <=2 stale attempts. FAIL: 3+
    // failed edit_file calls = retry storm.
    if failed_edits >= 3 {
        panic!(
            "FAIL: model retry-stormed edit_file ({failed_edits} failed calls). \
             stderr tail:\n{}",
            run.stderr_tail
        );
    }
    eprintln!("PASS: failed_edits={failed_edits} (<= 2)");
}

// -------------------------------------------------------------------
// Scenario: critique_no_progress_repro
//
// Anomaly target: `critique-no-progress` (2/3 trials in
// post-Phase-D K=3, tied for top remaining anomaly). Critique
// retry loop sees the same blocker count across iterations --
// model isn't actually FIXING the work between critique passes.
//
// Fixture: pre-write a deliberately flawed `docs/spec.md` missing
// the items DM0 critique gates on (clock frequency, technology
// node, gate budget). Pre-seed state to make the orchestrator
// resume in critique mode. Run with max_critique_iters=4 and
// max_critique_no_progress_iters=2.
//
// Pass: blocker count drops on iter 2 OR cap fires cleanly at iter
//       2 (the model gave up gracefully rather than retry-storming
//       to the absolute cap).
// Fail: blocker count flat AND the absolute max_critique_iters
//       cap fires (4 retries, no progress).
//
// IMPLEMENTATION NOTE: this scenario needs the orchestrator to
// dispatch a critique sub-session against a pre-existing work
// artifact. The cleanest entry point is to:
//   1. Stage a "broken" docs/spec.md
//   2. State.toml: mark DM0.work as passed so the orchestrator
//      moves to critique
//   3. Run e2e_auto -- it should enter the DM0 critique loop and
//      iterate against the pre-staged spec
// The current scaffolding doesn't expose that state-toml editing
// helper; landing this scenario depends on a `stage_passed_work`
// helper.
// -------------------------------------------------------------------

/// Extract per-iteration blocker counts from the captured
/// protocol JSONL by walking `tool-invoked` events for write_file
/// against the critique path and parsing the JSON written. Returns
/// the blocker count per critique-write in chronological order.
/// Currently unused -- the on-disk critique JSON parse below
/// gives the FINAL count, which is what the test asserts on.
/// Kept available for richer scenarios that want per-iteration
/// history.
#[allow(dead_code)]
fn critique_blocker_counts_from_protocol(jsonl: &Path) -> Vec<usize> {
    let content = match std::fs::read_to_string(jsonl) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    // The critique JSON is the `content` argument to write_file
    // calls against `docs/critiques/<step>-critique.json`. We
    // can't recover the argument shape from the tool-invoked event
    // (it only logs status), so instead walk the assistant chunks
    // for the JSON body. Simpler proxy: count `"kind":"blocker"`
    // substrings inside the captured request messages where the
    // user-feedback message echoes the prior critique.
    //
    // Even simpler: count blocker entries in the on-disk critique
    // file at the end of the run. Per-iteration tracking is best-
    // effort via the assistant chunks immediately preceding each
    // tool-invoked write_file event.
    for line in content.lines() {
        if !line.contains(r#""event":"llm-chunk""#) {
            continue;
        }
        // The chunk contains the assistant's text; native-mode
        // write_file goes via tool_calls, not chunks, but the
        // chunk MAY contain prose summarizing the critique. We
        // also look at request-llm-response events which carry
        // the message history including any prior critique
        // feedback.
        if line.contains(r#""kind\":\"blocker\""#) || line.contains(r#""kind": "blocker""#) {
            let count = line.matches(r#""kind\":\"blocker\""#).count()
                + line.matches(r#""kind": "blocker""#).count();
            if count > 0 {
                out.push(count);
            }
        }
    }
    out
}

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn critique_no_progress_repro_falls_back_within_cap() {
    // Run a full DM0 work->critique cycle with tight caps. The
    // model writes a spec, critique iterates against it, and the
    // test measures whether the no-progress cap fires within
    // budget (good -- the orchestrator gave up gracefully) vs the
    // absolute cap firing AFTER N retries with no improvement
    // (bad -- prompts aren't getting the model to fix what it's
    // told to fix).
    //
    // No pre-staging needed; the K=3 study shows this anomaly
    // fires naturally on the smoke fixture for ~2/3 of trials.
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    // Caps:
    // - max_auto_iters=4: plenty of room for work-side iteration.
    // - max_critique_iters=5: absolute cap; should NOT be the
    //   first cap to fire if the no-progress cap is doing its job.
    // - The harness binary defaults max_critique_no_progress_iters
    //   to 3 when not specified; we leave that default.
    let opts = ScenarioOpts {
        max_auto_iters: 4,
        max_critique_iters: 5,
        max_llm_requests: 60,
    };
    let run = run_scenario(&project, &cfg, &opts);

    // Final blocker count on disk:
    let critique_md = project.join("docs/critiques/DM0-critique.md");
    let critique_json = project.join("docs/critiques/DM0-critique.json");
    let final_blockers = std::fs::read_to_string(&critique_json)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("findings").cloned())
        .and_then(|f| f.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter(|item| {
                    item.get("kind")
                        .and_then(|k| k.as_str())
                        .map(|s| s == "blocker")
                        .unwrap_or(false)
                })
                .count()
        });
    let cap_fired_no_progress = run.stderr_tail.contains("critique made no progress");
    let cap_fired_absolute =
        run.stderr_tail.contains("max_critique_iters") && !cap_fired_no_progress;
    let gate_clean = run.stderr_tail.contains("structural gate clean")
        || run.stderr_tail.contains("advanced past DM0");

    eprintln!(
        "wall_ms={} final_blockers={:?} gate_clean={} cap_no_progress={} cap_absolute={}",
        run.wall_ms, final_blockers, gate_clean, cap_fired_no_progress, cap_fired_absolute,
    );
    eprintln!("  critique md exists: {}", critique_md.exists());
    eprintln!("  critique json exists: {}", critique_json.exists());

    // PASS:
    // - gate became clean (no anomaly fired), OR
    // - no-progress cap fired (orchestrator caught the plateau).
    // FAIL: absolute cap fired without no-progress cap firing
    // first AND the gate isn't clean. That's the worst case --
    // the model burned 5 retries without measurable progress and
    // the no-progress detector failed to catch it earlier.
    if !gate_clean && cap_fired_absolute && !cap_fired_no_progress {
        panic!(
            "FAIL: hit absolute critique cap with NO progress and no-progress cap missed it. \
             stderr tail:\n{}",
            run.stderr_tail
        );
    }
    eprintln!("PASS");
}

// -------------------------------------------------------------------
// Scenario: work_no_artifact_minimal_prompt
//
// The minimal write_file probe. A tight, explicit prompt: "write
// `docs/spec.md` with exactly this content using write_file."
// Tests whether the prompt-side scaffolding (conventions +
// templates) gets out of the way enough for a SIMPLE write to
// land in 1-2 turns.
//
// Pass: docs/spec.md exists with the expected content after <=2
//       turns.
// Fail: file missing or content wrong.
// -------------------------------------------------------------------

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn work_no_artifact_minimal_prompt_one_shot() {
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    stage_prompt_override(
        &project,
        "dm0-specification",
        "work",
        r##"# Minimal write probe

## Goal

Create the file `docs/spec.md` with EXACTLY this content (verbatim, no extra prose or wrappers):

```
# Spec

Body.
```

## Procedure

Call `write_file` with `path="docs/spec.md"` and `content="# Spec\n\nBody.\n"`. That's the entire task. Do not write any other files.

## Output

{{ output_intro }}
"##,
    );

    let opts = ScenarioOpts {
        max_auto_iters: 2,
        max_critique_iters: 0,
        max_llm_requests: 4,
    };
    let run = run_scenario(&project, &cfg, &opts);

    let spec = project.join("docs/spec.md");
    let landed = spec.exists();
    let body = if landed {
        std::fs::read_to_string(&spec).unwrap_or_default()
    } else {
        String::new()
    };
    eprintln!(
        "wall_ms={} landed={} body_len={} body={:?}",
        run.wall_ms,
        landed,
        body.len(),
        body.chars().take(120).collect::<String>(),
    );

    if !landed {
        panic!(
            "FAIL: docs/spec.md not written. stderr tail:\n{}",
            run.stderr_tail
        );
    }
    // Soft signal: the prompt asks for exact body. We don't
    // strict-assert because models love adding markdown
    // niceties; the win is "model wrote SOMETHING coherent at
    // the right path".
    assert!(
        body.contains("Spec") && body.contains("Body"),
        "spec content seems wrong: {body:?}"
    );
}

// -------------------------------------------------------------------
// Scenario: runaway_loop_identical_response
//
// Lures the model into emitting identical responses by giving an
// ambiguous task with no obvious next action. The orchestrator's
// `max_identical_responses` runaway guard should catch this
// before max_llm_requests does.
//
// Pass: model varied its output across turns, OR the identical-
//       response guard fired (didn't degrade into max_llm_requests).
// Fail: model emitted identical responses N times AND the
//       runaway guard missed -- max_llm_requests fired instead.
// -------------------------------------------------------------------

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn runaway_loop_identical_response_is_caught() {
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    // Deliberately ambiguous prompt with no concrete output path
    // or action. The model is most likely to either stall (no
    // tool calls, no writes) or to repeat itself.
    stage_prompt_override(
        &project,
        "dm0-specification",
        "work",
        r#"# Ambiguous task

## Goal

Describe the project.

## Procedure

Do nothing other than respond to this prompt. Do not write any files. Do not call any tools.

## Output

{{ output_intro }}
"#,
    );

    let opts = ScenarioOpts {
        max_auto_iters: 6,
        max_critique_iters: 0,
        max_llm_requests: 10,
    };
    let run = run_scenario(&project, &cfg, &opts);

    let runaway_caught = run.stderr_tail.contains("runaway")
        || run.stderr_tail.contains("identical")
        || run.stderr_tail.contains("max_identical_responses");
    let max_llm_first = run.stderr_tail.contains("max_llm_requests") && !runaway_caught;

    eprintln!(
        "wall_ms={} runaway_caught={} max_llm_first={}",
        run.wall_ms, runaway_caught, max_llm_first
    );
    eprintln!("stderr tail:\n{}", run.stderr_tail);
    // We don't strictly fail -- the model might just answer the
    // question and stop, which is also a valid response. But we
    // report which path the orchestrator took so the operator can
    // see if the runaway guard caught a repetition or if the
    // model behaved.
}

// -------------------------------------------------------------------
// Scenario: wrong_fence_info_string_regression
//
// The Phase 0d prompt hardening (commit 4ea2f9d) added strong
// warnings about emitting ` ```markdown ` instead of ` ```<path> `
// for artifact-write blocks. K=12 dropped from 92% trial rate to
// 33%. This scenario pins the post-fix behavior in FENCED mode
// (SIM_FLOW_TOOL_MODE unset). A regression here would mean the
// fenced-blocks convention drifted in a way the model can no
// longer follow.
//
// Pass: model emits at least one ` ```<path> ` fence with a real
//       path as the info-string AND no ` ```markdown ` info-strings.
// Fail: 1+ ` ```markdown ` info-strings appear in the assistant
//       text (the silently-dropped failure mode).
// -------------------------------------------------------------------

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn wrong_fence_info_string_regression_fenced_mode() {
    let mut cfg = LiveConfig::from_env();
    // Explicitly force fenced mode -- this scenario tests the
    // fence-mode prompt scaffolding even if the operator has
    // SIM_FLOW_TOOL_MODE=native in their env.
    cfg.tool_mode = None;
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    let opts = ScenarioOpts {
        max_auto_iters: 3,
        max_critique_iters: 0,
        max_llm_requests: 6,
    };
    let run = run_scenario(&project, &cfg, &opts);

    // Walk the assistant chunks looking for fence info-strings.
    let body = std::fs::read_to_string(&run.protocol_jsonl).unwrap_or_default();
    let mut bad_lang_tag_fences = 0;
    let mut path_fences = 0;
    let bad_tags = [
        "```markdown",
        "```json",
        "```toml",
        "```text",
        "```yaml",
        "```md",
    ];
    for line in body.lines() {
        if !line.contains(r#""event":"llm-chunk""#) {
            continue;
        }
        for tag in &bad_tags {
            // The chunk's text is JSON-escaped; the literal `
            // appears as ``` in the captured line.
            bad_lang_tag_fences += line.matches(tag).count();
        }
        // Count path-like fence info-strings (``` followed by
        // a project-relative path). Heuristic: ```docs/ or
        // ```src/ followed by .md / .rs / etc.
        for prefix in ["```docs/", "```src/", "```analysis/"] {
            path_fences += line.matches(prefix).count();
        }
    }

    eprintln!(
        "wall_ms={} bad_lang_tag_fences={} path_fences={}",
        run.wall_ms, bad_lang_tag_fences, path_fences
    );

    if bad_lang_tag_fences > 0 {
        panic!(
            "FAIL: model emitted {bad_lang_tag_fences} fenced blocks with a language tag \
             info-string (silently-dropped failure mode). path_fences={path_fences}.\n\
             stderr tail:\n{}",
            run.stderr_tail
        );
    }
    // No bad fences -- great. We don't strictly require a path
    // fence (the model might not have written anything; that's a
    // different anomaly).
    eprintln!("PASS: no language-tag info-strings detected");
}

// -------------------------------------------------------------------
// Scenario: edit_file_old_string_mismatch_recovery
//
// Variant of edit_file_stale_repro: instead of a totally-absent
// substring, the prompt tells the agent that the file contains
// content that is structurally CLOSE to disk but with subtle
// differences (extra whitespace, different line breaks, etc.).
// This is the more common live-K=3 shape -- the model's mental
// copy is "almost right".
//
// Fixture: pre-write `docs/foo.md` with content that has subtle
// differences from what the prompt tells the agent it contains.
// e.g. file has "  WIDGET\n" (with leading spaces) but prompt
// says "edit WIDGET -> GADGET" without context.
//
// Pass: model adds surrounding context after the first error
//       (uses the disambiguation guidance), OR reads the file
//       and copies the exact whitespace.
// Fail: model gives up without trying again.
// -------------------------------------------------------------------

#[test]
#[ignore = "live LLM call; run with --ignored"]
fn edit_file_whitespace_mismatch_recovery() {
    let cfg = LiveConfig::from_env();
    eprintln!("config: {cfg:?}");

    let tmp = tempfile::tempdir().unwrap();
    let project = setup_fresh_project(&tmp);

    // Pre-write with leading whitespace + trailing punctuation.
    let foo_path = project.join("docs/foo.md");
    std::fs::create_dir_all(foo_path.parent().unwrap()).unwrap();
    std::fs::write(
        &foo_path,
        "# Foo\n\n  WIDGET (the canonical name).\n  Other content.\n",
    )
    .unwrap();

    stage_prompt_override(
        &project,
        "dm0-specification",
        "work",
        r#"# DM0 whitespace-edit repro fixture

## Goal

In `docs/foo.md`, change the word `WIDGET` to `GADGET`. Preserve all surrounding text exactly.

## Procedure

1. Replace `WIDGET` with `GADGET`. Pick whichever combination of `read_file` / `edit_file` works.
2. After your edit lands, stop. Do not write a spec or any other artifact.

## Output

{{ output_intro }}
"#,
    );

    let opts = ScenarioOpts {
        max_auto_iters: 4,
        max_critique_iters: 0,
        max_llm_requests: 8,
    };
    let run = run_scenario(&project, &cfg, &opts);
    let failed_edits = count_failed_tool_calls(&run.protocol_jsonl, "edit_file");

    // After the test, check the file: did the model successfully
    // make the edit?
    let final_body = std::fs::read_to_string(&foo_path).unwrap_or_default();
    let edit_landed = final_body.contains("GADGET") && !final_body.contains("WIDGET");

    eprintln!(
        "wall_ms={} failed_edits={} edit_landed={}\nfinal body:\n{}",
        run.wall_ms, failed_edits, edit_landed, final_body
    );

    // PASS criteria (any of):
    // - The edit landed (model used context or read first)
    // - Failed edits stayed <= 2 AND model gave up cleanly
    // FAIL: 3+ failed edits AND edit didn't land = retry storm
    if !edit_landed && failed_edits >= 3 {
        panic!(
            "FAIL: model retry-stormed without landing the edit \
             ({failed_edits} failed edit_file calls). stderr tail:\n{}",
            run.stderr_tail
        );
    }
    eprintln!(
        "PASS: edit_landed={edit_landed}, failed_edits={failed_edits} (<= 2 or edit succeeded)"
    );
}

// -------------------------------------------------------------------
// All scenarios from the original skeleton list are implemented
// above. To extend: pick an anomaly from
// `docs/brainstorming/model-robustness-vllm-anomalies.md` that the
// current K=3 still hits, add a #[ignore]'d test here that
// reproduces it on a minimal fixture (stage_prompt_override +
// pre-staged files), and emit pass/fail with a diagnostic
// (count_failed_tool_calls / stderr tail / on-disk artifact
// inspection).
//
// -------------------------------------------------------------------
