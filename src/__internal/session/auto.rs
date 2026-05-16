//! Phase 2 auto driver: end-to-end work -> critique -> advance loop.
//!
//! `run_auto` drives a sequence of work and critique sessions for a
//! flow's remaining steps without user prompting. It runs in one of
//! two step-axis modes (see `docs/brainstorming/manual-step-mode.md`):
//!
//!   * **Auto** — iterates remaining steps end-to-end. Each iteration
//!     is a work sub-session, then a critique sub-session, then an
//!     advance attempt. If the critique reports `BLOCKER:` findings
//!     (per `parseFindings`-equivalent rules in `extensions/.../state/
//!     critiques.ts`) and the cross-session retry budget allows, the
//!     work sub-session re-runs.
//!
//!   * **Manual** — parks waiting for explicit `RunStep`,
//!     `RunCritique`, `RunGate`, `Advance`, or `Reset` host events.
//!     The orchestrator dispatches each command to the same internals
//!     the auto loop uses, then parks again. Critique-clean alone
//!     never auto-advances; the user must issue `Advance`.
//!
//! Mode is shared via an `Arc<AtomicU8>` between the run loop and the
//! `AutoPresenter` wrapper that intercepts host reads. `SetStepMode` host
//! events flip the flag at the next decision point — never mid-sub-
//! session — and emit a `StepModeChanged` event so the dashboard can
//! reflect the orchestrator's truth. The auto loop also flips to
//! manual when the per-session iteration cap (`max_auto_iters`) or
//! the cross-session critique cap (`max_critique_iters`) is exceeded
//! and when an advance attempt fails — same drop-to-interactive
//! semantics the cap-exceeded path used to have, unified with the
//! user-toggle path.
//!
//! `Shutdown` always wins: the wrapper sets a flag and (if a sub-
//! session is in flight) returns a synthetic `Cancel` so the
//! orchestrator terminates cleanly. The run loop sees the flag and
//! emits a final `SessionEnd` before returning.
//!
//! `AutoPresenter` reuses the existing `run_session` entry point and:
//!
//! - synthesizes a `Hello` for every sub-session after the first (so
//!   the wrapped `run_session` thinks each iteration is a fresh
//!   handshake);
//! - swallows `SessionEnd` writes for every sub-session in BOTH
//!   modes (the host sees exactly one `SessionEnd` when the
//!   orchestrator process exits on `Shutdown` or auto-loop
//!   completion). The dashboard infers sub-session completion from
//!   the absence of new chunks — same as auto mode between work
//!   and critique iterations. The host's `markTerminated` path
//!   treats `SessionEnd` as "the orchestrator process is gone" and
//!   clears its `activeSession` reference, so emitting one per
//!   manual sub-session would knock the toggle out of "live" state
//!   and force the next dashboard click to spawn a fresh side
//!   process;
//! - watches for the `max_auto_iters`-exceeded diagnostic and queues
//!   a `Cancel` so the orchestrator stops the current sub-session
//!   immediately (rather than parking on `RequestUserInput`);
//! - intercepts manual-mode commands when in_subsession is true and
//!   rejects them with a Diagnostic so they never confuse the inner
//!   orchestrator; intercepts `SetStepMode` and `Shutdown` from any
//!   read.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::__internal::critique::read_critique_entry;
use crate::Result;
use crate::session::llm_adapter::LlmAdapter;
use crate::session::orchestrator::{
    OrchestratorOptions, run_session, step_descriptor_for_protocol,
};
use crate::session::presenter::Presenter;
use crate::session::protocol::{
    DiagnosticLevel, Event, GateFailureOut, HostEvent, HostInfo, LlmMessage, LlmRole,
    PROTOCOL_VERSION, SessionEndReason, SessionKindOut, SessionTag, StepMode,
};
use crate::state::State;
use crate::steps::registry_for;

/// Inputs for `run_auto`. The driver picks up the active flow's
/// remaining steps starting from `state.current_step`.
#[derive(Clone)]
pub struct AutoOptions {
    pub project_dir: PathBuf,
    pub foundation_root: PathBuf,
    pub llm_backend: String,
    pub llm_model: Option<String>,
    pub llm_model_family_id: Option<String>,
    pub llm_runtime_profile_id: Option<String>,
    pub llm_debug_adaptation: bool,
    /// Optional base URL override for OpenAI-compatible local
    /// backends (`ollama`, `lmstudio`, `vllm`, `openai-compat`).
    /// Forwarded into each sub-session's `OrchestratorOptions`
    /// and consumed by the agent constructors. `None` means
    /// "use the backend's conventional default".
    pub llm_base_url: Option<String>,
    /// Optional separate LLM stack for critique sessions. When any
    /// of these fields are `Some`, critique sub-sessions use the
    /// override instead of the work-side stack (`llm_backend` etc.
    /// above); fields left `None` here individually fall back to
    /// the work-side value of the same name. Typical use case:
    /// run work on a fast / cheap local model (e.g. vLLM serving a
    /// 27B open model) and route critique to a stronger
    /// hosted model (e.g. `--critique-llm-backend anthropic
    /// --critique-llm-model claude-3-5-sonnet-latest`) so reviews
    /// catch issues the work-side model misses without paying the
    /// hosted-model cost on every turn. The `kind` axis is the only
    /// routing key today; per-step routing isn't modeled.
    pub critique_llm_backend: Option<String>,
    pub critique_llm_model: Option<String>,
    pub critique_llm_model_family_id: Option<String>,
    pub critique_llm_runtime_profile_id: Option<String>,
    pub critique_llm_base_url: Option<String>,
    /// Optional separate LLM stack for idle-state Q&A turns
    /// (`SessionKindOut::Qa` -- triggered by a `UserMessage`
    /// while manual mode is parked between sub-sessions). Each
    /// field falls back per-field to the work-side `llm_*` stack
    /// when unset, mirroring the critique override semantics. Q&A
    /// turns are conversational; users may want them to go to a
    /// chattier / cheaper model than work and critique do.
    pub qa_llm_backend: Option<String>,
    pub qa_llm_model: Option<String>,
    pub qa_llm_model_family_id: Option<String>,
    pub qa_llm_runtime_profile_id: Option<String>,
    pub qa_llm_base_url: Option<String>,
    /// Per-session structural-gate iteration cap (forwarded to the
    /// orchestrator's auto mode).
    pub max_auto_iters: u32,
    /// Cross-session retry cap (absolute ceiling). Each retry re-runs
    /// the work session for the same step (the orchestrator inlines
    /// the critique file so the agent sees what to fix). Hard backstop
    /// even when the agent is still making progress -- if the model
    /// keeps shaving one blocker per pass for 10+ passes, something
    /// is wrong with the prompt or the gate, not just the model.
    pub max_critique_iters: u32,
    /// Cross-session no-progress retry cap. Increments on every retry
    /// whose gate-failing-finding count is NOT strictly less than the
    /// previous pass; resets to 0 on strict progress. Catches the
    /// "model plateaued or is flipping the same finding back and
    /// forth" pattern that `max_critique_iters` (an absolute count)
    /// would only catch after wasting more retries.
    ///
    /// We DO NOT count the first critique pass as a no-progress event
    /// (there's no `previous_count` to compare against on pass 1);
    /// the counter only starts after the first delta is observable.
    pub max_critique_no_progress_iters: u32,
    /// If set, DM0 work runs in interactive mode (auto=false). The
    /// rest of the flow runs auto. Used when no spec has been
    /// provided -- the user collaborates on DM0, then auto takes
    /// over from DM0 critique onward.
    pub dm0_interactive: bool,
    /// Forwarded to each sub-session's `OrchestratorOptions`.
    /// Backstop runaway-loop guard; default 50.
    pub max_llm_requests: u32,
    /// Forwarded to each sub-session. Stuck-loop detection threshold;
    /// default 3.
    pub max_identical_responses: u32,
    /// Cap on concurrent in-flight LLM Work sessions during
    /// plan-detail walks (DM2cd / DM3ad / DM4ad). `0` means
    /// unbounded; `1` collapses to the legacy serial path. Higher
    /// values fan out pending milestone stubs in parallel up to the
    /// cap. Has no effect on execution walks (DM2d / DM3b / DM3c /
    /// DM4b) which stay strictly serial pending the milestone-DAG
    /// work in Phase 3/4 of the parallel-execution brainstorming
    /// doc. Resolved at the CLI layer: CLI flag wins, else
    /// `.sim-flow/config.toml::[llm].max_parallel_requests`, else 0.
    pub max_parallel_requests: u32,
    /// Initial step-axis mode. `Auto` walks `current_step` to end of
    /// flow without user input. `Manual` parks the orchestrator after
    /// the hello handshake and dispatches sub-sessions only in
    /// response to host commands. The mode flag is also live-mutable
    /// mid-run via the `SetStepMode` host event.
    pub step_mode: StepMode,
    /// Forwarded to every sub-session's `OrchestratorOptions`. When
    /// true (default), loads `_conventions/no-preamble.md` into the
    /// system prompt so verbose-CoT models lead with tool calls
    /// instead of preamble.
    pub no_preamble: bool,
}

pub fn run_auto<P, L>(opts: AutoOptions, host: &mut P, llm: &mut L) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    info!(
        project = %opts.project_dir.display(),
        backend = %opts.llm_backend,
        model = opts.llm_model.as_deref().unwrap_or("(default)"),
        model_family = opts.llm_model_family_id.as_deref().unwrap_or("(infer)"),
        runtime_profile = opts.llm_runtime_profile_id.as_deref().unwrap_or("(default)"),
        step_mode = ?opts.step_mode,
        "run_auto starting"
    );
    let mode = Arc::new(AtomicU8::new(step_mode_to_u8(opts.step_mode)));
    let mut auto_host = AutoPresenter::new(host, mode);

    // 1. Hello/HelloAck handshake. Once per process — every sub-
    //    session below queues a synthetic Hello via
    //    `queue_synthetic_hello`, so the real host's Hello is
    //    consumed exactly here. Without this step, manual mode would
    //    park in `wait_for_command` and reject the incoming Hello as
    //    "unexpected", and auto mode's first sub-session would race
    //    with the synthetic-Hello queue.
    perform_initial_handshake(&opts, &mut auto_host)?;

    // 2. Echo back the initial mode so the dashboard's toggle aligns
    //    with the orchestrator's truth before any sub-session runs.
    auto_host.send(&Event::StepModeChanged {
        mode: opts.step_mode,
    })?;

    // 2a. Sanity-check the per-kind LLM routing: a `critique_llm_*`
    //     field set WITHOUT a matching `critique_llm_backend` keeps
    //     the backend on the work-side stack but swaps the model /
    //     base_url. That can silently send (say)
    //     `claude-3-5-sonnet-latest` to a vLLM server that doesn't
    //     know the name. Emit a Diagnostic so the operator sees
    //     what the orchestrator is actually going to dispatch.
    if opts.critique_llm_backend.is_none() {
        let partials: Vec<&str> = [
            ("critique_llm_model", opts.critique_llm_model.is_some()),
            (
                "critique_llm_model_family_id",
                opts.critique_llm_model_family_id.is_some(),
            ),
            (
                "critique_llm_runtime_profile_id",
                opts.critique_llm_runtime_profile_id.is_some(),
            ),
            (
                "critique_llm_base_url",
                opts.critique_llm_base_url.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(name, set)| if set { Some(name) } else { None })
        .collect();
        if !partials.is_empty() {
            let msg = format!(
                "critique-llm override is partial: {} set but `critique_llm_backend` is not. \
                 Critique sub-sessions will use the work-side backend (`{}`) with the \
                 overridden field(s) -- if your intent was to route critique to a \
                 different backend, also set --critique-llm-backend.",
                partials.join(", "),
                opts.llm_backend
            );
            warn!("{msg}");
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message: msg,
            })?;
        }
    }

    // Idle-state Q&A history. Each `UserMessage` received while
    // manual mode is parked between sub-sessions appends to this
    // and triggers a side-conversation LLM dispatch. History
    // persists across step commands within the same run_auto
    // invocation so a follow-up question after a Run Step still
    // has the prior context.
    let mut qa_history: Vec<LlmMessage> = Vec::new();
    // Track the most recent sub-session this run dispatched in
    // manual mode (None after fresh start, after Reset, or after
    // an Advance moved current_step). `compute_next_manual_action`
    // reads this to pick the right successor (work -> critique,
    // critique -> advance|work, etc.).
    let mut last_subsession: Option<(String, SessionKindOut)> = None;

    let outcome = loop {
        if auto_host.shutdown_requested {
            break RunOutcome::Shutdown;
        }
        match auto_host.current_step_mode() {
            StepMode::Auto => {
                debug!("dispatching to run_auto_loop (auto mode)");
                match run_auto_loop(&opts, &mut auto_host, llm)? {
                    AutoLoopOutcome::Completed => break RunOutcome::Completed,
                    AutoLoopOutcome::FlippedToManual => {
                        debug!("auto loop flipped to manual; re-evaluating mode");
                        continue;
                    }
                    AutoLoopOutcome::Shutdown => break RunOutcome::Shutdown,
                }
            }
            StepMode::Manual => {
                debug!("dispatching to wait_for_command (manual mode)");
                // Emit a `RequestUserInput` *and* a `NextActionHint`
                // before blocking on `recv`. The RequestUserInput
                // settles the panel-side pump's drive promise; the
                // hint tells the chat panel's Continue button what
                // it would do next. Both are cheap and idempotent
                // -- emitting them on every loop iteration also
                // keeps the hint accurate between sub-sessions.
                auto_host.send(&Event::RequestUserInput {
                    prompt: None,
                    placeholder: None,
                })?;
                let last_snapshot = last_subsession.as_ref().map(|(s, k)| (s.as_str(), *k));
                emit_next_action_hint(&opts, &mut auto_host, last_snapshot)?;
                match wait_for_command(
                    &opts,
                    &mut auto_host,
                    &mut qa_history,
                    llm,
                    &mut last_subsession,
                )? {
                    ManualOutcome::Continue => continue,
                    ManualOutcome::Shutdown => break RunOutcome::Shutdown,
                    ManualOutcome::HostClosed => break RunOutcome::HostClosed,
                }
            }
        }
    };

    info!(outcome = ?outcome, "run_auto exiting");
    // Final SessionEnd. AutoPresenter forwards this to the underlying host
    // (consume_session_end is reset to false here so the user sees a
    // clean end-of-auto-run banner).
    auto_host.consume_session_end = false;
    let (reason, message) = match outcome {
        RunOutcome::Completed => (
            SessionEndReason::Completed,
            Some("auto run finished".into()),
        ),
        RunOutcome::Shutdown => (
            SessionEndReason::Completed,
            Some("orchestrator shut down".into()),
        ),
        RunOutcome::HostClosed => (
            SessionEndReason::Completed,
            Some("host disconnected".into()),
        ),
    };
    auto_host.send(&Event::SessionEnd { reason, message })?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum RunOutcome {
    /// Auto loop walked the remaining steps to the end of the flow,
    /// or a manual command sequence advanced past the last step.
    Completed,
    /// `Shutdown` host event observed; exit immediately after the
    /// current sub-session (if any) terminates.
    Shutdown,
    /// Inner host closed (read returned `None`) while parked.
    HostClosed,
}

#[derive(Debug, Clone, Copy)]
enum AutoLoopOutcome {
    /// Walked through every remaining step. Caller emits the final
    /// `SessionEnd`.
    Completed,
    /// Mode flag flipped to manual (cap exceeded, user toggle, gate
    /// failure on advance, …). Caller transitions to the parking
    /// loop.
    FlippedToManual,
    /// Shutdown observed during a sub-session. Caller exits.
    Shutdown,
}

#[derive(Debug, Clone, Copy)]
enum ManualOutcome {
    /// Command dispatched (or unrecognized event swallowed). Caller
    /// re-evaluates the mode flag and either parks again or resumes
    /// auto iteration.
    Continue,
    /// `Shutdown` host event observed while parked.
    Shutdown,
    /// Inner host closed (read returned `None`) while parked.
    HostClosed,
}

const STEP_MODE_AUTO: u8 = 0;
const STEP_MODE_MANUAL: u8 = 1;

fn step_mode_to_u8(mode: StepMode) -> u8 {
    match mode {
        StepMode::Auto => STEP_MODE_AUTO,
        StepMode::Manual => STEP_MODE_MANUAL,
    }
}

fn step_mode_from_u8(value: u8) -> StepMode {
    match value {
        STEP_MODE_AUTO => StepMode::Auto,
        _ => StepMode::Manual,
    }
}

// ---------------------------------------------------------------------
// Auto loop: walk remaining steps end-to-end.
// ---------------------------------------------------------------------

fn run_auto_loop<P, L>(
    opts: &AutoOptions,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
) -> Result<AutoLoopOutcome>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    let state = State::load(&opts.project_dir.join(".sim-flow"))?;
    let registry = registry_for(state.flow);
    let order = registry.order_for(state.flow);
    let starting = state.current_step.clone();
    let starting_idx = order.iter().position(|s| *s == starting).ok_or_else(|| {
        crate::Error::State(format!(
            "auto: current step `{starting}` is not in the {} flow",
            state.flow.as_str()
        ))
    })?;
    let remaining: Vec<&'static str> = order[starting_idx..].to_vec();

    // Resume from checkpoint when one exists for the same step.
    // The checkpoint persists `(step, critique_iters,
    // prev_blocker_count)` after every sub-session boundary, so a
    // process killed mid-retry can pick up where it left off
    // instead of redoing the prior retries from zero. Stale
    // checkpoints (different step than state.toml's current_step)
    // are ignored and cleared.
    let resumed = load_checkpoint(&opts.project_dir).filter(|c| c.step == starting);
    if let Some(c) = &resumed {
        let elapsed_min = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().saturating_sub(c.timestamp_unix) / 60)
            .unwrap_or(0);
        info!(
            step = %c.step,
            critique_iters = c.critique_iters,
            last_kind = %c.last_kind,
            elapsed_min,
            "auto: resuming from checkpoint"
        );
    } else {
        // Stale checkpoint (different step) gets cleaned up.
        clear_checkpoint(&opts.project_dir);
    }

    for (step_pos, step_id) in remaining.iter().enumerate() {
        let is_first_step = step_pos == 0;
        // Resume retry counters from checkpoint when this is the
        // first step of the resumed run AND the checkpoint matches.
        let (mut critique_iters, mut no_progress_iters, mut prev_blocker_count): (
            u32,
            u32,
            Option<usize>,
        ) = if step_pos == 0 && resumed.as_ref().map(|c| c.step.as_str()) == Some(*step_id) {
            let c = resumed.as_ref().unwrap();
            (c.critique_iters, c.no_progress_iters, c.prev_blocker_count)
        } else {
            (0, 0, None)
        };
        let step_started = std::time::Instant::now();

        loop {
            if let Some(o) = check_pre_subsession(auto_host) {
                return Ok(o);
            }

            // Plan-detail walks (DM2cd / DM3ad / DM4ad) parallelize
            // the Work phase: every pending stub is independent
            // (each reads the same predecessor docs and writes to
            // its own milestone file), so fanning out N Work sessions
            // collapses the wall-clock cost of detailing each stub.
            // Critique stays serial because all milestones share the
            // per-step JSON path. Falls through to the serial path
            // when `max_parallel_requests = 1`, there's only one
            // pending stub, or this isn't a placeholder-mode walk.
            if let Some(step) = registry.get(step_id) {
                let ran_parallel = run_plan_detail_walk_parallel(opts, step, auto_host, llm)?;
                if ran_parallel {
                    if let Some(o) = check_post_subsession(
                        auto_host,
                        step_id,
                        crate::client::SessionKind::Critique,
                        opts,
                    )? {
                        return Ok(o);
                    }
                    // Parallel path is one-shot: every pending stub
                    // got both Work and Critique. Re-check the gate
                    // and either advance or flip to manual. V1
                    // doesn't auto-retry on the parallel path
                    // because retries would force the entire fan-out
                    // to redo and the per-stub retry semantics on
                    // top of a shared per-step critique JSON path
                    // would race. Users re-run with
                    // `--max-parallel-requests 1` to fall back to
                    // the serial retry path if needed.
                    let gate_findings = read_gate_findings(&opts.project_dir, step_id);
                    if !gate_findings.is_empty() {
                        auto_host.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Error,
                            message: format!(
                                "auto: {step_id} parallel plan-detail walk produced {} \
                                 gate-failing finding(s); flipping to manual. Re-run with \
                                 `--max-parallel-requests 1` for the serial retry path.",
                                gate_findings.len()
                            ),
                        })?;
                        flip_to_manual(auto_host)?;
                        return Ok(AutoLoopOutcome::FlippedToManual);
                    }
                    try_advance_classified(&opts.project_dir, step_id, auto_host)?;
                    info!(
                        step = %step_id,
                        elapsed_s = step_started.elapsed().as_secs(),
                        "auto: step advanced (parallel plan-detail walk)"
                    );
                    clear_checkpoint(&opts.project_dir);
                    break;
                }
            }

            // Work session. The actual host's Hello was consumed in
            // `perform_initial_handshake`, so every sub-session queues
            // a synthetic one — including the first.
            let work_auto = !(opts.dm0_interactive && is_first_step && *step_id == "DM0");
            run_subsession(
                opts,
                step_id,
                crate::client::SessionKind::Work,
                work_auto,
                auto_host,
                /*consume_end=*/ true,
                /*synth_hello=*/ true,
                llm,
            )?;
            if let Some(o) =
                check_post_subsession(auto_host, step_id, crate::client::SessionKind::Work, opts)?
            {
                return Ok(o);
            }

            // Auto-tick milestone task rows whose named artifact
            // (path[::Symbol]) now exists on disk. Saves the agent
            // from spending a tool turn per task on `edit_file`
            // checkbox flips after it has already written the file.
            // Conservative: rows that don't parse as a path::sym
            // pattern are left for the agent to handle. Runs only
            // for milestone-walk steps (DM2d / DM3b / DM3c / DM4b);
            // a no-op for everything else.
            tick_milestone_checkboxes(opts, step_id);

            // Orchestrator-side cargo fmt --check + cargo clippy.
            // Writes a markdown summary the next Critique session
            // inlines. Saves the agent the 2-3 tool turns it would
            // otherwise spend invoking `run_cargo` and reasoning
            // about the output. No-op when the project has no
            // Cargo.toml.
            run_orchestrator_cargo_checks(opts, step_id);

            // Critique session.
            run_subsession(
                opts,
                step_id,
                crate::client::SessionKind::Critique,
                /*auto=*/ true,
                auto_host,
                /*consume_end=*/ true,
                /*synth_hello=*/ true,
                llm,
            )?;
            if let Some(o) = check_post_subsession(
                auto_host,
                step_id,
                crate::client::SessionKind::Critique,
                opts,
            )? {
                return Ok(o);
            }

            // Did the critique flag any gate-failing findings? If
            // yes and we have budget, loop back to work. Otherwise
            // proceed to advance.
            let gate_findings = read_gate_findings(&opts.project_dir, step_id);
            let cur_count = gate_findings.len();
            // Per-critique-pass delta. `delta` is signed because a
            // retry can introduce regressions (count strictly
            // increasing) -- worth surfacing so a stuck-loop is
            // visible without parsing the critique file.
            let delta = prev_blocker_count.map(|prev| cur_count as i64 - prev as i64);
            // No-progress accounting. A strict-decrease counts as
            // progress and resets the streak; a flat or
            // increasing count extends it. The very first pass
            // (`delta.is_none()`) does not extend the streak --
            // there's no previous count to compare against.
            match delta {
                Some(d) if d < 0 => no_progress_iters = 0,
                Some(_) => no_progress_iters = no_progress_iters.saturating_add(1),
                None => {}
            }
            // Checkpoint after each Critique boundary so a kill
            // during the next Work session resumes with the right
            // retry counters rather than starting the step over.
            save_checkpoint(
                &opts.project_dir,
                step_id,
                critique_iters,
                no_progress_iters,
                Some(cur_count),
                "critique",
            );
            tracing::info!(
                target: "sim_flow::metrics",
                event = "critique_pass",
                step = %step_id,
                pass_index = critique_iters,
                blockers = cur_count,
                prev_blockers = ?prev_blocker_count,
                delta = ?delta,
                no_progress_iters,
            );
            prev_blocker_count = Some(cur_count);
            if gate_findings.is_empty() {
                // Clean critique. Try to advance the step. For
                // milestone-walk steps, the gate may stay dirty
                // (`MilestonesAllResolved` fails) between
                // milestones -- in that case loop back here to
                // run Work + Critique for the next milestone
                // INSTEAD of breaking out and advancing the for
                // loop. This is the structural enforcement that
                // makes "one milestone at a time, critique each,
                // advance the step only when ALL milestones
                // resolved" actually happen in auto mode.
                let advance_outcome =
                    try_advance_classified(&opts.project_dir, step_id, auto_host)?;
                let step_wall_ms = step_started.elapsed().as_millis() as u64;
                let advanced = matches!(advance_outcome, AdvanceOutcome::Advanced);
                tracing::info!(
                    target: "sim_flow::metrics",
                    event = "step_end",
                    step = %step_id,
                    critique_iters,
                    advanced,
                    wall_ms = step_wall_ms,
                );
                match advance_outcome {
                    AdvanceOutcome::Advanced => {
                        // Step advanced cleanly; the checkpoint
                        // belongs to a step we're past, so wipe it.
                        clear_checkpoint(&opts.project_dir);
                        break;
                    }
                    AdvanceOutcome::MoreMilestonesPending => {
                        critique_iters = 0;
                        no_progress_iters = 0;
                        prev_blocker_count = None;
                        save_checkpoint(
                            &opts.project_dir,
                            step_id,
                            critique_iters,
                            no_progress_iters,
                            prev_blocker_count,
                            "advance-milestone",
                        );
                        auto_host.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Info,
                            message: format!(
                                "auto: {step_id} milestone-walk step has more pending milestones; \
                                 re-running work session for next milestone"
                            ),
                        })?;
                        continue;
                    }
                    AdvanceOutcome::Stuck => {
                        flip_to_manual(auto_host)?;
                        return Ok(AutoLoopOutcome::FlippedToManual);
                    }
                }
            }
            critique_iters += 1;
            // Two caps. Absolute (`max_critique_iters`) is the hard
            // backstop -- even genuinely-progressing runs stop here
            // because something is wrong if 10+ retries are needed.
            // No-progress (`max_critique_no_progress_iters`) catches
            // plateaus / oscillations early so we don't waste retries
            // on a stuck model. Both flip to manual on trip.
            let hit_absolute = critique_iters > opts.max_critique_iters;
            let hit_no_progress = opts.max_critique_no_progress_iters > 0
                && no_progress_iters > opts.max_critique_no_progress_iters;
            if hit_absolute || hit_no_progress {
                let reason = if hit_no_progress && !hit_absolute {
                    format!(
                        "auto: {} critique made no progress for {} consecutive retry(ies) (blocker count {} did not strictly decrease); flipping to manual mode. \
                         Total retries this step: {}/{}. Raise `sim-flow.auto.maxCritiqueNoProgressIterations` to allow more plateau retries.",
                        step_id,
                        no_progress_iters,
                        gate_findings.len(),
                        critique_iters - 1,
                        opts.max_critique_iters,
                    )
                } else {
                    format!(
                        "auto: {} critique still has {} gate-failing finding(s) after {} retries; flipping to manual mode. \
                         Use the dashboard's per-step controls to inspect, re-run, or advance. Raise \
                         `sim-flow.auto.maxCritiqueIterations` and toggle back to auto if you want more retries per resume \
                         (current cap: {}).",
                        step_id,
                        gate_findings.len(),
                        critique_iters - 1,
                        opts.max_critique_iters,
                    )
                };
                auto_host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: reason,
                })?;
                flip_to_manual(auto_host)?;
                return Ok(AutoLoopOutcome::FlippedToManual);
            }
            // Retry diagnostic: include the no-progress streak so an
            // operator watching the run sees an early signal when
            // the model has plateaued rather than only learning
            // about it at the cap.
            let progress_note = match delta {
                Some(d) if d < 0 => format!(" (-{} from prior pass)", -d),
                Some(0) => format!(
                    " (no progress, streak {}/{})",
                    no_progress_iters, opts.max_critique_no_progress_iters
                ),
                Some(d) => format!(
                    " (+{} from prior pass, streak {}/{})",
                    d, no_progress_iters, opts.max_critique_no_progress_iters
                ),
                None => String::new(),
            };
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Info,
                message: format!(
                    "auto: {} critique reported {} gate-failing finding(s); re-running work (retry {}/{}{})",
                    step_id,
                    gate_findings.len(),
                    critique_iters,
                    opts.max_critique_iters,
                    progress_note,
                ),
            })?;
            // Loop body re-runs work; the orchestrator's
            // build_session_inputs will inline the critique file so
            // the agent sees the findings.
        }
        // Inner loop break-only path: clean critique advanced the
        // step. Outer for-loop moves to the next step.
    }

    // Clean exit: every step advanced. The checkpoint belongs to
    // a finished run; remove it so a future invocation against
    // the same project starts fresh.
    clear_checkpoint(&opts.project_dir);
    Ok(AutoLoopOutcome::Completed)
}

fn check_pre_subsession<P: Presenter + ?Sized>(
    auto_host: &AutoPresenter<P>,
) -> Option<AutoLoopOutcome> {
    if auto_host.shutdown_requested {
        return Some(AutoLoopOutcome::Shutdown);
    }
    if matches!(auto_host.current_step_mode(), StepMode::Manual) {
        return Some(AutoLoopOutcome::FlippedToManual);
    }
    None
}

fn check_post_subsession<P: Presenter + ?Sized>(
    auto_host: &mut AutoPresenter<P>,
    step_id: &str,
    kind: crate::client::SessionKind,
    opts: &AutoOptions,
) -> Result<Option<AutoLoopOutcome>> {
    if auto_host.shutdown_requested {
        return Ok(Some(AutoLoopOutcome::Shutdown));
    }
    if auto_host.cap_exceeded {
        emit_cap_exceeded_diagnostic(auto_host, step_id, kind, opts)?;
        flip_to_manual(auto_host)?;
        return Ok(Some(AutoLoopOutcome::FlippedToManual));
    }
    if matches!(auto_host.current_step_mode(), StepMode::Manual) {
        return Ok(Some(AutoLoopOutcome::FlippedToManual));
    }
    Ok(None)
}

/// Auto-tick milestone task rows whose named artifact (`path` or
/// `path::Symbol`) now resolves on disk. Wraps the pure
/// `crate::__internal::steps::tick_resolved_milestone_tasks` with the
/// step lookup + a tracing emit so a flipped count is visible in the
/// metrics stream.
fn tick_milestone_checkboxes(opts: &AutoOptions, step_id: &str) {
    let dot = opts.project_dir.join(".sim-flow");
    let state = match State::load(&dot) {
        Ok(s) => s,
        Err(_) => return,
    };
    let registry = crate::__internal::steps::registry_for(state.flow);
    let Some(step) = registry.get(step_id) else {
        return;
    };
    let flipped = crate::__internal::steps::tick_resolved_milestone_tasks(&opts.project_dir, step);
    if flipped > 0 {
        tracing::info!(
            target: "sim_flow::metrics",
            event = "milestone_tasks_auto_ticked",
            step = %step_id,
            flipped,
        );
    }
}

/// Run `cargo fmt --check` + `cargo clippy` after the Work session,
/// stash a markdown summary at
/// `.sim-flow/cargo-checks-{step_id}.md`, and emit a tracing event
/// with the pass/fail outcome. The Critique session inlines that file
/// via `build_session_inputs`, so the agent sees an objective lint /
/// build / fmt signal instead of relying on the Work session's
/// self-report. No-op when the project has no `Cargo.toml` (early DM
/// steps before any code has landed). The file path is overwritten on
/// each call so each milestone's report is fresh.
fn run_orchestrator_cargo_checks(opts: &AutoOptions, step_id: &str) {
    let report = match crate::__internal::session::runners::run_post_work_cargo(&opts.project_dir) {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(
                target: "sim_flow::metrics",
                event = "post_work_cargo_failed",
                step = %step_id,
                error = %err,
            );
            return;
        }
    };
    let path = opts
        .project_dir
        .join(".sim-flow")
        .join(format!("cargo-checks-{step_id}.md"));
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if std::fs::write(&path, report.render_markdown()).is_err() {
        return;
    }
    tracing::info!(
        target: "sim_flow::metrics",
        event = "post_work_cargo_checks",
        step = %step_id,
        fmt_ok = report.fmt_ok,
        clippy_ok = report.clippy_ok,
        all_clean = report.all_clean(),
    );
}

/// Mid-step checkpoint persisted to `.sim-flow/checkpoint.json`
/// after every sub-session boundary so an interrupted auto run can
/// resume retry counters instead of starting each step from
/// scratch. Disk state (milestone-NN-*.md ticks, critique JSONs,
/// cargo-checks.md) is the primary checkpoint; this file just
/// captures the in-memory loop counters that would otherwise be
/// lost when the process dies.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutoCheckpoint {
    /// Step the auto loop was processing when the checkpoint was
    /// written. On restart, the resume logic only restores
    /// counters when `state.toml::current_step` matches this --
    /// stale checkpoints from a different step are ignored.
    step: String,
    /// Cross-session retry count for the current step. Restored
    /// on resume so a process that died mid-retry doesn't spin
    /// `max_critique_iters` extra retries.
    critique_iters: u32,
    /// Consecutive no-progress retry count. Counts critique
    /// passes whose blocker count did NOT strictly decrease from
    /// the previous pass. Resets to 0 on strict progress.
    /// Restored on resume so the no-progress cap doesn't get a
    /// free reset after a process restart.
    /// `#[serde(default)]` so old checkpoints (written before the
    /// no-progress cap landed) still load.
    #[serde(default)]
    no_progress_iters: u32,
    /// Last critique's blocker count, for the delta-tracking
    /// metric. Restored on resume so the post-restart pass can
    /// still compute a sensible delta.
    prev_blocker_count: Option<usize>,
    /// Last sub-session kind written ("work" or "critique"). Used
    /// only for diagnostic logging on resume.
    last_kind: String,
    /// Wall-clock timestamp (seconds since epoch) of the
    /// checkpoint write. Lets the resume log emit a "resuming
    /// from N minutes ago" line so the user can sanity-check.
    timestamp_unix: u64,
}

fn checkpoint_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join(".sim-flow").join("checkpoint.json")
}

fn save_checkpoint(
    project_dir: &Path,
    step: &str,
    critique_iters: u32,
    no_progress_iters: u32,
    prev_blocker_count: Option<usize>,
    last_kind: &str,
) {
    let timestamp_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let checkpoint = AutoCheckpoint {
        step: step.to_string(),
        critique_iters,
        no_progress_iters,
        prev_blocker_count,
        last_kind: last_kind.to_string(),
        timestamp_unix,
    };
    let path = checkpoint_path(project_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(&checkpoint) {
        let _ = std::fs::write(&path, body);
    }
}

fn load_checkpoint(project_dir: &Path) -> Option<AutoCheckpoint> {
    let path = checkpoint_path(project_dir);
    let body = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Wipe the checkpoint when the auto loop advances to the next
/// step (the prior step's counters are irrelevant once we're past
/// it) or when run_auto exits cleanly.
fn clear_checkpoint(project_dir: &Path) {
    let _ = std::fs::remove_file(checkpoint_path(project_dir));
}

fn flip_to_manual<P: Presenter + ?Sized>(auto_host: &mut AutoPresenter<P>) -> Result<()> {
    let prev = auto_host.current_step_mode();
    auto_host.store_step_mode(StepMode::Manual);
    if !matches!(prev, StepMode::Manual) {
        warn!("flipping step mode auto -> manual (cap exceeded or gate failure)");
        auto_host.send(&Event::StepModeChanged {
            mode: StepMode::Manual,
        })?;
    }
    Ok(())
}

fn emit_cap_exceeded_diagnostic<P: Presenter + ?Sized>(
    auto_host: &mut AutoPresenter<P>,
    step_id: &str,
    kind: crate::client::SessionKind,
    opts: &AutoOptions,
) -> Result<()> {
    let kind_s = match kind {
        crate::client::SessionKind::Work => "work",
        crate::client::SessionKind::Critique => "critique",
    };
    auto_host.send(&Event::Diagnostic {
        level: DiagnosticLevel::Error,
        message: format!(
            "auto: {step_id} {kind_s} session hit the per-session iteration cap ({}); flipping to manual mode. \
             Use the dashboard's per-step controls to inspect, re-run, or advance. Raise \
             `sim-flow.auto.maxWorkIterations` and toggle back to auto if you want more work-side iterations \
             per resume; the critique-side cap is {}.",
            opts.max_auto_iters, opts.max_critique_iters,
        ),
    })?;
    Ok(())
}

/// Hello / HelloAck handshake. Run exactly once per `run_auto`
/// invocation, before either the auto-iteration loop or the manual
/// parking loop. The HelloAck's `session` / `step_descriptor` reflect
/// `current_step` with `Work` as the kind — sub-sessions launched
/// later (auto-mode iteration or manual `RunStep` / `RunCritique`)
/// each queue a synthetic Hello internally and fire a fresh HelloAck
/// with their own kind, so this initial pair is purely a connection-
/// established signal for the host (the dashboard renders the banner
/// from the first HelloAck and ignores subsequent ones for sub-
/// session boundaries).
fn perform_initial_handshake<P: Presenter + ?Sized>(
    opts: &AutoOptions,
    auto_host: &mut AutoPresenter<P>,
) -> Result<()> {
    let hello_version = match auto_host.recv()? {
        Some(HostEvent::Hello {
            protocol_version, ..
        }) => protocol_version,
        Some(other) => {
            auto_host.send(&Event::SessionEnd {
                reason: SessionEndReason::ProtocolError,
                message: Some(format!("expected Hello first, got {other:?}")),
            })?;
            return Err(crate::Error::Protocol(format!(
                "expected Hello first, got {other:?}"
            )));
        }
        None => {
            return Err(crate::Error::HostClosed("before Hello".into()));
        }
    };
    if hello_version != PROTOCOL_VERSION {
        auto_host.send(&Event::SessionEnd {
            reason: SessionEndReason::ProtocolMismatch,
            message: Some(format!(
                "host sent protocolVersion={hello_version}; orchestrator speaks {PROTOCOL_VERSION}"
            )),
        })?;
        return Err(crate::Error::ProtocolVersionMismatch {
            host: hello_version,
            orchestrator: PROTOCOL_VERSION.into(),
        });
    }

    let dot = opts.project_dir.join(".sim-flow");
    let state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(&state.current_step).ok_or_else(|| {
        crate::Error::State(format!(
            "auto: current_step `{}` is not in the {} registry",
            state.current_step,
            state.flow.as_str()
        ))
    })?;
    let descriptor =
        step_descriptor_for_protocol(step, SessionKindOut::Work, &opts.foundation_root);
    auto_host.send(&Event::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        sim_flow_version: env!("CARGO_PKG_VERSION").into(),
        session: SessionTag {
            step: step.id.into(),
            kind: SessionKindOut::Work,
            candidate: None,
        },
        step_descriptor: descriptor,
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_subsession<P, L>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto: bool,
    host: &mut AutoPresenter<P>,
    consume_end: bool,
    synth_hello: bool,
    llm: &mut L,
) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    run_subsession_scoped(
        opts,
        step_id,
        kind,
        auto,
        host,
        consume_end,
        synth_hello,
        llm,
        None,
    )
}

/// `run_subsession` with milestone pinning. Used by
/// [`run_plan_detail_walk_parallel`] so each worker thread's session
/// targets the exact stub it was assigned via
/// `OrchestratorOptions::milestone_name`. Identical to
/// `run_subsession` in every other respect.
#[allow(clippy::too_many_arguments)]
fn run_subsession_scoped<P, L>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto: bool,
    host: &mut AutoPresenter<P>,
    consume_end: bool,
    synth_hello: bool,
    llm: &mut L,
    milestone_name: Option<String>,
) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    if synth_hello {
        host.queue_synthetic_hello();
    }
    host.consume_session_end = consume_end;
    host.cap_exceeded = false;
    host.in_subsession = true;
    host.in_subsession_parked = false;
    let kind_out = session_kind_to_protocol(kind);
    info!(step = step_id, kind = ?kind_out, auto, "sub-session starting");
    // Bracket the inner run_session with SubSessionStarted /
    // SubSessionEnded so the dashboard can disable per-step
    // buttons while the orchestrator is busy. Emitted regardless
    // of step-mode; auto mode drives multiple bracketed sub-
    // sessions per outer run, manual mode drives one per
    // dispatched command.
    host.send(&Event::SubSessionStarted {
        step: step_id.to_string(),
        kind: kind_out,
    })?;
    // Route the LLM stack: `opts.llm_*` is the routing metadata
    // logged with metrics and surfaced in diagnostics, but the actual
    // dispatch goes through the injected `llm` adapter. The
    // `critique_llm_*` / `qa_llm_*` overrides are informational only
    // since the doc-endorsed simplification has run_auto take a
    // single adapter for every sub-session.
    let routing = resolve_llm_for_kind(opts, kind);
    let session_opts = OrchestratorOptions {
        project_dir: opts.project_dir.clone(),
        foundation_root: opts.foundation_root.clone(),
        step_id: step_id.to_string(),
        kind,
        candidate: None,
        llm_backend: routing.backend,
        llm_model: routing.model,
        llm_model_family_id: routing.model_family_id,
        llm_runtime_profile_id: routing.runtime_profile_id,
        llm_debug_adaptation: opts.llm_debug_adaptation,
        llm_base_url: routing.base_url,
        auto,
        max_auto_iters: opts.max_auto_iters,
        max_llm_requests: opts.max_llm_requests,
        max_identical_responses: opts.max_identical_responses,
        // JSONL host path: the orchestrator extracts fenced
        // ` ```<path>` blocks from each turn and writes them. Use
        // the artifact-write convention.
        agent_has_native_fs_tools: false,
        no_preamble: opts.no_preamble,
        // Pinned only when this sub-session was dispatched by the
        // parallel plan-detail walk path via run_subsession_scoped.
        // Serial walkers pass None and the orchestrator falls back
        // to find_current_milestone's heuristics.
        milestone_name,
    };
    let result = run_session(session_opts, host, llm);
    host.in_subsession = false;
    host.in_subsession_parked = false;
    // run_session returns Ok(()) for both clean completion and
    // user-initiated Cancel (the Cancel path emits its own internal
    // SessionEnd and returns Ok). Err is genuine protocol / I/O /
    // state error — surface that as "error" so the dashboard can
    // distinguish.
    let outcome = if result.is_ok() { "completed" } else { "error" };
    info!(step = step_id, kind = ?kind_out, outcome, "sub-session ended");
    // Best-effort: if writing the closing event fails (e.g. host
    // socket already closed), keep the inner `result` to surface.
    let _ = host.send(&Event::SubSessionEnded {
        step: step_id.to_string(),
        kind: kind_out,
        outcome: outcome.into(),
    });
    result
}

/// `Presenter` shim used by `run_plan_detail_walk_parallel`'s worker
/// threads. Forwards every outgoing event into an `mpsc::Sender`
/// (drained by the coordinator on the main thread) and reports
/// clean channel close on every `recv()` -- the orchestrator's
/// handshake Hello is supplied by `AutoPresenter::queue_synthetic_hello`
/// (the OUTER presenter wrapping this one). Auto-mode work
/// sessions never call recv mid-session today; if any future
/// feature does, returning None here correctly signals "host
/// channel closed" without injecting a phantom Hello on top of
/// the handshake one.
struct ChannelPresenter {
    tx: std::sync::mpsc::Sender<Event>,
}

impl ChannelPresenter {
    fn new(tx: std::sync::mpsc::Sender<Event>) -> Self {
        Self { tx }
    }
}

impl Presenter for ChannelPresenter {
    fn send(&mut self, event: &Event) -> Result<()> {
        // Best-effort: if the coordinator has dropped its receiver
        // (orchestrator-side error already in flight), don't override
        // that error with a SendError. The first error wins.
        let _ = self.tx.send(event.clone());
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<HostEvent>> {
        // No real host on the other end of the channel; the only
        // Hello the orchestrator ever sees here is the one
        // AutoPresenter queues synthetically. Subsequent recvs
        // returning None correctly signal "host channel closed".
        Ok(None)
    }
}

/// Borrowing `LlmAdapter` wrapper. Each worker thread in the
/// parallel plan-detail walk holds an owned `RefAdapter<'a, L>`
/// that captures a shared reference into the main-thread `&L`,
/// satisfying `run_subsession_scoped`'s `&mut L: LlmAdapter` bound
/// without forcing a Send+Sync `Arc<dyn LlmAdapter>` at every call
/// site. Works because `LlmAdapter::dispatch` is already `&self`
/// and the trait requires `Send + Sync`.
struct RefAdapter<'a, L: LlmAdapter + ?Sized>(&'a L);

impl<'a, L: LlmAdapter + ?Sized> LlmAdapter for RefAdapter<'a, L> {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn dispatch(
        &self,
        messages: &[LlmMessage],
    ) -> Result<(String, crate::session::agent::LlmCallMetrics)> {
        self.0.dispatch(messages)
    }
    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[crate::session::agent::ToolAdvertise],
    ) -> Result<(
        String,
        Vec<crate::session::agent::AdvertisedToolCall>,
        crate::session::agent::LlmCallMetrics,
    )> {
        self.0.dispatch_with_tools(messages, tools)
    }
    fn adaptation_summary(&self) -> Option<crate::session::agent::AgentAdaptationSummary> {
        self.0.adaptation_summary()
    }
}

/// Parallel plan-detail walk dispatcher. For steps with a
/// placeholder-marker milestone walk (DM2cd / DM3ad / DM4ad) where
/// every pending stub is independent (each one reads the same
/// predecessor docs and writes only its own milestone file), fan out
/// Work sessions across worker threads up to `max_parallel_requests`.
/// Critique sessions stay serial because they all write to the
/// shared `docs/critiques/<step>-critique.json` path.
///
/// Returns `Ok(true)` when the parallel path ran to completion (Work
/// for every stub, Critique for every stub, all reaching session
/// end without error). Caller re-checks the step gate; on a clean
/// gate the step advances, on a dirty gate the caller decides
/// whether to retry. Returns `Ok(false)` when the parallel path was
/// not applicable (single pending stub, `max_parallel_requests = 1`,
/// or this isn't a placeholder-mode walk) and the caller should
/// fall back to the existing serial walker.
fn run_plan_detail_walk_parallel<P, L>(
    opts: &AutoOptions,
    step: &crate::__internal::steps::StepDescriptor,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
) -> Result<bool>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    // Only placeholder-mode walks parallelize today.
    let walk = match step.milestone_walk {
        Some(w) if w.placeholder_marker.is_some() => w,
        _ => return Ok(false),
    };
    // `max_parallel_requests == 1` is the explicit "use serial path"
    // setting.
    if opts.max_parallel_requests == 1 {
        return Ok(false);
    }
    let pending =
        match crate::__internal::steps::enumerate_pending_milestones(&opts.project_dir, &walk) {
            crate::__internal::steps::PendingMilestones::Present { pending } => pending,
            crate::__internal::steps::PendingMilestones::DirectoryMissing => {
                // Probable setup error (e.g. user pointed the
                // orchestrator at the wrong project dir, or DM2c's
                // impl-plan output was never produced). Fall through
                // to the serial walker so it surfaces the standard
                // "no milestones" diagnostic on the established path
                // instead of silently treating "missing" as
                // "nothing-to-do."
                return Ok(false);
            }
        };
    // Invalidate any pre-existing per-step critique JSON before
    // we re-enter the walk. Two scenarios this addresses:
    //
    // (a) Crash between Phase 1 and Phase 2: Phase 2 may have
    //     written a partial critique (milestone-01 critiqued,
    //     orchestrator died before milestone-02). On resume,
    //     pending.len() == 0 (Phase 1 done) and we fall through
    //     to the serial walker, which reads the stale JSON and
    //     may advance against a critique nobody re-validated.
    // (b) User manually retrying after a previous dirty-gate run:
    //     the prior critique was against OLDER code; the code has
    //     since been edited, so the prior findings are no longer
    //     authoritative. A fresh critique sweep is what the user
    //     wants.
    //
    // Deleting unconditionally on parallel-walker entry handles
    // both: Phase 2 below will write a fresh JSON if it runs, and
    // if we fall through to the serial walker its gate evaluation
    // sees "no critique yet" rather than a stale one.
    let critiques_dir = opts.project_dir.join("docs/critiques");
    let critique_json = critiques_dir.join(format!("{step_id}-critique.json", step_id = step.id));
    if critique_json.exists() {
        let _ = std::fs::remove_file(&critique_json);
        let critique_md = critiques_dir.join(format!("{step_id}-critique.md", step_id = step.id));
        let _ = std::fs::remove_file(&critique_md);
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: format!(
                "auto: {step_id} parallel plan-detail re-entry invalidated prior critique \
                 JSON to avoid acting on stale findings from a previous run",
                step_id = step.id,
            ),
        })?;
    }
    if pending.len() < 2 {
        // One or zero pending stubs: no parallelism win; let the
        // serial walker handle it (it has the established retry
        // semantics). With the prior critique already invalidated
        // above, the serial walker's gate evaluation will require
        // a fresh critique pass before advancing.
        return Ok(false);
    }
    let cap_raw = opts.max_parallel_requests as usize;
    let cap = if cap_raw == 0 {
        pending.len()
    } else {
        cap_raw.min(pending.len())
    };

    let step_id_owned = step.id.to_string();
    let n_pending = pending.len();
    auto_host.send(&Event::Diagnostic {
        level: DiagnosticLevel::Info,
        message: format!(
            "auto: {step_id_owned} parallel plan-detail walk: {n_pending} pending stub(s), \
             dispatching up to {cap} concurrent Work sessions"
        ),
    })?;

    // PHASE 1: parallel Work fan-out.
    // Shared queue of stubs the workers pull from; `pop()` so workers
    // pick up stubs in reverse order (cheap, ordering doesn't matter
    // here -- each stub is independent).
    let queue = Arc::new(std::sync::Mutex::new(pending.clone()));
    let (tx, rx) = std::sync::mpsc::channel::<Event>();

    // `thread::scope` lets the workers borrow `&L` from the main
    // thread without `'static` requirements. Workers send events
    // through `tx`; the coordinator drains them on the main thread
    // and forwards to `auto_host` so the dashboard sees per-worker
    // SubSessionStarted / SubSessionEnded brackets and Diagnostic
    // messages in arrival order.
    let llm_shared: &L = &*llm;
    let opts_ref: &AutoOptions = opts;
    let step_id_ref: &str = &step_id_owned;
    // Shared "stop dispatching new milestones" flag. The coordinator
    // sets this when it forwards a max_auto_iters / max_critique_iters
    // Diagnostic; workers check it before pulling the next stub off
    // the queue so they don't continue burning compute past the cap.
    // In-flight worker sessions complete normally (their own
    // AutoPresenter already responds to the diagnostic with a Cancel).
    let cap_exceeded_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let work_result: Result<()> = std::thread::scope(|scope| -> Result<()> {
        let mut handles: Vec<std::thread::ScopedJoinHandle<'_, Result<()>>> =
            Vec::with_capacity(cap);
        for _ in 0..cap {
            let tx_w = tx.clone();
            let queue_w = Arc::clone(&queue);
            let opts_w = opts_ref;
            let step_id_w = step_id_ref;
            let llm_w = llm_shared;
            let cap_flag_w = Arc::clone(&cap_exceeded_flag);
            handles.push(scope.spawn(move || -> Result<()> {
                loop {
                    if cap_flag_w.load(std::sync::atomic::Ordering::Acquire) {
                        // Cap fired elsewhere; stop pulling new
                        // milestones so we don't continue running
                        // past the cap. Any sessions already in
                        // flight complete on their own.
                        break;
                    }
                    let stub = queue_w.lock().unwrap().pop();
                    let Some(milestone_rel) = stub else { break };
                    let bare_name = std::path::Path::new(&milestone_rel)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                        .unwrap_or_else(|| milestone_rel.clone());
                    let mut sink = ChannelPresenter::new(tx_w.clone());
                    let mode = Arc::new(AtomicU8::new(step_mode_to_u8(opts_w.step_mode)));
                    let mut worker_host = AutoPresenter::new(&mut sink, mode);
                    let mut adapter = RefAdapter(llm_w);
                    run_subsession_scoped(
                        opts_w,
                        step_id_w,
                        crate::client::SessionKind::Work,
                        /*auto=*/ true,
                        &mut worker_host,
                        /*consume_end=*/ true,
                        /*synth_hello=*/ true,
                        &mut adapter,
                        Some(bare_name),
                    )?;
                }
                Ok(())
            }));
        }
        // Drop the original sender so the channel closes when every
        // worker's clone is dropped at thread exit.
        drop(tx);
        // Coordinator: forward worker events to the host serially.
        // host.send is fallible (transport-closed). Capture the
        // first such error into `first_err` rather than propagating
        // via `?` -- bailing here would skip the handle-join loop
        // below, and `thread::scope`'s auto-join would silently
        // discard every worker's `Result` and panic payload.
        // Continue draining the channel so workers can finish
        // emitting (their `send` is best-effort and won't block
        // when the receiver is still alive).
        let mut first_err: Option<crate::Error> = None;
        while let Ok(event) = rx.recv() {
            // Signal workers to stop pulling new milestones the
            // moment any worker emits the auto-cap diagnostic.
            // Match the same string AutoPresenter's send watches
            // for (line ~3096) so the parallel and serial paths
            // agree on what "cap exceeded" means.
            if let Event::Diagnostic { level, message } = &event
                && matches!(level, DiagnosticLevel::Error)
                && (message.contains("max_auto_iters") || message.contains("max_critique_iters"))
                && !cap_exceeded_flag.load(std::sync::atomic::Ordering::Acquire)
            {
                cap_exceeded_flag.store(true, std::sync::atomic::Ordering::Release);
            }
            if first_err.is_some() {
                // Already in shutdown; drop subsequent events
                // rather than re-trying host.send (which would
                // likely fail again and clobber the original
                // error).
                continue;
            }
            if let Err(e) = auto_host.send(&event) {
                first_err = Some(e);
            }
        }
        // Join workers; surface the first worker Err / panic if
        // the rx loop didn't already report one.
        for h in handles {
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(panic) => {
                    let msg = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(|s| s.as_str()))
                        .unwrap_or("(unknown panic payload)");
                    if first_err.is_none() {
                        first_err = Some(crate::Error::State(format!(
                            "parallel plan-detail worker panicked: {msg}"
                        )));
                    }
                }
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(())
    });
    work_result?;

    // PHASE 2: serial Critique for each (originally pending) stub.
    //
    // Critiques write to a single per-step `docs/critiques/<step>-critique.json`
    // path (the critique prompts hard-code it). Running N critiques back
    // to back means each one overwrites the previous; only the last
    // critique's findings survive on disk for the caller to read. If
    // milestone-1's critique fires a BLOCKER but milestone-N's is clean,
    // the caller's `read_gate_findings` call would see an empty list and
    // advance the step incorrectly.
    //
    // To keep the V1 "no auto-retry" contract while not losing findings:
    // read the gate-findings file after each iteration, and break out of
    // the loop on the first non-empty result. The blocking findings stay
    // on disk for the caller's gate evaluation; the caller flips to
    // manual on a dirty gate the same way it would for a single-critique
    // path.
    for milestone_rel in &pending {
        let bare_name = std::path::Path::new(milestone_rel)
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_else(|| milestone_rel.clone());
        run_subsession_scoped(
            opts,
            step_id_ref,
            crate::client::SessionKind::Critique,
            /*auto=*/ true,
            auto_host,
            /*consume_end=*/ true,
            /*synth_hello=*/ true,
            llm,
            Some(bare_name),
        )?;
        let findings = read_gate_findings(&opts.project_dir, step_id_ref);
        if !findings.is_empty() {
            // Halt: this critique's findings survive on disk so the
            // caller sees them. Continuing would let a clean
            // milestone-K critique overwrite the blockers.
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Info,
                message: format!(
                    "auto: {step_id_ref} parallel plan-detail Phase 2 halted on \
                     milestone `{milestone_rel}` with {n} gate finding(s); \
                     remaining milestones' critiques deferred to manual",
                    n = findings.len(),
                ),
            })?;
            break;
        }
    }
    Ok(true)
}

pub(crate) fn session_kind_to_protocol(kind: crate::client::SessionKind) -> SessionKindOut {
    match kind {
        crate::client::SessionKind::Work => SessionKindOut::Work,
        crate::client::SessionKind::Critique => SessionKindOut::Critique,
    }
}

/// Effective LLM stack for one sub-session, resolved from
/// `AutoOptions` against the session `kind`.
///
/// Routing:
///   - `SessionKind::Work` -> the primary `llm_*` fields, unchanged.
///   - `SessionKind::Critique` -> each `critique_llm_*` field when
///     set, with per-field fallback to the matching `llm_*` field
///     when unset. `critique_llm_backend` falls back unconditionally
///     (it's the routing key); the rest fall back via `or_else`.
///
/// Q&A and other future kinds always use the work-side stack today;
/// add a dedicated knob if/when it becomes worth modeling.
pub(crate) struct ResolvedLlmConfig {
    pub backend: String,
    pub model: Option<String>,
    pub model_family_id: Option<String>,
    pub runtime_profile_id: Option<String>,
    pub base_url: Option<String>,
}

pub(crate) fn resolve_llm_for_kind(
    opts: &AutoOptions,
    kind: crate::client::SessionKind,
) -> ResolvedLlmConfig {
    let is_critique = matches!(kind, crate::client::SessionKind::Critique);
    if is_critique {
        ResolvedLlmConfig {
            backend: opts
                .critique_llm_backend
                .clone()
                .unwrap_or_else(|| opts.llm_backend.clone()),
            model: opts
                .critique_llm_model
                .clone()
                .or_else(|| opts.llm_model.clone()),
            model_family_id: opts
                .critique_llm_model_family_id
                .clone()
                .or_else(|| opts.llm_model_family_id.clone()),
            runtime_profile_id: opts
                .critique_llm_runtime_profile_id
                .clone()
                .or_else(|| opts.llm_runtime_profile_id.clone()),
            base_url: opts
                .critique_llm_base_url
                .clone()
                .or_else(|| opts.llm_base_url.clone()),
        }
    } else {
        ResolvedLlmConfig {
            backend: opts.llm_backend.clone(),
            model: opts.llm_model.clone(),
            model_family_id: opts.llm_model_family_id.clone(),
            runtime_profile_id: opts.llm_runtime_profile_id.clone(),
            base_url: opts.llm_base_url.clone(),
        }
    }
}

/// Per-field-fallback resolver for the idle-state Q&A LLM stack.
/// Mirrors `resolve_llm_for_kind` for the Critique kind but reads
/// the `qa_llm_*` fields off `AutoOptions`. Used by the Q&A turn
/// emit point in `run_manual_qa_turn` -- Q&A doesn't go through
/// `run_subsession` (it's not a sub-session in the structural
/// sense), so the resolver runs separately at the dispatch point.
pub(crate) fn resolve_llm_for_qa(opts: &AutoOptions) -> ResolvedLlmConfig {
    ResolvedLlmConfig {
        backend: opts
            .qa_llm_backend
            .clone()
            .unwrap_or_else(|| opts.llm_backend.clone()),
        model: opts.qa_llm_model.clone().or_else(|| opts.llm_model.clone()),
        model_family_id: opts
            .qa_llm_model_family_id
            .clone()
            .or_else(|| opts.llm_model_family_id.clone()),
        runtime_profile_id: opts
            .qa_llm_runtime_profile_id
            .clone()
            .or_else(|| opts.llm_runtime_profile_id.clone()),
        base_url: opts
            .qa_llm_base_url
            .clone()
            .or_else(|| opts.llm_base_url.clone()),
    }
}

/// Outcome of `try_advance_classified`. Distinguishes "gate clean,
/// advanced" from "gate dirty only because more milestones are
/// pending in a milestone-walk step (loop back to Work)" from "gate
/// dirty for a real reason (flip to manual)".
#[derive(Debug, Clone, PartialEq, Eq)]
enum AdvanceOutcome {
    Advanced,
    MoreMilestonesPending,
    Stuck,
}

fn try_advance_classified<P: Presenter + ?Sized>(
    project_dir: &Path,
    step_id: &str,
    host: &mut AutoPresenter<P>,
) -> Result<AdvanceOutcome> {
    use crate::gate;
    let dot = project_dir.join(".sim-flow");
    let state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(step_id).ok_or_else(|| {
        crate::Error::InvalidStep(format!("{} is not a {} step", step_id, state.flow.as_str()))
    })?;
    let report = gate::evaluate(project_dir, &step.gate_checks)?;
    if report.is_clean() {
        // Re-use the existing advance helper to do the bookkeeping
        // (mark passed, bump current_step, git commit).
        let advanced = try_advance(project_dir, step_id, host)?;
        return Ok(if advanced {
            AdvanceOutcome::Advanced
        } else {
            // The advance helper re-evaluates the gate; if it sees
            // dirty there but we saw clean here, the world changed
            // mid-evaluation. Treat as Stuck.
            AdvanceOutcome::Stuck
        });
    }
    // Gate dirty. For milestone-walk steps, classify whether ANY
    // failing check is the MilestonesAllResolved one -- if so, the
    // agent is mid-walk and the next iteration should be a fresh
    // Work session for the next milestone. Previously this required
    // EVERY failure to be milestone-pending, which gave up too eagerly:
    // most step gates have shell checks like `cargo test --test X`
    // or `grep -r SymbolY src` that the agent only satisfies in
    // (often) the LAST milestone. When such a check failed at the
    // same time as a milestone-pending check, we returned Stuck
    // instead of letting the walk continue. Loosening to `any` keeps
    // the walk going; once all milestones land, only the genuine
    // structural failures remain and we (correctly) return Stuck
    // then. MILESTONE_WALK_CAP in the caller bounds total iterations.
    let any_milestone_pending = step.milestone_walk.is_some()
        && !report.failures.is_empty()
        && report.failures.iter().any(|f| {
            // The MilestonesAllResolved gate check uses two
            // distinct failure reason strings depending on mode:
            //   - Execution-mode (DM2d / DM3b / DM3c / DM4b): the
            //     reason starts with "milestone files still have
            //     unresolved rows" (some `- [ ]` rows remain).
            //   - Detail-mode (DM2cd / DM3ad / DM4ad): the reason
            //     starts with "milestone stubs not yet detailed"
            //     (the placeholder marker is still in the body).
            // Both are valid "loop back to next milestone's Work
            // session" signals. The third match handles the
            // empty-directory edge case.
            f.reason
                .contains("milestone files still have unresolved rows")
                || f.reason.contains("milestone stubs not yet detailed")
                || f.reason.contains("no `") && f.reason.contains("NN-*.md` files found")
        });
    if any_milestone_pending {
        return Ok(AdvanceOutcome::MoreMilestonesPending);
    }
    host.send(&Event::Diagnostic {
        level: DiagnosticLevel::Error,
        message: format!(
            "auto: {step_id} gate is not clean after critique; cannot advance. {} failure(s).",
            report.failures.len()
        ),
    })?;
    for f in &report.failures {
        host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("  - {}: {}", f.description, f.reason),
        })?;
    }
    Ok(AdvanceOutcome::Stuck)
}

fn try_advance<P: Presenter + ?Sized>(
    project_dir: &Path,
    step_id: &str,
    host: &mut AutoPresenter<P>,
) -> Result<bool> {
    use crate::gate;
    let dot = project_dir.join(".sim-flow");
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(step_id).ok_or_else(|| {
        crate::Error::InvalidStep(format!("{} is not a {} step", step_id, state.flow.as_str()))
    })?;
    let report = gate::evaluate(project_dir, &step.gate_checks)?;
    if !report.is_clean() {
        host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!(
                "auto: {step_id} gate is not clean after critique; cannot advance. {} failure(s).",
                report.failures.len()
            ),
        })?;
        return Ok(false);
    }
    let order = registry.order_for(state.flow);
    let next = order
        .iter()
        .position(|s| *s == step.id)
        .and_then(|idx| order.get(idx + 1).copied());

    // Auto-render the block diagram on the DM2d -> DM3a boundary
    // so DM3a (test-plan author) and downstream readers can see
    // the topology the model just landed without manually clicking
    // the dashboard's Render button. Failures are surfaced as
    // warnings rather than aborting the advance: the agent's
    // main.rs may not have wired `dump_netlist_json` yet, in which
    // case the user gets an actionable diagnostic and the flow
    // continues. The dashboard's manual Render button stays
    // available for retries.
    if step.id == "DM2d" && next == Some("DM3a") {
        match crate::block_diagram::render_for_project(crate::block_diagram::RenderConfig {
            project_dir,
            output: None,
            direction: "tb",
            show_types: false,
            netlist_in: None,
        }) {
            Ok(svg_path) => {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Info,
                    message: format!("auto: rendered block diagram at {}", svg_path.display()),
                })?;
            }
            Err(err) => {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Warning,
                    message: format!("auto: block-diagram render failed (advancing anyway): {err}"),
                })?;
            }
        }
    }

    // Commit step artifacts before mutating sim-flow state so a
    // committed git history reflects each gate-clean checkpoint.
    let outcome = crate::git_commit::commit_step_advance(project_dir, step.id, next);
    if let Some(msg) = crate::git_commit::outcome_message(&outcome) {
        host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: msg,
        })?;
    }

    state.mark_passed(step.id, current_iso8601());
    if let Some(next_step) = next {
        state.current_step = next_step.to_string();
    }
    state.save(&dot)?;

    // Auto-resolve open bugs filed under this step. The gate just
    // passed; whatever bugs the agent was tracking for it are
    // presumed fixed (the resolution narrative says so explicitly
    // so the operator can spot false closures by scanning the log).
    // Bugs filed under DIFFERENT steps stay open -- a DM3c-era bug
    // doesn't get auto-closed when DM4ad passes.
    let bugs = crate::__internal::bug_log::load_all(project_dir);
    for bug in bugs
        .iter()
        .filter(|b| b.status == "open" && b.step == step.id)
    {
        let _ = crate::__internal::bug_log::resolve(
            project_dir,
            &bug.id,
            &format!(
                "auto-resolved: {} gate passed (bug presumed fixed by the work that cleared the gate)",
                step.id
            ),
            None,
        );
    }

    host.send(&Event::StateAdvanced {
        from: step.id.into(),
        to: next.map(String::from),
    })?;
    Ok(true)
}

// ---------------------------------------------------------------------
// Manual command dispatcher.
// ---------------------------------------------------------------------

/// Action the manual loop should run next for the user's current
/// step. Lives on the orchestrator side so the chat panel doesn't
/// have to duplicate the state machine -- the orchestrator has
/// every input it needs (state.toml, critique file, the last
/// sub-session it just ran).
#[derive(Debug, Clone)]
enum NextManualAction {
    /// Run the step's work sub-session.
    Work { step: String },
    /// Run the step's critique sub-session.
    Critique { step: String },
    /// Attempt to advance past the step (runs the gate internally).
    Advance { step: String },
}

impl NextManualAction {
    /// Pre-rendered label the chat panel surfaces on the Continue
    /// button so the user knows what's about to happen.
    fn label(&self) -> String {
        match self {
            NextManualAction::Work { step } => format!("Run work on {step}"),
            NextManualAction::Critique { step } => format!("Run critique on {step}"),
            NextManualAction::Advance { step } => format!("Advance past {step}"),
        }
    }
}

/// Pick the next manual-mode action. `last_subsession` is the
/// most recent sub-session this `run_auto` invocation ran (None
/// after fresh start, after Reset, or after an Advance bumped
/// current_step). Selection rules:
/// - last was work on the current step    → Critique
/// - last was critique on the current step → check critique file:
///   hasBlocking → Work (iterate); else → Advance (gate inside)
/// - last sub-session belongs to a different step OR is None
///   → Work (we're at the start of a step, or the user just
///   advanced)
fn compute_next_manual_action(
    opts: &AutoOptions,
    last_subsession: Option<(&str, SessionKindOut)>,
) -> Option<NextManualAction> {
    let state = State::load(&opts.project_dir.join(".sim-flow")).ok()?;
    let step = state.current_step.clone();
    let mid_step = last_subsession.is_some_and(|(s, _)| s == step.as_str());
    if !mid_step {
        return Some(NextManualAction::Work { step });
    }
    match last_subsession.map(|(_, k)| k) {
        Some(SessionKindOut::Work) => Some(NextManualAction::Critique { step }),
        Some(SessionKindOut::Critique) => {
            let critique = read_critique_entry(&opts.project_dir, &step).ok().flatten();
            match critique {
                Some(entry) if entry.has_blocking => Some(NextManualAction::Work { step }),
                Some(_) => Some(NextManualAction::Advance { step }),
                // No critique on disk despite a Critique sub-session
                // ending: edge case (e.g. the orchestrator crashed
                // mid-write). Suggest Work to recover.
                None => Some(NextManualAction::Work { step }),
            }
        }
        // Q&A turns don't change the work/critique cycle.
        Some(SessionKindOut::Qa) | None => Some(NextManualAction::Work { step }),
    }
}

/// Emit a `NextActionHint` for the orchestrator's current state.
fn emit_next_action_hint<P: Presenter + ?Sized>(
    opts: &AutoOptions,
    auto_host: &mut AutoPresenter<P>,
    last_subsession: Option<(&str, SessionKindOut)>,
) -> Result<()> {
    let hint = compute_next_manual_action(opts, last_subsession).map(|a| a.label());
    auto_host.send(&Event::NextActionHint { label: hint })
}

/// Dispatch the host-computed next manual action inline. Updates
/// `last_subsession` in place so subsequent `compute_next_manual_action`
/// calls see the new tail.
fn dispatch_continue_flow<P, L>(
    opts: &AutoOptions,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
    last_subsession: &mut Option<(String, SessionKindOut)>,
) -> Result<ManualOutcome>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    let snapshot = last_subsession.as_ref().map(|(s, k)| (s.as_str(), *k));
    let Some(action) = compute_next_manual_action(opts, snapshot) else {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: "ContinueFlow with no current step; nothing to do.".into(),
        })?;
        return Ok(ManualOutcome::Continue);
    };
    match action {
        NextManualAction::Work { step } => {
            run_manual_subsession(
                opts,
                &step,
                crate::client::SessionKind::Work,
                auto_host,
                llm,
            )?;
            *last_subsession = Some((step, SessionKindOut::Work));
        }
        NextManualAction::Critique { step } => {
            run_manual_subsession(
                opts,
                &step,
                crate::client::SessionKind::Critique,
                auto_host,
                llm,
            )?;
            *last_subsession = Some((step, SessionKindOut::Critique));
        }
        NextManualAction::Advance { step } => {
            run_manual_advance(opts, &step, auto_host, llm)?;
            // After advance succeeds current_step moved past `step`;
            // after refusal it didn't. Either way the next tick
            // should re-evaluate from a clean slate (no in-step
            // sub-session yet), so clear the tracker.
            *last_subsession = None;
        }
    }
    Ok(ManualOutcome::Continue)
}

fn wait_for_command<P, L>(
    opts: &AutoOptions,
    auto_host: &mut AutoPresenter<P>,
    qa_history: &mut Vec<LlmMessage>,
    llm: &mut L,
    last_subsession: &mut Option<(String, SessionKindOut)>,
) -> Result<ManualOutcome>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    match auto_host.recv()? {
        None => Ok(ManualOutcome::HostClosed),
        Some(HostEvent::Shutdown) => Ok(ManualOutcome::Shutdown),
        Some(HostEvent::RunStep { step, kind }) => {
            let (session_kind, tracker_kind) = match kind {
                SessionKindOut::Work => (crate::client::SessionKind::Work, SessionKindOut::Work),
                SessionKindOut::Critique => (
                    crate::client::SessionKind::Critique,
                    SessionKindOut::Critique,
                ),
                SessionKindOut::Qa => {
                    // RunStep is the user's "run a sub-session"
                    // command; Q&A is its own thing (triggered by
                    // UserMessage). A RunStep with kind=Qa is a host
                    // bug; warn and drop rather than dispatching.
                    auto_host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Warning,
                        message: format!(
                            "RunStep with kind=qa is not a real command; \
                             type a UserMessage to start a Q&A turn instead. \
                             Ignoring step={step}."
                        ),
                    })?;
                    return Ok(ManualOutcome::Continue);
                }
            };
            run_manual_subsession(opts, &step, session_kind, auto_host, llm)?;
            *last_subsession = Some((step, tracker_kind));
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::RunCritique { step }) => {
            run_manual_subsession(
                opts,
                &step,
                crate::client::SessionKind::Critique,
                auto_host,
                llm,
            )?;
            *last_subsession = Some((step, SessionKindOut::Critique));
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::RunGate { step }) => {
            run_manual_gate(opts, &step, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::Advance { step }) => {
            // Compare `current_step` before/after to detect a
            // refused advance. `run_manual_advance` emits Diagnostic
            // Errors on the gate-failed / critique-iters-exceeded /
            // milestone-walk-cap paths and then returns silently;
            // without an explicit signal here, the manual loop goes
            // back to parking on `parked_recv` while the host (e2e_manual,
            // dashboard) waits for a `StateAdvanced` event that
            // will never arrive -- deadlock. Emit `RequestUserInput`
            // when the step didn't change so the host can decide
            // what to do (rerun, reset, shut down).
            let before = State::load(&opts.project_dir.join(".sim-flow"))
                .ok()
                .map(|s| s.current_step.clone());
            run_manual_advance(opts, &step, auto_host, llm)?;
            let after = State::load(&opts.project_dir.join(".sim-flow"))
                .ok()
                .map(|s| s.current_step.clone());
            if before == after {
                auto_host.send(&Event::RequestUserInput {
                    prompt: Some(format!(
                        "Advance refused: `{step}` did not change state. \
                         Inspect the prior Error diagnostics, then issue \
                         RunStep / RunCritique / Reset, or end the session."
                    )),
                    placeholder: None,
                })?;
            }
            // Either advance succeeded (new step, no sub-session
            // yet) or it refused (next tick should re-evaluate
            // from the critique state). In both cases the in-step
            // tracker is no longer relevant.
            *last_subsession = None;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::Reset { step }) => {
            run_manual_reset(opts, &step, auto_host)?;
            *last_subsession = None;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::UserMessage { text }) => {
            // Idle-state freeform Q&A: the user typed a message while
            // manual mode is parked between sub-sessions. Dispatch a
            // side-conversation LLM turn against the accumulated
            // history. The user exits Q&A by clicking a step command
            // (RunStep / Advance / etc.) -- those still route through
            // their own arms above and naturally leave Q&A behind.
            //
            // History persists across step commands within this
            // `run_auto` invocation so a follow-up question after a
            // Run Step retains its earlier context.
            run_manual_qa_turn(opts, &text, qa_history, auto_host, llm)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::ContinueFlow) => {
            dispatch_continue_flow(opts, auto_host, llm, last_subsession)
        }
        Some(other) => {
            // Stray events while parked. Most aren't meaningful here
            // (leftover LlmChunk, etc.). Surface a warning so the
            // host operator can see the event was dropped, then keep
            // parking.
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message: format!(
                    "manual mode: ignored unexpected host event: {}",
                    host_event_label(&other),
                ),
            })?;
            Ok(ManualOutcome::Continue)
        }
    }
}

fn run_manual_subsession<P, L>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    if !validate_step_id(opts, step_id, kind_label_for_manual(kind), auto_host)? {
        return Ok(());
    }
    // Swallow the inner run_session's SessionEnd. The host treats
    // SessionEnd as "the orchestrator process is gone" and clears
    // its `activeSession` reference (see `socketPump.ts` →
    // `markTerminated` → `onManagedSessionSettled` →
    // `clearIfActive`). If we forwarded the per-sub-session
    // SessionEnd, the next dashboard click would find no active
    // pump and fall through to spawning a fresh `sim-flow session`
    // side process — reverting to classic non-manual behavior. The
    // orchestrator's outer `SessionEnd` only fires on `Shutdown`,
    // matching auto mode's between-iterations behavior. The
    // dashboard infers sub-session completion from the absence of
    // new chunks (same as auto mode between work and critique).
    //
    // Use the same `auto=true` semantics as the iterating loop so
    // the agent runs unattended within the sub-session.
    let session_auto = !(opts.dm0_interactive
        && step_id == "DM0"
        && matches!(kind, crate::client::SessionKind::Work));
    run_subsession(
        opts,
        step_id,
        kind,
        session_auto,
        auto_host,
        /*consume_end=*/ true,
        /*synth_hello=*/ true,
        llm,
    )
}

fn run_manual_gate<P: Presenter + ?Sized>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoPresenter<P>,
) -> Result<()> {
    use crate::gate;
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("RunGate: failed to load state: {err}"),
            })?;
            return Ok(());
        }
    };
    let registry = registry_for(state.flow);
    let step = match registry.get(step_id) {
        Some(s) => s,
        None => {
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("RunGate: `{step_id}` is not a {} step", state.flow.as_str()),
            })?;
            return Ok(());
        }
    };
    let report = gate::evaluate(&opts.project_dir, &step.gate_checks)?;
    auto_host.send(&Event::GateResult {
        step: step.id.into(),
        clean: report.is_clean(),
        failures: report
            .failures
            .iter()
            .map(|f| GateFailureOut {
                description: f.description.clone(),
                reason: f.reason.clone(),
            })
            .collect(),
    })?;
    Ok(())
}

fn run_manual_advance<P, L>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("Advance: failed to load state: {err}"),
            })?;
            return Ok(());
        }
    };
    let registry = registry_for(state.flow);
    if registry.get(step_id).is_none() {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("Advance: `{step_id}` is not a {} step", state.flow.as_str()),
        })?;
        return Ok(());
    }
    // For non-milestone-walk steps the simple `try_advance` is
    // sufficient: gate clean -> StateAdvanced; dirty -> Diagnostic
    // + stay parked. For milestone-walk steps (DM2cd, DM3ad, DM3b,
    // DM3c, DM4ad, DM4b) we mirror auto mode's two retry paths:
    //
    //   - **Critique findings on the current milestone**: the
    //     agent's Work session left issues the Critique flagged
    //     (e.g. duplicate task symbols). Re-run Work + Critique
    //     for the SAME milestone (the orchestrator's per-session
    //     prior critique feedback gives the agent the prior
    //     gate-failing findings verbatim). Bounded by `max_critique_iters`.
    //   - **More milestones pending**: the current milestone is
    //     clean and the gate's only failures are
    //     `MilestonesAllResolved` -- run Work + Critique for the
    //     NEXT pending milestone. Resets the critique-iter budget.
    //
    // Without this, manual hosts (e2e_manual, dashboard) stall the
    // moment the first per-milestone critique flags any blocker:
    // Advance fails, the host sits in `AwaitAdvance`, and the
    // milestone walk never progresses.
    const MILESTONE_WALK_CAP: u32 = 50;
    let mut walk_iter: u32 = 0;
    let mut critique_iters: u32 = 0;
    let mut no_progress_iters: u32 = 0;
    let mut prev_blocker_count: Option<usize> = None;
    loop {
        // Critique retry path: read the on-disk critique BEFORE
        // attempting advance. If gate-failing findings are present we
        // know advance would return Stuck for that reason; loop
        // back to Work directly so the agent gets the prior
        // findings in the next prompt without the user seeing a
        // misleading "cannot advance" Error.
        let gate_findings = read_gate_findings(&opts.project_dir, step_id);
        if !gate_findings.is_empty() {
            let cur_count = gate_findings.len();
            let delta = prev_blocker_count.map(|prev| cur_count as i64 - prev as i64);
            match delta {
                Some(d) if d < 0 => no_progress_iters = 0,
                Some(_) => no_progress_iters = no_progress_iters.saturating_add(1),
                None => {}
            }
            prev_blocker_count = Some(cur_count);
            critique_iters += 1;
            let hit_absolute = critique_iters > opts.max_critique_iters;
            let hit_no_progress = opts.max_critique_no_progress_iters > 0
                && no_progress_iters > opts.max_critique_no_progress_iters;
            if hit_absolute || hit_no_progress {
                let reason = if hit_no_progress && !hit_absolute {
                    format!(
                        "Advance: {step_id} critique made no progress for {} consecutive retry(ies) \
                         (blocker count {} did not strictly decrease); giving up. Inspect \
                         `docs/critiques/{step_id}-critique.json` and re-issue RunStep / RunCritique \
                         manually after fixing.",
                        no_progress_iters, cur_count,
                    )
                } else {
                    format!(
                        "Advance: {step_id} critique still has {} gate-failing finding(s) after {} retries; \
                         giving up. Inspect `docs/critiques/{step_id}-critique.json` and re-issue \
                         RunStep / RunCritique manually after fixing.",
                        cur_count, opts.max_critique_iters,
                    )
                };
                auto_host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: reason,
                })?;
                return Ok(());
            }
            let progress_note = match delta {
                Some(d) if d < 0 => format!(" (-{} from prior pass)", -d),
                Some(0) => format!(
                    " (no progress, streak {}/{})",
                    no_progress_iters, opts.max_critique_no_progress_iters
                ),
                Some(d) => format!(
                    " (+{} from prior pass, streak {}/{})",
                    d, no_progress_iters, opts.max_critique_no_progress_iters
                ),
                None => String::new(),
            };
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Info,
                message: format!(
                    "Advance: {step_id} critique has {} gate-failing finding(s); re-running Work + Critique \
                     (retry {}/{}{}).",
                    cur_count, critique_iters, opts.max_critique_iters, progress_note,
                ),
            })?;
            run_subsession(
                opts,
                step_id,
                crate::client::SessionKind::Work,
                /*auto=*/ true,
                auto_host,
                /*consume_end=*/ true,
                /*synth_hello=*/ true,
                llm,
            )?;
            run_subsession(
                opts,
                step_id,
                crate::client::SessionKind::Critique,
                /*auto=*/ true,
                auto_host,
                /*consume_end=*/ true,
                /*synth_hello=*/ true,
                llm,
            )?;
            continue;
        }

        let outcome = try_advance_classified(&opts.project_dir, step_id, auto_host)?;
        match outcome {
            AdvanceOutcome::Advanced => {
                // try_advance_classified already emitted StateAdvanced.
                return Ok(());
            }
            AdvanceOutcome::Stuck => {
                // Critique findings are clean but the structural gate
                // still has failures (typical: a `cargo test --test X`
                // or symbol-grep check the milestone walk hasn't yet
                // landed). try_advance_classified already emitted the
                // per-failure Error diagnostics. Instead of giving up
                // immediately, re-run Work + Critique so the agent
                // gets another pass -- the work session's no-artifact
                // pump surfaces the gate failures (see orchestrator
                // commit "surface gate failures in no-artifact pump").
                // Bounded by the existing `max_critique_iters` budget
                // so this can't loop forever.
                critique_iters += 1;
                if critique_iters > opts.max_critique_iters {
                    auto_host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message: format!(
                            "Advance: {step_id} structural gate still dirty after \
                             {} retries; giving up. Inspect the prior Error \
                             diagnostics and the milestone files, then re-issue \
                             RunStep / Advance after fixing.",
                            opts.max_critique_iters,
                        ),
                    })?;
                    return Ok(());
                }
                auto_host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Info,
                    message: format!(
                        "Advance: {step_id} structural gate dirty; re-running Work + \
                         Critique to address it (retry {}/{}).",
                        critique_iters, opts.max_critique_iters,
                    ),
                })?;
                run_subsession(
                    opts,
                    step_id,
                    crate::client::SessionKind::Work,
                    /*auto=*/ true,
                    auto_host,
                    /*consume_end=*/ true,
                    /*synth_hello=*/ true,
                    llm,
                )?;
                run_subsession(
                    opts,
                    step_id,
                    crate::client::SessionKind::Critique,
                    /*auto=*/ true,
                    auto_host,
                    /*consume_end=*/ true,
                    /*synth_hello=*/ true,
                    llm,
                )?;
                // Loop and re-attempt advance.
            }
            AdvanceOutcome::MoreMilestonesPending => {
                walk_iter += 1;
                if walk_iter > MILESTONE_WALK_CAP {
                    auto_host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message: format!(
                            "Advance: {step_id} milestone walk exceeded {MILESTONE_WALK_CAP} iterations \
                             without clearing the gate; aborting. Inspect the milestone files manually."
                        ),
                    })?;
                    return Ok(());
                }
                // New milestone targeted: reset critique-iter +
                // no-progress budgets and the prior blocker count
                // (the new milestone starts fresh).
                critique_iters = 0;
                no_progress_iters = 0;
                prev_blocker_count = None;
                auto_host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Info,
                    message: format!(
                        "Advance: {step_id} has more pending milestones; running next Work + Critique pair."
                    ),
                })?;
                run_subsession(
                    opts,
                    step_id,
                    crate::client::SessionKind::Work,
                    /*auto=*/ true,
                    auto_host,
                    /*consume_end=*/ true,
                    /*synth_hello=*/ true,
                    llm,
                )?;
                run_subsession(
                    opts,
                    step_id,
                    crate::client::SessionKind::Critique,
                    /*auto=*/ true,
                    auto_host,
                    /*consume_end=*/ true,
                    /*synth_hello=*/ true,
                    llm,
                )?;
                // Loop and re-attempt advance.
            }
        }
    }
}

fn run_manual_reset<P: Presenter + ?Sized>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoPresenter<P>,
) -> Result<()> {
    let dot = opts.project_dir.join(".sim-flow");
    let mut state = match State::load(&dot) {
        Ok(s) => s,
        Err(err) => {
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("Reset: failed to load state: {err}"),
            })?;
            return Ok(());
        }
    };
    let registry = registry_for(state.flow);
    let order: Vec<&'static str> = registry.order_for(state.flow);
    let Some(idx) = order.iter().position(|s| *s == step_id) else {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("Reset: `{step_id}` is not a {} step", state.flow.as_str()),
        })?;
        return Ok(());
    };

    // Step 1: delete generated collateral (artifacts + critiques)
    // for `step_id` and every downstream step. Shared with the
    // CLI-side `sim-flow reset` so both entry points clear the
    // same set of files.
    let (deleted, delete_failures) =
        clear_step_collateral_forward(&opts.project_dir, idx, &order, &registry);

    // Step 2: clear gate flags + rewind current_step.
    if let Err(err) = state.reset(step_id, &order) {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("Reset: {err}"),
        })?;
        return Ok(());
    }
    state.save(&dot)?;

    // Step 3: report.
    let cleared_count = order.len() - idx;
    let mut summary = format!("Reset to `{step_id}`. Cleared {cleared_count} gate flag(s)");
    if deleted.is_empty() {
        summary.push_str("; no generated collateral found to delete.");
    } else {
        summary.push_str(&format!(
            "; deleted {} file(s) / directory(ies):",
            deleted.len()
        ));
        for path in &deleted {
            let rel = path
                .strip_prefix(&opts.project_dir)
                .unwrap_or(path)
                .display();
            summary.push_str(&format!("\n  - {rel}"));
        }
    }
    auto_host.send(&Event::Diagnostic {
        level: DiagnosticLevel::Info,
        message: summary,
    })?;
    for (path, err) in &delete_failures {
        let rel = path
            .strip_prefix(&opts.project_dir)
            .unwrap_or(path)
            .display();
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!("Reset: failed to delete {rel}: {err}"),
        })?;
    }
    Ok(())
}

/// Idle-state freeform Q&A: dispatch ONE LLM round-trip against the
/// accumulated chat history and append both the new user message and
/// the assistant reply.
///
/// **Scope of this v1**: text-only conversation. The LLM cannot read
/// or write project files yet -- the full tool-execution loop from
/// `run_session` hasn't been factored into a shared helper. Multi-turn
/// works (history persists across calls), so the user can have a
/// back-and-forth conversation about whatever's in `qa_history` plus
/// the small project-state preamble the system prompt carries. The
/// follow-up commit that extends this with read/write/cargo will
/// reuse the same `qa_history` and bracket structure.
///
/// Each turn is bracketed by `SubSessionStarted/Ended { kind: Qa }`
/// so hosts can mark Q&A turns visually vs flow work.
fn run_manual_qa_turn<P, L>(
    opts: &AutoOptions,
    user_text: &str,
    qa_history: &mut Vec<LlmMessage>,
    auto_host: &mut AutoPresenter<P>,
    llm: &mut L,
) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    // Determine the current step for the bracket payload + system
    // prompt context. If state.toml is unreadable we still run Q&A,
    // just without the step anchor.
    let current_step = State::load(&opts.project_dir.join(".sim-flow"))
        .ok()
        .map(|s| s.current_step)
        .unwrap_or_else(|| "?".to_string());

    auto_host.send(&Event::SubSessionStarted {
        step: current_step.clone(),
        kind: SessionKindOut::Qa,
    })?;

    // Append the user turn to the persistent history BEFORE building
    // the request so the LLM sees it. A snapshot of the history is
    // what we ship over the wire.
    qa_history.push(LlmMessage {
        role: LlmRole::User,
        content: user_text.to_string(),
        ..LlmMessage::default()
    });

    // System prompt: small, focused on the side-conversation role.
    // The project dir + current step give the LLM minimal grounding
    // without dragging in step-specific instructions (which would
    // push it toward writing artifacts -- not the Q&A intent).
    let system = LlmMessage {
        role: LlmRole::System,
        content: format!(
            "You are sim-flow's interactive assistant. The user has paused the \
             direct-modeling flow at step `{current_step}` and is asking you a \
             question or requesting guidance.\n\n\
             Project root: `{}`\n\n\
             Answer concisely. The user is in control of the flow -- they will \
             click a step command (Run Step, Advance, etc.) when ready to \
             resume; that exits this side conversation.\n\n\
             NOTE: Tool execution is not yet wired for Q&A turns. You can answer \
             from the conversation context you have, but cannot read project \
             files, run cargo, or write changes in this turn. If the user asks \
             for an investigation or edit, summarize what should be done and \
             have them click a step command (or run the relevant cargo / git \
             query manually) to take the action.",
            opts.project_dir.display(),
        ),
        ..LlmMessage::default()
    };
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(qa_history.len() + 1);
    messages.push(system);
    messages.extend(qa_history.iter().cloned());

    // Q&A LLM stack: `resolve_llm_for_qa` still produces the routing
    // metadata so diagnostics / metrics see "this turn intended to
    // route to backend X". The actual dispatch goes through the
    // injected `llm` adapter (run_auto's parameter). The
    // `qa_llm_*` override knob is now informational; if/when the
    // dashboard wants per-kind routing back, run_auto's API will
    // need to accept multiple adapters.
    let _routing = resolve_llm_for_qa(opts);
    // Tools are deferred — Q&A is text-only in v1. Subprocess CLI
    // backends fall through to the trait-default `dispatch_with_tools`
    // which drops the empty catalog and returns no tool calls.
    let qa_tools: Vec<crate::session::agent::ToolAdvertise> = Vec::new();
    let mut assistant_text = String::new();
    let mut llm_failed = false;
    let mut llm_error_message: Option<String> = None;
    match llm.dispatch_with_tools(&messages, &qa_tools) {
        Ok((text, _calls, _metrics)) => {
            assistant_text = text;
        }
        Err(err) => {
            llm_failed = true;
            llm_error_message = Some(format!("{err}"));
        }
    }

    if llm_failed {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!(
                "Q&A turn failed: {}",
                llm_error_message.unwrap_or_else(|| "unknown error".into()),
            ),
        })?;
        // Roll back the user message so a retry doesn't double up.
        qa_history.pop();
        auto_host.send(&Event::SubSessionEnded {
            step: current_step,
            kind: SessionKindOut::Qa,
            outcome: "error".to_string(),
        })?;
        return Ok(());
    }

    // Single AssistantText carrying the full Q&A reply plus the
    // final-chunk marker. The legacy host-driven streaming path
    // emitted chunks as they arrived; the in-process dispatch
    // returns the full body at once so the chat panel sees one
    // event per turn -- same shape as run_session's main path.
    auto_host.send(&Event::AssistantText {
        text: assistant_text.clone(),
        final_chunk: true,
        tool_calls: Vec::new(),
    })?;

    qa_history.push(LlmMessage {
        role: LlmRole::Assistant,
        content: assistant_text,
        ..LlmMessage::default()
    });

    auto_host.send(&Event::SubSessionEnded {
        step: current_step,
        kind: SessionKindOut::Qa,
        outcome: "completed".to_string(),
    })?;
    Ok(())
}

/// Delete every step's `work_artifacts` AND its critique file for
/// the reset target step (`order[idx]`) and every downstream step.
/// Returns (deleted_paths, failures). Source spec, conversation
/// transcript, git history, and `.sim-flow/` are not touched. Files
/// / dirs that don't exist are silently skipped; deletion failures
/// are collected for the caller to surface.
///
/// **Upstream-protection**: paths claimed by steps UPSTREAM of the
/// reset target are never deleted, even when a downstream step's
/// `work_artifacts` declaration would otherwise sweep them. The
/// concrete bug this guards against: DM4b's `work_artifacts` is
/// `["docs/analysis/"]` (the whole directory) but DM2a writes
/// `docs/analysis/decomposition.md` and DM2b writes
/// `docs/analysis/pipeline-mapping.md` into the same directory.
/// Without protection a reset to DM3a walks DM3a -> DM3b -> DM3c
/// -> DM4a -> DM4b and `remove_dir_all`s the whole directory,
/// taking DM2a/DM2b's outputs with it. The protection logic here
/// scans `docs/analysis/` for upstream-owned children and either
/// (a) skips the directory delete entirely if any child is
/// upstream-owned, doing a selective per-file walk instead, or
/// (b) does a full `remove_dir_all` only when nothing inside is
/// upstream-owned. Same logic applies to `tests/` (shared by
/// DM2d/DM3b/DM3c) and any other coarse work_artifact declaration
/// that's also touched by an earlier step.
///
/// Shared between the auto-driver's in-session `Reset` HostEvent
/// handler and the CLI-side `sim-flow reset` command so both entry
/// points clear the same set of files. Without this sharing the CLI
/// reset would only clear gate flags, leaving stale critiques + work
/// artifacts on disk that confuse the next auto run (the agent reads
/// "DM3a-critique.md" left over from the prior pass and thinks it's
/// in critique-retry mode for a step it just reset).
pub fn clear_step_collateral_forward(
    project_dir: &Path,
    idx: usize,
    order: &[&'static str],
    registry: &crate::__internal::steps::StepRegistry,
) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    use std::collections::HashSet;

    // FILE-granular upstream-protected set. Sources, in order:
    //
    //   1. Each upstream step's manifest
    //      (`.sim-flow/manifests/<step>.txt`) -- the authoritative
    //      list of files that step's run wrote. With a manifest, we
    //      can preserve shared-directory files exactly: tests/
    //      contains both DM2d's smoke tests (upstream) and DM3b's
    //      testbench/ scaffolding (downstream-of-DM3a) -- resetting
    //      DM3a should clean tests/testbench/ but keep tests/
    //      contents DM2d wrote. The manifest captures that.
    //   2. Each upstream step's critique JSON + rendered .md so the
    //      downstream cascade doesn't sweep them on the way past
    //      `docs/critiques/`.
    //   3. Fallback when (1) is empty: protect the upstream's
    //      `work_artifacts` declarations wholesale. This is the old
    //      pre-manifest behavior and remains useful for projects
    //      that ran before the manifest mechanism existed -- their
    //      manifests are empty so we fall back to the coarser dir-
    //      level protection.
    let mut protected: HashSet<PathBuf> = HashSet::new();
    for upstream in &order[..idx] {
        let Some(step) = registry.get(upstream) else {
            continue;
        };
        let manifest = crate::__internal::manifest::step_paths(project_dir, step.id);
        if manifest.is_empty() {
            for art in step.work_artifacts {
                protected.insert(project_dir.join(art.trim_end_matches('/')));
            }
        } else {
            protected.extend(manifest);
        }
        protected.insert(project_dir.join(format!("docs/critiques/{}-critique.json", step.id)));
        protected.insert(project_dir.join(format!("docs/critiques/{}-critique.md", step.id)));
    }

    let mut deleted: Vec<PathBuf> = Vec::new();
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    for downstream in &order[idx..] {
        let Some(step) = registry.get(downstream) else {
            continue;
        };
        // Pass 1: manifest-driven deletion. Every file this step's
        // run wrote is a candidate. Files that an upstream step also
        // wrote (and thus appear in `protected`) survive --
        // `delete_with_upstream_protection` short-circuits on
        // `protected.contains(&path)`.
        let manifest = crate::__internal::manifest::step_paths(project_dir, step.id);
        for abs in &manifest {
            let Ok(rel) = abs.strip_prefix(project_dir) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().to_string();
            if rel_str.is_empty() {
                continue;
            }
            delete_with_upstream_protection(
                project_dir,
                &rel_str,
                &protected,
                &mut deleted,
                &mut failures,
            );
        }
        // Clear the manifest now that we've consumed it. A
        // resurrected manifest (from a later run that re-enters this
        // step) will be rebuilt as the step runs again.
        crate::__internal::manifest::clear(project_dir, step.id);
        // Pass 2: work_artifacts walk. Catches anything the manifest
        // didn't capture: in-flight writes from a step that hadn't
        // gate-passed before the reset (still useful), and the
        // historical-fallback case where the step ran before the
        // manifest mechanism existed.
        for art in step.work_artifacts {
            delete_with_upstream_protection(
                project_dir,
                art,
                &protected,
                &mut deleted,
                &mut failures,
            );
        }
        // Delete BOTH critique forms. Post-migration the agent emits
        // the JSON via `write_file`; the orchestrator renders the
        // `.md` sibling. A reset that only removes one of the two
        // leaves a stale critique on disk that the gate / retry path
        // (`Critique::load`) will pick up the next time the step
        // runs, defeating the reset. See
        // `tools/sim-flow/src/__internal/critique.rs::Critique::load`
        // -- it resolves the JSON sibling first.
        let json_rel = format!("docs/critiques/{}-critique.json", step.id);
        delete_with_upstream_protection(
            project_dir,
            &json_rel,
            &protected,
            &mut deleted,
            &mut failures,
        );
        let md_rel = format!("docs/critiques/{}-critique.md", step.id);
        delete_with_upstream_protection(
            project_dir,
            &md_rel,
            &protected,
            &mut deleted,
            &mut failures,
        );
        // Pass 3: milestone-progress reset. Steps that walk a
        // milestone directory (DM2d / DM3b / DM3c / DM4b) store
        // per-task progress as `- [x]` rows inside files OWNED by
        // an upstream step (DM2c / DM3a). Those files survive the
        // file sweep above (they're upstream's `work_artifacts`)
        // and the `[x]` marks leak across resets, so the dashboard
        // reports 100% completion for a step with no source code
        // on disk and the agent's next run hits a confused state.
        // `MilestoneManager::reset` flips each `[x]`/`[-]` back to
        // `[ ]` in place, preserving the task TEXT (which the
        // upstream's planning content owns) and clearing only the
        // progress state this step writes.
        use crate::__internal::steps::MilestoneManager;
        if let Some(walk) = step.milestone_walk {
            match walk.reset(project_dir) {
                Ok(paths) => deleted.extend(paths),
                Err(err) => {
                    let dir = project_dir.join(walk.dir().trim_end_matches('/'));
                    failures.push((dir, format!("milestone reset failed: {err}")));
                }
            }
        }
    }
    (deleted, failures)
}

/// File names / extensions that always survive a reset, regardless
/// of which work_artifact directory they sit in. These are project
/// scaffolding (templates seeded from `templates/model-project/`
/// during `sim-flow new model`, gitkeep markers) that no flow step
/// claims as its output -- so a reset cascade should not sweep
/// them when it's selectively cleaning a shared directory.
fn is_reset_scaffolding(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == ".gitkeep" || name == ".gitignore" {
        return true;
    }
    // Conservative extension list: only the ones we ship as
    // scaffolding from templates/model-project/. Adding more is
    // safe; flagging an output as scaffolding by mistake would
    // leak stale state into the next run.
    matches!(path.extension().and_then(|e| e.to_str()), Some("tmpl"))
}

/// True iff `dir` contains any file (recursively) that
/// `is_reset_scaffolding` accepts. Triggers the selective-walk
/// branch in `delete_with_upstream_protection` so a coarse
/// `remove_dir_all` can't blow away `.tmpl` / `.gitkeep` files
/// the project seeds at creation time.
fn dir_has_scaffolding(dir: &Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_reset_scaffolding(&path) {
            return true;
        }
        if path.is_dir() && dir_has_scaffolding(&path) {
            return true;
        }
    }
    false
}

/// Delete one collateral path under `project_dir`, honoring an
/// upstream-protected set. See `clear_step_collateral_forward` for
/// the rationale.
///
/// Behavior:
/// - Path missing -> no-op.
/// - Path is reset-scaffolding (`.gitkeep`, `*.tmpl`) -> skip.
/// - Path exact-matches a protected entry -> skip (upstream owns
///   this file/dir entirely).
/// - Path is a regular file and not protected -> remove_file.
/// - Path is a directory:
///   - If no protected entry is a descendant -> remove_dir_all.
///   - Otherwise: walk the directory recursively, deleting only
///     entries that aren't protected, aren't scaffolding, and
///     aren't ancestors of a protected entry. The directory
///     itself is left in place because protected children still
///     need a parent.
fn delete_with_upstream_protection(
    project_dir: &Path,
    rel: &str,
    protected: &std::collections::HashSet<PathBuf>,
    deleted: &mut Vec<PathBuf>,
    failures: &mut Vec<(PathBuf, String)>,
) {
    let path = project_dir.join(rel.trim_end_matches('/'));
    if !path.exists() {
        return;
    }
    if protected.contains(&path) {
        return;
    }
    if is_reset_scaffolding(&path) {
        return;
    }
    let is_dir = rel.ends_with('/') || path.is_dir();
    if !is_dir {
        match std::fs::remove_file(&path) {
            Ok(()) => deleted.push(path),
            Err(err) => failures.push((path, err.to_string())),
        }
        return;
    }
    // Directory: switch to a selective walk if EITHER any
    // protected file lives inside (upstream-owned children we
    // must preserve) OR any scaffolding file (`.gitkeep`,
    // `*.tmpl`) lives inside. Without the scaffolding check a
    // `remove_dir_all` here would sweep the .tmpl files that
    // `sim-flow new model` seeded -- the agent would lose the
    // copy-then-fill template DM3a/DM1 prompts depend on.
    let has_protected_descendant = protected.iter().any(|p| p.starts_with(&path) && p != &path);
    let has_scaffolding_descendant = dir_has_scaffolding(&path);
    if !has_protected_descendant && !has_scaffolding_descendant {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => deleted.push(path),
            Err(err) => failures.push((path, err.to_string())),
        }
        return;
    }
    // Selective walk: read children, recurse on subdirs, delete
    // files that aren't protected. Don't `remove_dir_all` the
    // parent because protected children must survive.
    let entries = match std::fs::read_dir(&path) {
        Ok(e) => e,
        Err(err) => {
            failures.push((path, err.to_string()));
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let child = entry.path();
        if protected.contains(&child) {
            continue;
        }
        if is_reset_scaffolding(&child) {
            continue;
        }
        // If a protected entry is below `child`, recurse into
        // `child` rather than removing it wholesale.
        let child_has_protected_descendant = protected
            .iter()
            .any(|p| p.starts_with(&child) && p != &child);
        if child_has_protected_descendant {
            // Build a child rel-path-suffix to recurse with. We
            // don't strictly need the rel string for behavior but
            // keep the signature consistent.
            let child_rel = match child.strip_prefix(project_dir) {
                Ok(p) => format!("{}/", p.display()),
                Err(_) => continue,
            };
            delete_with_upstream_protection(project_dir, &child_rel, protected, deleted, failures);
            continue;
        }
        let result = if child.is_dir() {
            std::fs::remove_dir_all(&child)
        } else {
            std::fs::remove_file(&child)
        };
        match result {
            Ok(()) => deleted.push(child),
            Err(err) => failures.push((child, err.to_string())),
        }
    }
}

fn validate_step_id<P: Presenter + ?Sized>(
    opts: &AutoOptions,
    step_id: &str,
    cmd_label: &str,
    auto_host: &mut AutoPresenter<P>,
) -> Result<bool> {
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("{cmd_label}: failed to load state: {err}"),
            })?;
            return Ok(false);
        }
    };
    let registry = registry_for(state.flow);
    if registry.get(step_id).is_none() {
        auto_host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!(
                "{cmd_label}: `{step_id}` is not a {} step",
                state.flow.as_str()
            ),
        })?;
        return Ok(false);
    }
    Ok(true)
}

fn kind_label_for_manual(kind: crate::client::SessionKind) -> &'static str {
    match kind {
        crate::client::SessionKind::Work => "RunStep",
        crate::client::SessionKind::Critique => "RunCritique",
    }
}

/// Findings that prevent the gate from passing. Both `BLOCKER:`
/// and `UNRESOLVED:` lines block advancement. The auto driver MUST match
/// `Finding::is_blocking` in `tools/sim-flow/src/critique.rs` (which
/// the gate's `CritiqueClean` check uses) or it will loop on issues
/// the gate would happily pass.
/// Returns the gate-failing finding texts the gate's
/// `CritiqueClean` check would flag. Delegates to `Critique::parse`
/// (the gate-side parser) so the auto-driver and gate can never
/// disagree about what counts as a finding. Without this sharing
/// the auto-driver could decide a critique was clean and advance
/// while the gate held it back, or vice versa -- exactly the bug
/// that let DM3b advance past 5 heading-style `## BLOCKER:`
/// findings the gate-side parser missed.
fn read_gate_findings(project_dir: &Path, step_id: &str) -> Vec<String> {
    let path = project_dir
        .join("docs/critiques")
        .join(format!("{step_id}-critique.md"));
    let critique = match crate::critique::Critique::load(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    critique
        .blocking()
        .into_iter()
        .map(|f| match f {
            crate::critique::Finding::Resolved(_) => unreachable!("blocking() excludes resolved"),
            crate::critique::Finding::Unresolved(text) => format!("UNRESOLVED: {text}"),
            crate::critique::Finding::Blocker(text) => format!("BLOCKER: {text}"),
        })
        .collect()
}

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Match main.rs's format-free unix epoch placeholder for now.
    format!("{}", dur.as_secs())
}

fn host_event_label(event: &HostEvent) -> &'static str {
    match event {
        HostEvent::Hello { .. } => "Hello",
        HostEvent::UserMessage { .. } => "UserMessage",
        HostEvent::FollowupSelected { .. } => "FollowupSelected",
        HostEvent::Cancel => "Cancel",
        HostEvent::RunStep { .. } => "RunStep",
        HostEvent::RunCritique { .. } => "RunCritique",
        HostEvent::RunGate { .. } => "RunGate",
        HostEvent::Advance { .. } => "Advance",
        HostEvent::Reset { .. } => "Reset",
        HostEvent::SetStepMode { .. } => "SetStepMode",
        HostEvent::Shutdown => "Shutdown",
        HostEvent::ContinueFlow => "ContinueFlow",
    }
}

fn is_manual_command(event: &HostEvent) -> bool {
    matches!(
        event,
        HostEvent::RunStep { .. }
            | HostEvent::RunCritique { .. }
            | HostEvent::RunGate { .. }
            | HostEvent::Advance { .. }
            | HostEvent::Reset { .. }
    )
}

// ---------------------------------------------------------------------
// AutoPresenter wrapper
// ---------------------------------------------------------------------

/// `Host` wrapper that lets the auto driver run multiple back-to-back
/// `run_session` calls through one underlying connection while
/// surfacing manual-mode commands and step-mode transitions to the
/// run loop.
pub struct AutoPresenter<'a, P: Presenter + ?Sized> {
    inner: &'a mut P,
    pending_reads: VecDeque<HostEvent>,
    /// Set true before each non-final sub-session; on the next
    /// SessionEnd write we swallow it instead of forwarding.
    pub consume_session_end: bool,
    /// Set when we observe a `max_auto_iters`-exceeded diagnostic so
    /// the driver can stop scheduling further sub-sessions.
    pub cap_exceeded: bool,
    /// Shared step-axis mode flag. Updated by `SetStepMode` host
    /// events, by the cap-exceeded path, and by gate-failure /
    /// blocker-cap halt paths. Read by the run loop at every decision
    /// point.
    step_mode: Arc<AtomicU8>,
    /// Set when a `Shutdown` host event is observed. Run loop checks
    /// this between sub-sessions and exits the orchestrator.
    pub shutdown_requested: bool,
    /// True while we're inside `run_subsession`. Manual-mode commands
    /// (RunStep, etc.) that arrive in this window are normally
    /// rejected with a Diagnostic; outside this window the parking
    /// loop reads them.
    pub in_subsession: bool,
    /// True while the inner sub-session is parked at
    /// `RequestUserInput`. The orchestrator isn't doing any work
    /// during this span -- it's blocked at `host.recv()` waiting for
    /// the user's reply -- so a manual-mode command arriving here is
    /// reasonable to interpret as "I'm done with this sub-session;
    /// run the new command instead". Set on every `RequestUserInput`
    /// write, cleared on the next active-work event from the
    /// orchestrator and on every sub-session boundary.
    pub in_subsession_parked: bool,
}

impl<'a, P: Presenter + ?Sized> AutoPresenter<'a, P> {
    pub fn new(inner: &'a mut P, step_mode: Arc<AtomicU8>) -> Self {
        Self {
            inner,
            pending_reads: VecDeque::new(),
            consume_session_end: false,
            cap_exceeded: false,
            step_mode,
            shutdown_requested: false,
            in_subsession: false,
            in_subsession_parked: false,
        }
    }

    /// Queue a synthetic `Hello` event so the next sub-session's
    /// orchestrator handshake reads it instead of blocking on the
    /// underlying host.
    pub fn queue_synthetic_hello(&mut self) {
        self.pending_reads.push_back(HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "sim-flow-auto".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
        });
    }

    fn current_step_mode(&self) -> StepMode {
        step_mode_from_u8(self.step_mode.load(Ordering::Acquire))
    }

    fn store_step_mode(&self, mode: StepMode) {
        self.step_mode
            .store(step_mode_to_u8(mode), Ordering::Release);
    }
}

impl<P: Presenter + ?Sized> Presenter for AutoPresenter<'_, P> {
    fn send(&mut self, event: &Event) -> Result<()> {
        // Watch for the auto-cap diagnostic so the driver can stop
        // and queue a Cancel to break the in-flight sub-session
        // immediately rather than parking on RequestUserInput.
        if !self.cap_exceeded
            && let Event::Diagnostic { level, message } = event
            && matches!(level, DiagnosticLevel::Error)
            && message.contains("max_auto_iters")
        {
            self.cap_exceeded = true;
            self.pending_reads.push_back(HostEvent::Cancel);
        }
        // Track whether the inner sub-session is currently parked at
        // `RequestUserInput`. This lets the manual-command branch in
        // `read` distinguish "actively running, reject" from "parked
        // and idle, accept the command by cancelling the parked
        // session". The bracket open/close paths in `run_subsession`
        // reset the flag explicitly; we only mutate it for events
        // that occur DURING the sub-session.
        if self.in_subsession {
            match event {
                Event::RequestUserInput { .. } => {
                    self.in_subsession_parked = true;
                }
                // Bracket transitions and pure-state echoes don't
                // affect the parking signal; the sub-session
                // boundary handlers in `run_subsession` reset the
                // flag at the right moments.
                Event::SubSessionStarted { .. }
                | Event::SubSessionEnded { .. }
                | Event::StepModeChanged { .. }
                | Event::SessionEnd { .. } => {}
                // Anything else is the orchestrator actively
                // producing output -- back to running.
                _ => {
                    self.in_subsession_parked = false;
                }
            }
        }
        if self.consume_session_end && matches!(event, Event::SessionEnd { .. }) {
            self.consume_session_end = false;
            return Ok(());
        }
        self.inner.send(event)
    }

    fn recv(&mut self) -> Result<Option<HostEvent>> {
        loop {
            if let Some(h) = self.pending_reads.pop_front() {
                return Ok(Some(h));
            }
            let next = self.inner.recv()?;
            match next {
                Some(HostEvent::SetStepMode { mode }) => {
                    let prev = self.current_step_mode();
                    self.store_step_mode(mode);
                    if prev != mode {
                        info!(from = ?prev, to = ?mode, "step mode flipped by host command");
                        self.inner.send(&Event::StepModeChanged { mode })?;
                    } else {
                        debug!(mode = ?mode, "SetStepMode received but mode unchanged");
                    }
                    continue;
                }
                Some(HostEvent::Shutdown) => {
                    info!(in_subsession = self.in_subsession, "shutdown requested");
                    self.shutdown_requested = true;
                    if self.in_subsession {
                        // Cancel the in-flight sub-session at the next
                        // safe boundary. The run loop reads
                        // shutdown_requested afterward and exits.
                        return Ok(Some(HostEvent::Cancel));
                    }
                    return Ok(Some(HostEvent::Shutdown));
                }
                Some(cmd) if is_manual_command(&cmd) => {
                    let label = host_event_label(&cmd);
                    if matches!(self.current_step_mode(), StepMode::Auto) {
                        warn!(cmd = label, "rejecting manual command in auto mode");
                        self.inner.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "ignored {label}: auto mode owns step execution; toggle to manual first."
                            ),
                        })?;
                        continue;
                    }
                    if self.in_subsession {
                        if self.in_subsession_parked {
                            // Parked sub-session: the orchestrator is
                            // blocked at `host.recv()` waiting for the
                            // user's reply. The manual command is the
                            // user saying "I'm done with this session;
                            // run the new command instead". Queue the
                            // command for the OUTER manual loop and
                            // return Cancel to break the inner read
                            // out of its park -- the inner emits
                            // SessionEnd, `run_session` returns,
                            // `run_subsession` emits SubSessionEnded,
                            // and the outer loop's next read picks up
                            // the queued manual command.
                            info!(
                                cmd = label,
                                "manual command received while parked; cancelling and dispatching"
                            );
                            self.pending_reads.push_back(cmd);
                            return Ok(Some(HostEvent::Cancel));
                        }
                        warn!(
                            cmd = label,
                            "rejecting manual command while sub-session in flight"
                        );
                        self.inner.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "ignored {label}: a sub-session is currently running; retry after it finishes."
                            ),
                        })?;
                        continue;
                    }
                    debug!(cmd = label, "manual command dispatched");
                    return Ok(Some(cmd));
                }
                other => return Ok(other),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::host::TestHost;

    fn options_with_llm(backend: &str, model: Option<&str>, base_url: Option<&str>) -> AutoOptions {
        AutoOptions {
            project_dir: std::path::PathBuf::new(),
            foundation_root: std::path::PathBuf::new(),
            llm_backend: backend.into(),
            llm_model: model.map(String::from),
            llm_model_family_id: None,
            llm_runtime_profile_id: None,
            llm_debug_adaptation: false,
            llm_base_url: base_url.map(String::from),
            critique_llm_backend: None,
            critique_llm_model: None,
            critique_llm_model_family_id: None,
            critique_llm_runtime_profile_id: None,
            critique_llm_base_url: None,
            qa_llm_backend: None,
            qa_llm_model: None,
            qa_llm_model_family_id: None,
            qa_llm_runtime_profile_id: None,
            qa_llm_base_url: None,
            max_auto_iters: 1,
            max_critique_iters: 1,
            max_critique_no_progress_iters: 0,
            dm0_interactive: false,
            max_llm_requests: 50,
            max_identical_responses: 0,
            max_parallel_requests: 0,
            step_mode: crate::session::protocol::StepMode::Auto,
            no_preamble: true,
        }
    }

    #[test]
    fn channel_presenter_forwards_send_into_channel() {
        // ChannelPresenter::send must clone the event into the
        // mpsc channel so the coordinator on the main thread can
        // forward it to the real AutoPresenter. A regression here
        // would silently drop per-worker events from the dashboard.
        let (tx, rx) = std::sync::mpsc::channel::<Event>();
        let mut pres = ChannelPresenter::new(tx);
        pres.send(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: "hello from worker".into(),
        })
        .unwrap();
        let received = rx.recv().unwrap();
        match received {
            Event::Diagnostic { level, message } => {
                assert_eq!(level, DiagnosticLevel::Info);
                assert_eq!(message, "hello from worker");
            }
            other => panic!("expected Diagnostic, got {other:?}"),
        }
    }

    #[test]
    fn channel_presenter_recv_always_returns_none() {
        // After the parallel-walk #3 fix, ChannelPresenter never
        // emits its own Hello. The orchestrator's handshake Hello
        // is supplied by AutoPresenter::queue_synthetic_hello (the
        // outer presenter layer that wraps this one). Returning
        // Some(Hello) here would land a second, phantom Hello in
        // the orchestrator any time pending_reads was exhausted
        // and recv fell through to the inner presenter -- a
        // footgun if any future feature adds a recv to the auto
        // path. None on every recv is the correct contract:
        // "this channel has no real host on the other end."
        let (tx, _rx) = std::sync::mpsc::channel::<Event>();
        let mut pres = ChannelPresenter::new(tx);
        assert!(pres.recv().unwrap().is_none(), "first recv must be None");
        assert!(pres.recv().unwrap().is_none(), "second recv stays None");
        assert!(pres.recv().unwrap().is_none(), "subsequent recv stays None");
    }

    #[test]
    fn ref_adapter_delegates_dispatch_to_inner() {
        // RefAdapter is the borrow-friendly LlmAdapter wrapper each
        // worker thread holds. The delegations must be 1:1 so the
        // shared underlying adapter sees every dispatch as if it
        // were called directly.
        let mock = crate::session::MockAgent::new();
        mock.enqueue("scripted text");
        let adapter = RefAdapter(&mock);
        let (text, _metrics) = adapter.dispatch(&[]).unwrap();
        assert_eq!(text, "scripted text");
        assert_eq!(mock.seen.lock().unwrap().len(), 1);
        assert_eq!(adapter.name(), mock.name());
    }

    #[test]
    fn run_plan_detail_walk_parallel_returns_false_for_max_parallel_one() {
        // max_parallel_requests = 1 is the explicit opt-out
        // signal -- caller must fall back to the serial walker
        // even if there are many pending stubs.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let state = State::new(crate::state::Flow::DirectModeling, "DM2cd");
        state.save(&project.join(".sim-flow")).unwrap();

        let mut opts = options_with_llm("mock", None, None);
        opts.project_dir = project;
        opts.max_parallel_requests = 1;
        let step = registry_for(crate::state::Flow::DirectModeling)
            .get("DM2cd")
            .unwrap()
            .clone();
        let mut host = TestHost::new();
        let mode = Arc::new(AtomicU8::new(STEP_MODE_AUTO));
        let mut auto_host = AutoPresenter::new(&mut host, mode);
        let mut mock = crate::session::MockAgent::new();
        let ran = run_plan_detail_walk_parallel(&opts, &step, &mut auto_host, &mut mock).unwrap();
        assert!(
            !ran,
            "max_parallel_requests=1 must opt out of the parallel path"
        );
    }

    #[test]
    fn run_plan_detail_walk_parallel_returns_false_when_no_pending_stubs() {
        // No milestone files on disk -> nothing to fan out ->
        // caller falls back to the serial walker, which produces
        // the right "AllResolved" or "NoMilestonesPresent" gate
        // diagnostic.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        std::fs::create_dir_all(project.join("docs/impl-plan")).unwrap();
        let state = State::new(crate::state::Flow::DirectModeling, "DM2cd");
        state.save(&project.join(".sim-flow")).unwrap();

        let mut opts = options_with_llm("mock", None, None);
        opts.project_dir = project;
        opts.max_parallel_requests = 0;
        let step = registry_for(crate::state::Flow::DirectModeling)
            .get("DM2cd")
            .unwrap()
            .clone();
        let mut host = TestHost::new();
        let mode = Arc::new(AtomicU8::new(STEP_MODE_AUTO));
        let mut auto_host = AutoPresenter::new(&mut host, mode);
        let mut mock = crate::session::MockAgent::new();
        let ran = run_plan_detail_walk_parallel(&opts, &step, &mut auto_host, &mut mock).unwrap();
        assert!(
            !ran,
            "empty milestone dir must opt out of the parallel path (caller serial walker handles it)"
        );
    }

    #[test]
    fn run_plan_detail_walk_parallel_returns_false_for_single_pending_stub() {
        // One pending stub: no parallelism win; the serial walker
        // (with its retry semantics) is fine. The parallel path
        // would just incur thread-spawn overhead and the same
        // wall-clock as serial.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let plan_dir = project.join("docs/impl-plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(
            plan_dir.join("milestone-01-foo.md"),
            "# 01\n<!-- detail-pending\n",
        )
        .unwrap();
        let state = State::new(crate::state::Flow::DirectModeling, "DM2cd");
        state.save(&project.join(".sim-flow")).unwrap();

        let mut opts = options_with_llm("mock", None, None);
        opts.project_dir = project;
        opts.max_parallel_requests = 0;
        let step = registry_for(crate::state::Flow::DirectModeling)
            .get("DM2cd")
            .unwrap()
            .clone();
        let mut host = TestHost::new();
        let mode = Arc::new(AtomicU8::new(STEP_MODE_AUTO));
        let mut auto_host = AutoPresenter::new(&mut host, mode);
        let mut mock = crate::session::MockAgent::new();
        let ran = run_plan_detail_walk_parallel(&opts, &step, &mut auto_host, &mut mock).unwrap();
        assert!(
            !ran,
            "single pending stub must opt out of the parallel path"
        );
    }

    #[test]
    fn run_plan_detail_walk_parallel_returns_false_for_non_placeholder_walk() {
        // Execution-mode walks (DM2d / DM3b / DM3c / DM4b) share
        // src/ writes across milestones, so per-milestone Work
        // sessions running in parallel would race on the source
        // tree. The dispatcher refuses to engage on those.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let plan_dir = project.join("docs/impl-plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        // Two pending DM2d-style milestone files (checkbox mode).
        std::fs::write(
            plan_dir.join("milestone-01-foo.md"),
            "# 01\n## Tasks\n- [ ] task\n",
        )
        .unwrap();
        std::fs::write(
            plan_dir.join("milestone-02-bar.md"),
            "# 02\n## Tasks\n- [ ] task\n",
        )
        .unwrap();
        let state = State::new(crate::state::Flow::DirectModeling, "DM2d");
        state.save(&project.join(".sim-flow")).unwrap();

        let mut opts = options_with_llm("mock", None, None);
        opts.project_dir = project;
        opts.max_parallel_requests = 0;
        // DM2d's walk has placeholder_marker = None -> not a
        // plan-detail walk -> dispatcher returns false.
        let step = registry_for(crate::state::Flow::DirectModeling)
            .get("DM2d")
            .unwrap()
            .clone();
        let mut host = TestHost::new();
        let mode = Arc::new(AtomicU8::new(STEP_MODE_AUTO));
        let mut auto_host = AutoPresenter::new(&mut host, mode);
        let mut mock = crate::session::MockAgent::new();
        let ran = run_plan_detail_walk_parallel(&opts, &step, &mut auto_host, &mut mock).unwrap();
        assert!(
            !ran,
            "execution-mode walks (DM2d et al.) must NOT engage the parallel path"
        );
    }

    #[test]
    fn resolve_llm_for_kind_work_returns_primary_stack() {
        let opts = options_with_llm("vllm", Some("qwen3.6"), Some("http://localhost:8012/v1"));
        let resolved = resolve_llm_for_kind(&opts, crate::client::SessionKind::Work);
        assert_eq!(resolved.backend, "vllm");
        assert_eq!(resolved.model.as_deref(), Some("qwen3.6"));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("http://localhost:8012/v1")
        );
    }

    #[test]
    fn resolve_llm_for_kind_critique_falls_back_to_primary_when_no_override() {
        // No critique override set -> critique uses the work stack
        // exactly. Behavior matches every pre-feature run, so adding
        // the per-kind routing doesn't change anyone's existing
        // dispatches unless they opt in.
        let opts = options_with_llm("vllm", Some("qwen3.6"), Some("http://localhost:8012/v1"));
        let resolved = resolve_llm_for_kind(&opts, crate::client::SessionKind::Critique);
        assert_eq!(resolved.backend, "vllm");
        assert_eq!(resolved.model.as_deref(), Some("qwen3.6"));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("http://localhost:8012/v1")
        );
    }

    #[test]
    fn resolve_llm_for_kind_critique_uses_full_override() {
        // The canonical use case: work on vLLM, critique on
        // Anthropic. Every critique knob is set and the work-side
        // stack is unaffected.
        let mut opts = options_with_llm("vllm", Some("qwen3.6"), Some("http://localhost:8012/v1"));
        opts.critique_llm_backend = Some("anthropic".into());
        opts.critique_llm_model = Some("claude-3-5-sonnet-latest".into());
        opts.critique_llm_model_family_id = Some("claude_messages".into());
        opts.critique_llm_runtime_profile_id = Some("anthropic_messages".into());
        opts.critique_llm_base_url = Some("https://api.anthropic.com".into());

        let work = resolve_llm_for_kind(&opts, crate::client::SessionKind::Work);
        assert_eq!(work.backend, "vllm");
        assert_eq!(work.model.as_deref(), Some("qwen3.6"));

        let crit = resolve_llm_for_kind(&opts, crate::client::SessionKind::Critique);
        assert_eq!(crit.backend, "anthropic");
        assert_eq!(crit.model.as_deref(), Some("claude-3-5-sonnet-latest"));
        assert_eq!(crit.model_family_id.as_deref(), Some("claude_messages"));
        assert_eq!(
            crit.runtime_profile_id.as_deref(),
            Some("anthropic_messages")
        );
        assert_eq!(crit.base_url.as_deref(), Some("https://api.anthropic.com"));
    }

    #[test]
    fn resolve_llm_for_kind_critique_partial_override_inherits_per_field() {
        // Override JUST the backend; everything else falls back to
        // the work-side stack. This is what the user actually wants
        // when they swap backends but keep the model id meaningful
        // (e.g. an Anthropic backend that knows how to resolve a
        // model string they typed on the work side).
        let mut opts = options_with_llm("vllm", Some("qwen3.6"), Some("http://localhost:8012/v1"));
        opts.critique_llm_backend = Some("anthropic".into());
        // model / base_url left unset

        let crit = resolve_llm_for_kind(&opts, crate::client::SessionKind::Critique);
        assert_eq!(crit.backend, "anthropic");
        // Inherits the work-side knobs verbatim.
        assert_eq!(crit.model.as_deref(), Some("qwen3.6"));
        assert_eq!(crit.base_url.as_deref(), Some("http://localhost:8012/v1"));
    }

    #[test]
    fn read_gate_findings_handles_numbered_bold_markdown() {
        // Replaces the old `strip_finding_prefix` test. Now that
        // `read_blockers` delegates to `Critique::parse`, the
        // expectations cover every form the gate parser
        // recognizes. Mirrors the extension's parser regression
        // test in critiques.test.ts.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DM0-critique.md"),
            "\
- BLOCKER: list-style finding
* BLOCKER: asterisk-list finding
  - UNRESOLVED: ignored (not a blocker)
1. **BLOCKER: numbered + bold finding**
12. **UNRESOLVED: also ignored**
BLOCKER: bare-line finding
## BLOCKER: heading-style finding
### ❌ BLOCKER: heading + emoji finding
not a finding
",
        )
        .unwrap();
        let findings = read_gate_findings(tmp.path(), "DM0");
        assert_eq!(findings.len(), 8, "got {findings:?}");
        assert!(findings.iter().any(|b| b.contains("list-style")));
        assert!(findings.iter().any(|b| b.contains("asterisk-list")));
        assert!(
            findings
                .iter()
                .any(|b| b.contains("ignored (not a blocker)"))
        );
        assert!(findings.iter().any(|b| b.contains("numbered + bold")));
        assert!(findings.iter().any(|b| b.contains("also ignored")));
        assert!(findings.iter().any(|b| b.contains("bare-line")));
        assert!(findings.iter().any(|b| b.contains("heading-style")));
        assert!(findings.iter().any(|b| b.contains("heading + emoji")));
    }

    #[test]
    fn read_gate_findings_returns_empty_when_critique_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_gate_findings(tmp.path(), "DM0").is_empty());
    }

    #[test]
    fn read_gate_findings_extracts_gate_failing_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DM0-critique.md"),
            "# DM0 Critique\n\n## Issues\n\n\
             1. **BLOCKER: missing clock frequency.**\n   ...\n\n\
             2. **UNRESOLVED: minor wording.**\n\n\
             3. **BLOCKER: bad pinout.**\n\n\
             4. RESOLVED: cleaned up.\n",
        )
        .unwrap();
        let findings = read_gate_findings(tmp.path(), "DM0");
        assert_eq!(findings.len(), 3);
        assert!(findings[0].starts_with("BLOCKER: missing clock frequency"));
        assert!(findings[1].starts_with("UNRESOLVED: minor wording"));
        assert!(findings[2].starts_with("BLOCKER: bad pinout"));
    }

    #[test]
    fn read_gate_findings_treats_unresolved_only_critique_as_gate_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DM0-critique.md"),
            "# DM0 Critique\n\n## Issues\n\n\
             1. UNRESOLVED: minor wording nit.\n\n\
             2. UNRESOLVED: future cleanup note.\n",
        )
        .unwrap();
        let findings = read_gate_findings(tmp.path(), "DM0");
        assert_eq!(findings.len(), 2, "got {findings:?}");
        assert!(findings[0].starts_with("UNRESOLVED: minor wording nit."));
        assert!(findings[1].starts_with("UNRESOLVED: future cleanup note."));
    }

    // -----------------------------------------------------------------
    // AutoPresenter interception
    // -----------------------------------------------------------------

    fn auto_host_with_mode(
        mode: StepMode,
        inner: &mut TestHost,
    ) -> (AutoPresenter<'_, TestHost>, Arc<AtomicU8>) {
        let flag = Arc::new(AtomicU8::new(step_mode_to_u8(mode)));
        (AutoPresenter::new(inner, flag.clone()), flag)
    }

    #[test]
    fn set_step_mode_updates_flag_and_emits_event() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::SetStepMode {
            mode: StepMode::Manual,
        });
        let (mut host, flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        // The intercepted SetStepMode flips the flag and continues
        // reading; with no follow-up event the inner read returns None.
        let r = host.recv().unwrap();
        assert!(r.is_none());
        assert_eq!(
            step_mode_from_u8(flag.load(Ordering::Acquire)),
            StepMode::Manual
        );
        let saw_changed = inner.written.iter().any(|e| {
            matches!(
                e,
                Event::StepModeChanged {
                    mode: StepMode::Manual
                }
            )
        });
        assert!(saw_changed, "SetStepMode should emit StepModeChanged");
    }

    #[test]
    fn shutdown_in_subsession_returns_cancel() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::Shutdown);
        let (mut host, _flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        host.in_subsession = true;
        let r = host.recv().unwrap();
        assert!(matches!(r, Some(HostEvent::Cancel)));
        assert!(host.shutdown_requested);
    }

    #[test]
    fn shutdown_outside_subsession_passes_through() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::Shutdown);
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = false;
        let r = host.recv().unwrap();
        assert!(matches!(r, Some(HostEvent::Shutdown)));
        assert!(host.shutdown_requested);
    }

    #[test]
    fn manual_command_in_auto_mode_is_rejected_with_diagnostic() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::RunStep {
            step: "DM0".into(),
            kind: SessionKindOut::Work,
        });
        let (mut host, _flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        host.in_subsession = false;
        // RunStep is swallowed in auto mode; subsequent read returns None.
        let r = host.recv().unwrap();
        assert!(r.is_none());
        let saw_warn = inner.written.iter().any(|e| {
            matches!(
                e,
                Event::Diagnostic { level: DiagnosticLevel::Warning, message }
                    if message.contains("auto mode owns step execution")
            )
        });
        assert!(
            saw_warn,
            "auto-mode rejection should emit a Warning Diagnostic"
        );
    }

    #[test]
    fn manual_command_during_subsession_is_rejected() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::RunGate { step: "DM0".into() });
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = true;
        let r = host.recv().unwrap();
        assert!(r.is_none());
        let saw_warn = inner.written.iter().any(|e| {
            matches!(
                e,
                Event::Diagnostic { level: DiagnosticLevel::Warning, message }
                    if message.contains("sub-session is currently running")
            )
        });
        assert!(saw_warn);
    }

    #[test]
    fn manual_command_during_parked_subsession_cancels_and_dispatches() {
        // The user clicks Run Step (or any manual command) while the
        // inner sub-session is parked at RequestUserInput. AutoPresenter
        // should:
        //   1. Cancel the parked inner session so it can unwind.
        //   2. Queue the manual command so the OUTER manual loop
        //      dispatches it on its next read.
        // This is the only way for dashboard buttons to work while a
        // critique session is parked asking the user what to do.
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::RunStep {
            step: "DM0".into(),
            kind: SessionKindOut::Work,
        });
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = true;
        // Simulate the orchestrator parking before the manual
        // command arrives.
        host.send(&Event::RequestUserInput {
            prompt: None,
            placeholder: None,
        })
        .unwrap();
        assert!(host.in_subsession_parked);

        // Manual command arrives -- AutoPresenter returns Cancel to break
        // the inner read.
        let first = host.recv().unwrap();
        assert!(matches!(first, Some(HostEvent::Cancel)));

        // The inner sub-session ends, run_subsession resets
        // in_subsession=false, and the next outer read picks up the
        // queued RunStep.
        host.in_subsession = false;
        host.in_subsession_parked = false;
        let second = host.recv().unwrap();
        match second {
            Some(HostEvent::RunStep {
                step,
                kind: SessionKindOut::Work,
            }) => assert_eq!(step, "DM0"),
            other => panic!("expected queued RunStep, got {other:?}"),
        }

        // No "sub-session is currently running" diagnostic was
        // written -- the parked path is the silent-accept path.
        let saw_reject = inner.written.iter().any(|e| {
            matches!(
                e,
                Event::Diagnostic { level: DiagnosticLevel::Warning, message }
                    if message.contains("sub-session is currently running")
            )
        });
        assert!(!saw_reject);
    }

    #[test]
    fn parked_flag_clears_on_active_event_after_user_resumes() {
        // After the user replies, the orchestrator's next active
        // event (assistant-text, request-llm-response, ...) should
        // clear the parked flag so a manual command arriving AFTER
        // the resume but BEFORE the next park is rejected (no race
        // window where the dashboard cancels a sub-session that's
        // mid-stream).
        let mut inner = TestHost::new();
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = true;
        host.send(&Event::RequestUserInput {
            prompt: None,
            placeholder: None,
        })
        .unwrap();
        assert!(host.in_subsession_parked);
        host.send(&Event::AssistantText {
            text: "resuming".into(),
            final_chunk: false,
            tool_calls: Vec::new(),
        })
        .unwrap();
        assert!(!host.in_subsession_parked);
    }

    #[test]
    fn manual_command_in_manual_mode_passes_through_to_caller() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::RunStep {
            step: "DM0".into(),
            kind: SessionKindOut::Work,
        });
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = false;
        let r = host.recv().unwrap();
        match r {
            Some(HostEvent::RunStep {
                step,
                kind: SessionKindOut::Work,
            }) => {
                assert_eq!(step, "DM0");
            }
            other => panic!("expected RunStep, got {other:?}"),
        }
    }

    #[test]
    fn cap_exceeded_diagnostic_queues_cancel() {
        let mut inner = TestHost::new();
        let (mut host, _flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        // The orchestrator emits this kind of Diagnostic when the
        // per-session cap fires.
        host.send(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: "auto: DM0 exceeded max_auto_iters (3); ...".into(),
        })
        .unwrap();
        assert!(host.cap_exceeded);
        // Next read should return the queued Cancel so the inner
        // orchestrator terminates immediately.
        let r = host.recv().unwrap();
        assert!(matches!(r, Some(HostEvent::Cancel)));
    }

    #[test]
    fn auto_mode_swallows_session_end_during_subsession() {
        let mut inner = TestHost::new();
        let (mut host, _flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        host.consume_session_end = true;
        host.send(&Event::SessionEnd {
            reason: SessionEndReason::Completed,
            message: None,
        })
        .unwrap();
        assert!(
            inner
                .written
                .iter()
                .all(|e| !matches!(e, Event::SessionEnd { .. })),
            "auto-mode SessionEnd should be swallowed while consume_session_end is set"
        );
    }

    #[test]
    fn manual_mode_forwards_session_end_to_inner_host() {
        let mut inner = TestHost::new();
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.consume_session_end = false;
        host.send(&Event::SessionEnd {
            reason: SessionEndReason::Completed,
            message: None,
        })
        .unwrap();
        let saw = inner
            .written
            .iter()
            .any(|e| matches!(e, Event::SessionEnd { .. }));
        assert!(saw, "manual-mode SessionEnd should reach the host");
    }

    /// Both reset entry points (`Reset` HostEvent + CLI `sim-flow
    /// reset`) must clear BOTH critique forms (canonical
    /// `<step>-critique.json` and rendered `<step>-critique.md`)
    /// for the reset target step AND every downstream step. The
    /// bug this guards against: a stale DM3a critique left on
    /// disk after a reset to DM3a makes the next critique session
    /// trigger focused-retry mode against the prior pass's
    /// blockers, which were rendered moot by the reset. Pre-fix,
    /// only the `.md` was cleared, so `Critique::load` (which
    /// resolves the JSON sibling first) kept seeing the stale
    /// findings.
    #[test]
    fn clear_step_collateral_forward_deletes_critiques_for_target_and_downstream() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // Stage critiques for several DM steps in BOTH forms.
        // Reset to DM3a should clear DM3a / DM3b / DM3c / DM4a /
        // DM4b (both forms) but leave DM0-DM2d alone.
        for step in [
            "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2d", "DM3a", "DM3b", "DM3c", "DM4a", "DM4b",
        ] {
            std::fs::write(
                project.join(format!("docs/critiques/{step}-critique.md")),
                format!("# {step} critique\n\n- RESOLVED: stub\n"),
            )
            .unwrap();
            std::fs::write(
                project.join(format!("docs/critiques/{step}-critique.json")),
                format!(r#"{{"step":"{step}","summary":"","findings":[],"notes":""}}"#),
            )
            .unwrap();
        }

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);

        assert!(failures.is_empty(), "got failures: {failures:?}");

        // Deleted set must include every DM3a-onwards critique in
        // BOTH forms.
        for step in ["DM3a", "DM3b", "DM3c", "DM4a", "DM4b"] {
            for ext in ["md", "json"] {
                let p = project.join(format!("docs/critiques/{step}-critique.{ext}"));
                assert!(
                    !p.exists(),
                    "{step}-critique.{ext} should be deleted by reset to DM3a"
                );
                assert!(
                    deleted.iter().any(|d| d == &p),
                    "{step}-critique.{ext} should be in the deleted list"
                );
            }
        }
        // Upstream critiques (both forms) must survive untouched.
        for step in ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2d"] {
            for ext in ["md", "json"] {
                let p = project.join(format!("docs/critiques/{step}-critique.{ext}"));
                assert!(
                    p.exists(),
                    "{step}-critique.{ext} should NOT be deleted by reset to DM3a"
                );
            }
        }
    }

    /// Reset must also work when only the canonical JSON form is on
    /// disk (the orchestrator's render-on-write usually produces
    /// both, but a transient state where only the JSON exists is
    /// possible after a render failure or partial migration).
    #[test]
    fn clear_step_collateral_forward_deletes_json_only_critiques() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // JSON-only stage for the target step.
        std::fs::write(
            project.join("docs/critiques/DM3a-critique.json"),
            r#"{"step":"DM3a","summary":"","findings":[],"notes":""}"#,
        )
        .unwrap();
        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");
        let p = project.join("docs/critiques/DM3a-critique.json");
        assert!(!p.exists(), "DM3a-critique.json should be deleted");
        assert!(deleted.iter().any(|d| d == &p));
    }

    /// Upstream-protection: when DM4b's coarse `["docs/analysis/"]`
    /// `work_artifact` would otherwise sweep DM2a/DM2b's specific
    /// files (`decomposition.md`, `pipeline-mapping.md`,
    /// `data-movement.md`), the cleanup walker must do a selective
    /// per-file delete instead of `remove_dir_all`. Same idea for
    /// any other coarse-claim overlap.
    #[test]
    fn clear_step_collateral_forward_protects_upstream_files_inside_shared_dirs() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/analysis")).unwrap();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // Upstream-owned (DM2a / DM2b).
        std::fs::write(project.join("docs/analysis/decomposition.md"), "DM2a\n").unwrap();
        std::fs::write(project.join("docs/analysis/pipeline-mapping.md"), "DM2b\n").unwrap();
        std::fs::write(project.join("docs/analysis/data-movement.md"), "DM2a\n").unwrap();
        // DM4b-era report file that DOES belong to the reset scope.
        std::fs::write(project.join("docs/analysis/throughput.md"), "DM4b report\n").unwrap();
        // Auxiliary file that's not in any work_artifact set.
        // Conservative behavior: leave it alone (the directory has
        // upstream-protected children, so we don't `remove_dir_all`).
        std::fs::write(project.join("docs/analysis/.gitkeep"), "").unwrap();

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (_deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");

        // Upstream files survive.
        assert!(
            project.join("docs/analysis/decomposition.md").exists(),
            "DM2a's decomposition.md must survive a reset to DM3a"
        );
        assert!(
            project.join("docs/analysis/pipeline-mapping.md").exists(),
            "DM2b's pipeline-mapping.md must survive a reset to DM3a"
        );
        assert!(
            project.join("docs/analysis/data-movement.md").exists(),
            "DM2a's data-movement.md must survive a reset to DM3a"
        );
        // DM4b's report inside the same dir gets cleaned up.
        assert!(
            !project.join("docs/analysis/throughput.md").exists(),
            "DM4b's throughput.md must be deleted by reset to DM3a"
        );
        // The directory itself stays in place because protected
        // children still need a parent.
        assert!(
            project.join("docs/analysis").is_dir(),
            "docs/analysis/ must stay as a directory; protected children need it"
        );
    }

    /// Scaffolding files inside a step's exclusive work_artifact
    /// directory must survive a reset. The bug this regression-
    /// tests: when `docs/test-plan/` is DM3a's work_artifact and
    /// nothing upstream lives inside it, the directory delete
    /// path would `remove_dir_all` the whole directory -- sweeping
    /// the `coverage.md.tmpl` and other `.tmpl` scaffolding the
    /// project seeded at `sim-flow new model` time. The reset must
    /// fall back to a selective walk when scaffolding files are
    /// present, even with no upstream-protected descendants.
    #[test]
    fn clear_step_collateral_forward_preserves_tmpl_inside_exclusive_dir() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/test-plan")).unwrap();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // DM3a's work_artifact (entire docs/test-plan/ dir).
        std::fs::write(project.join("docs/test-plan/test-plan.md"), "# old\n").unwrap();
        std::fs::write(
            project.join("docs/test-plan/test-milestone-01-smoke.md"),
            "- [ ] from prior pass\n",
        )
        .unwrap();
        // Scaffolding seeded by sim-flow new model. Must survive.
        std::fs::write(
            project.join("docs/test-plan/coverage.md.tmpl"),
            "template body\n",
        )
        .unwrap();
        std::fs::write(
            project.join("docs/test-plan/test-plan.md.tmpl"),
            "template body\n",
        )
        .unwrap();
        std::fs::write(project.join("docs/test-plan/.gitkeep"), "").unwrap();

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (_deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");

        // Generated artifacts gone.
        assert!(
            !project.join("docs/test-plan/test-plan.md").exists(),
            "test-plan.md (generated) should be deleted"
        );
        assert!(
            !project
                .join("docs/test-plan/test-milestone-01-smoke.md")
                .exists(),
            "milestone files (generated) should be deleted"
        );
        // Scaffolding must survive.
        assert!(
            project.join("docs/test-plan/coverage.md.tmpl").exists(),
            "coverage.md.tmpl is scaffolding; reset must NOT delete it"
        );
        assert!(
            project.join("docs/test-plan/test-plan.md.tmpl").exists(),
            "test-plan.md.tmpl is scaffolding; reset must NOT delete it"
        );
        assert!(
            project.join("docs/test-plan/.gitkeep").exists(),
            ".gitkeep is scaffolding; reset must NOT delete it"
        );
        // The dir itself stays because the scaffolding files need a parent.
        assert!(
            project.join("docs/test-plan").is_dir(),
            "docs/test-plan/ must remain to hold the surviving scaffolding"
        );
    }

    /// Project scaffolding -- `.gitkeep` markers and `*.tmpl`
    /// template files seeded by `sim-flow new model` -- survive a
    /// reset even when they sit inside a step's work_artifact
    /// directory. They aren't owned by any flow step; deleting
    /// them on reset would force the user to git-restore template
    /// scaffolding every time they reset a step.
    #[test]
    fn clear_step_collateral_forward_preserves_reset_scaffolding() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/analysis")).unwrap();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // DM4b-era report (downstream-owned -> deleted).
        std::fs::write(project.join("docs/analysis/throughput.md"), "report\n").unwrap();
        // Project scaffolding (no step claims it -> survives).
        std::fs::write(project.join("docs/analysis/.gitkeep"), "").unwrap();
        std::fs::write(
            project.join("docs/analysis/decomposition.md.tmpl"),
            "template\n",
        )
        .unwrap();
        // Upstream-owned (DM2a) file.
        std::fs::write(
            project.join("docs/analysis/decomposition.md"),
            "DM2a output\n",
        )
        .unwrap();

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (_deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");

        assert!(
            !project.join("docs/analysis/throughput.md").exists(),
            "downstream report should be cleaned"
        );
        assert!(
            project.join("docs/analysis/.gitkeep").exists(),
            ".gitkeep is scaffolding; reset must not touch it"
        );
        assert!(
            project.join("docs/analysis/decomposition.md.tmpl").exists(),
            "*.tmpl files are scaffolding; reset must not touch them"
        );
        assert!(
            project.join("docs/analysis/decomposition.md").exists(),
            "DM2a's decomposition.md is upstream-protected"
        );
    }

    /// Reset to a step clears that step's `work_artifacts` (and
    /// downstream steps' artifacts) when nothing upstream claims
    /// them. For DM3a: `docs/test-plan/` is exclusively DM3a's, so
    /// it gets wiped wholesale. `tests/` is shared with DM2d
    /// (upstream) so the directory survives intact -- a known
    /// limitation of coarse `work_artifacts` declarations; the
    /// alternative was the bug that wiped DM2d's elaboration test.
    #[test]
    fn clear_step_collateral_forward_deletes_exclusive_artifacts_only() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/test-plan")).unwrap();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        std::fs::create_dir_all(project.join("tests")).unwrap();
        // Old-flow leftovers under docs/test-plan/ (DM3a's
        // exclusive artifact -> deleted).
        std::fs::write(project.join("docs/test-plan/test-plan.md"), "# old\n").unwrap();
        std::fs::write(project.join("docs/test-plan/smoke.md"), "# old\n").unwrap();
        std::fs::write(project.join("docs/test-plan/edge.md"), "# old\n").unwrap();
        // tests/ is shared between DM2d and DM3b/DM3c -- the
        // upstream-protection logic preserves the dir as a whole
        // because DM2d (upstream of DM3a) claims it. Stale DM3b
        // artifacts inside survive too, but that's the safer
        // tradeoff vs. wiping DM2d's smoke tests.
        std::fs::write(project.join("tests/testbench.rs"), "// old\n").unwrap();
        std::fs::write(project.join("tests/elaboration.rs"), "// dm2d\n").unwrap();
        // Pre-DM3a artifacts that must NOT be deleted.
        std::fs::create_dir_all(project.join("docs/impl-plan")).unwrap();
        std::fs::write(project.join("docs/impl-plan/plan.md"), "# keep\n").unwrap();
        std::fs::write(project.join("docs/spec.md"), "# keep\n").unwrap();

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (_deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");

        // DM3a's exclusive artifact `docs/test-plan/` is gone.
        assert!(
            !project.join("docs/test-plan").exists(),
            "docs/test-plan/ is DM3a-exclusive; reset to DM3a should remove it"
        );
        // tests/ survives because DM2d (upstream) also claims it.
        // DM2d's elaboration test specifically must survive; the
        // stale DM3b file inside is collateral that the
        // re-running DM3b will overwrite.
        assert!(
            project.join("tests").exists(),
            "tests/ is shared with DM2d; reset to DM3a must not delete the dir"
        );
        assert!(
            project.join("tests/elaboration.rs").exists(),
            "tests/elaboration.rs is DM2d's; reset to DM3a MUST NOT delete it"
        );
        // Upstream artifacts that don't share a dir survive.
        assert!(
            project.join("docs/impl-plan/plan.md").exists(),
            "docs/impl-plan/plan.md is DM2c's; reset to DM3a must NOT touch it"
        );
        assert!(
            project.join("docs/spec.md").exists(),
            "docs/spec.md is DM0's; reset to DM3a must NOT touch it"
        );
    }

    /// Manifest-driven reset: when each step has recorded what it
    /// wrote, the shared-directory case (`tests/` claimed by both
    /// upstream DM2d and downstream DM3b/c) is cleaned at file
    /// granularity. DM2d's `tests/elaboration.rs` survives; DM3b's
    /// `tests/testbench/mod.rs` is swept. Without this the run that
    /// re-enters DM3b inherits stale scaffolding from the failed
    /// prior run, and the agent has to detect and reconcile it
    /// before making progress.
    #[test]
    fn clear_step_collateral_forward_uses_manifests_for_shared_dirs() {
        use crate::__internal::manifest;
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("tests/testbench")).unwrap();
        std::fs::create_dir_all(project.join("tests/edge")).unwrap();
        // Files DM2d (upstream) wrote.
        std::fs::write(project.join("tests/elaboration.rs"), "// dm2d\n").unwrap();
        std::fs::write(project.join("tests/skeleton_test.rs"), "// dm2d\n").unwrap();
        // Files DM3b (downstream of DM3a) wrote.
        std::fs::write(project.join("tests/testbench/mod.rs"), "// dm3b\n").unwrap();
        std::fs::write(project.join("tests/testbench/sequences.rs"), "// dm3b\n").unwrap();
        std::fs::write(project.join("tests/testbench.rs"), "// dm3b\n").unwrap();
        // Files DM3c (downstream of DM3a) wrote.
        std::fs::write(project.join("tests/edge.rs"), "// dm3c\n").unwrap();
        std::fs::write(project.join("tests/edge/all_zeros.rs"), "// dm3c\n").unwrap();

        // Record the manifests as the real run would have.
        manifest::record_write(project, "DM2d", "tests/elaboration.rs");
        manifest::record_write(project, "DM2d", "tests/skeleton_test.rs");
        manifest::record_write(project, "DM3b", "tests/testbench/mod.rs");
        manifest::record_write(project, "DM3b", "tests/testbench/sequences.rs");
        manifest::record_write(project, "DM3b", "tests/testbench.rs");
        manifest::record_write(project, "DM3c", "tests/edge.rs");
        manifest::record_write(project, "DM3c", "tests/edge/all_zeros.rs");

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (_deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);
        assert!(failures.is_empty(), "got failures: {failures:?}");

        // Upstream (DM2d) files survive at file granularity.
        assert!(
            project.join("tests/elaboration.rs").exists(),
            "DM2d's elaboration.rs is in its manifest -> protected"
        );
        assert!(
            project.join("tests/skeleton_test.rs").exists(),
            "DM2d's skeleton_test.rs is in its manifest -> protected"
        );
        // Downstream (DM3b / DM3c) files in the shared dir are now
        // swept -- this is the new behavior the manifest enables.
        assert!(
            !project.join("tests/testbench/mod.rs").exists(),
            "DM3b's testbench/mod.rs must be deleted on reset to DM3a"
        );
        assert!(
            !project.join("tests/testbench/sequences.rs").exists(),
            "DM3b's testbench/sequences.rs must be deleted on reset to DM3a"
        );
        assert!(
            !project.join("tests/testbench.rs").exists(),
            "DM3b's testbench.rs must be deleted on reset to DM3a"
        );
        assert!(
            !project.join("tests/edge.rs").exists(),
            "DM3c's edge.rs must be deleted on reset to DM3a"
        );
        assert!(
            !project.join("tests/edge/all_zeros.rs").exists(),
            "DM3c's edge/all_zeros.rs must be deleted on reset to DM3a"
        );

        // Manifests for the cleaned steps are also gone so a follow-
        // up reset doesn't try to re-delete non-existent files.
        assert!(manifest::step_paths(project, "DM3b").is_empty());
        assert!(manifest::step_paths(project, "DM3c").is_empty());
        // Upstream manifest preserved.
        assert!(!manifest::step_paths(project, "DM2d").is_empty());
    }
}
