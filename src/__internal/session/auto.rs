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
//! `AutoHost` wrapper that intercepts host reads. `SetStepMode` host
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
//! `AutoHost` reuses the existing `run_session` entry point and:
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

use tracing::{debug, info, warn};

use crate::Result;
use crate::session::host::Host;
use crate::session::orchestrator::{
    OrchestratorOptions, run_session, step_descriptor_for_protocol,
};
use crate::session::protocol::{
    DiagnosticLevel, Event, GateFailureOut, HostEvent, HostInfo, PROTOCOL_VERSION,
    SessionEndReason, SessionKindOut, SessionTag, StepMode,
};
use crate::state::State;
use crate::steps::registry_for;

/// Inputs for `run_auto`. The driver picks up the active flow's
/// remaining steps starting from `state.current_step`.
pub struct AutoOptions {
    pub project_dir: PathBuf,
    pub foundation_root: PathBuf,
    pub llm_backend: String,
    pub llm_model: Option<String>,
    /// Per-session structural-gate iteration cap (forwarded to the
    /// orchestrator's auto mode).
    pub max_auto_iters: u32,
    /// Cross-session retry cap. Each retry re-runs the work session
    /// for the same step (the orchestrator inlines the critique file
    /// so the agent sees what to fix).
    pub max_critique_iters: u32,
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
    /// Initial step-axis mode. `Auto` walks `current_step` to end of
    /// flow without user input. `Manual` parks the orchestrator after
    /// the hello handshake and dispatches sub-sessions only in
    /// response to host commands. The mode flag is also live-mutable
    /// mid-run via the `SetStepMode` host event.
    pub step_mode: StepMode,
}

pub fn run_auto<H: Host>(opts: AutoOptions, host: &mut H) -> Result<()> {
    info!(
        project = %opts.project_dir.display(),
        backend = %opts.llm_backend,
        model = opts.llm_model.as_deref().unwrap_or("(default)"),
        step_mode = ?opts.step_mode,
        "run_auto starting"
    );
    let mode = Arc::new(AtomicU8::new(step_mode_to_u8(opts.step_mode)));
    let mut auto_host = AutoHost::new(host, mode);

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
    auto_host.write(&Event::StepModeChanged {
        mode: opts.step_mode,
    })?;

    let outcome = loop {
        if auto_host.shutdown_requested {
            break RunOutcome::Shutdown;
        }
        match auto_host.current_step_mode() {
            StepMode::Auto => {
                debug!("dispatching to run_auto_loop (auto mode)");
                match run_auto_loop(&opts, &mut auto_host)? {
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
                match wait_for_command(&opts, &mut auto_host)? {
                    ManualOutcome::Continue => continue,
                    ManualOutcome::Shutdown => break RunOutcome::Shutdown,
                    ManualOutcome::HostClosed => break RunOutcome::HostClosed,
                }
            }
        }
    };

    info!(outcome = ?outcome, "run_auto exiting");
    // Final SessionEnd. AutoHost forwards this to the underlying host
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
    auto_host.write(&Event::SessionEnd { reason, message })?;
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

fn run_auto_loop<H: Host>(
    opts: &AutoOptions,
    auto_host: &mut AutoHost<H>,
) -> Result<AutoLoopOutcome> {
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

    for (step_pos, step_id) in remaining.iter().enumerate() {
        let is_first_step = step_pos == 0;
        let mut critique_iters: u32 = 0;
        // Track the prior critique's blocker count so each retry can
        // report the delta (`12 -> 4 (-8)`) under
        // `sim_flow::metrics`. None until the first critique pass
        // measures.
        let mut prev_blocker_count: Option<usize> = None;
        let step_started = std::time::Instant::now();

        loop {
            if let Some(o) = check_pre_subsession(auto_host) {
                return Ok(o);
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
            )?;
            if let Some(o) = check_post_subsession(
                auto_host,
                step_id,
                crate::client::SessionKind::Critique,
                opts,
            )? {
                return Ok(o);
            }

            // Did the critique flag any blockers? If yes and we have
            // budget, loop back to work. Otherwise proceed to advance.
            let blockers = read_blockers(&opts.project_dir, step_id);
            let cur_count = blockers.len();
            // Per-critique-pass delta. `delta` is signed because a
            // retry can introduce regressions (count strictly
            // increasing) -- worth surfacing so a stuck-loop is
            // visible without parsing the critique file.
            let delta = prev_blocker_count.map(|prev| cur_count as i64 - prev as i64);
            tracing::info!(
                target: "sim_flow::metrics",
                event = "critique_pass",
                step = %step_id,
                pass_index = critique_iters,
                blockers = cur_count,
                prev_blockers = ?prev_blocker_count,
                delta = ?delta,
            );
            prev_blocker_count = Some(cur_count);
            if blockers.is_empty() {
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
                    AdvanceOutcome::Advanced => break,
                    AdvanceOutcome::MoreMilestonesPending => {
                        critique_iters = 0;
                        prev_blocker_count = None;
                        auto_host.write(&Event::Diagnostic {
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
            if critique_iters > opts.max_critique_iters {
                auto_host.write(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "auto: {} critique still has {} blocker(s) after {} retries; flipping to manual mode. \
                         Use the dashboard's per-step controls to inspect, re-run, or advance. Raise \
                         `sim-flow.auto.maxCritiqueIterations` and toggle back to auto if you want more retries per resume \
                         (current cap: {}).",
                        step_id,
                        blockers.len(),
                        critique_iters - 1,
                        opts.max_critique_iters,
                    ),
                })?;
                flip_to_manual(auto_host)?;
                return Ok(AutoLoopOutcome::FlippedToManual);
            }
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Info,
                message: format!(
                    "auto: {} critique reported {} blocker(s); re-running work (retry {}/{})",
                    step_id,
                    blockers.len(),
                    critique_iters,
                    opts.max_critique_iters,
                ),
            })?;
            // Loop body re-runs work; the orchestrator's
            // build_session_inputs will inline the critique file so
            // the agent sees the findings.
        }
        // Inner loop break-only path: clean critique advanced the
        // step. Outer for-loop moves to the next step.
    }

    Ok(AutoLoopOutcome::Completed)
}

fn check_pre_subsession<H: Host>(auto_host: &AutoHost<H>) -> Option<AutoLoopOutcome> {
    if auto_host.shutdown_requested {
        return Some(AutoLoopOutcome::Shutdown);
    }
    if matches!(auto_host.current_step_mode(), StepMode::Manual) {
        return Some(AutoLoopOutcome::FlippedToManual);
    }
    None
}

fn check_post_subsession<H: Host>(
    auto_host: &mut AutoHost<H>,
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

fn flip_to_manual<H: Host>(auto_host: &mut AutoHost<H>) -> Result<()> {
    let prev = auto_host.current_step_mode();
    auto_host.store_step_mode(StepMode::Manual);
    if !matches!(prev, StepMode::Manual) {
        warn!("flipping step mode auto -> manual (cap exceeded or gate failure)");
        auto_host.write(&Event::StepModeChanged {
            mode: StepMode::Manual,
        })?;
    }
    Ok(())
}

fn emit_cap_exceeded_diagnostic<H: Host>(
    auto_host: &mut AutoHost<H>,
    step_id: &str,
    kind: crate::client::SessionKind,
    opts: &AutoOptions,
) -> Result<()> {
    let kind_s = match kind {
        crate::client::SessionKind::Work => "work",
        crate::client::SessionKind::Critique => "critique",
    };
    auto_host.write(&Event::Diagnostic {
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
fn perform_initial_handshake<H: Host>(
    opts: &AutoOptions,
    auto_host: &mut AutoHost<H>,
) -> Result<()> {
    let hello_version = match auto_host.read()? {
        Some(HostEvent::Hello {
            protocol_version, ..
        }) => protocol_version,
        Some(other) => {
            auto_host.write(&Event::SessionEnd {
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
        auto_host.write(&Event::SessionEnd {
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
    auto_host.write(&Event::HelloAck {
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

fn run_subsession<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto: bool,
    host: &mut AutoHost<H>,
    consume_end: bool,
    synth_hello: bool,
) -> Result<()> {
    if synth_hello {
        host.queue_synthetic_hello();
    }
    host.consume_session_end = consume_end;
    host.cap_exceeded = false;
    host.in_subsession = true;
    let kind_out = session_kind_to_protocol(kind);
    info!(step = step_id, kind = ?kind_out, auto, "sub-session starting");
    // Bracket the inner run_session with SubSessionStarted /
    // SubSessionEnded so the dashboard can disable per-step
    // buttons while the orchestrator is busy. Emitted regardless
    // of step-mode; auto mode drives multiple bracketed sub-
    // sessions per outer run, manual mode drives one per
    // dispatched command.
    host.write(&Event::SubSessionStarted {
        step: step_id.to_string(),
        kind: kind_out,
    })?;
    let session_opts = OrchestratorOptions {
        project_dir: opts.project_dir.clone(),
        foundation_root: opts.foundation_root.clone(),
        step_id: step_id.to_string(),
        kind,
        candidate: None,
        llm_backend: opts.llm_backend.clone(),
        llm_model: opts.llm_model.clone(),
        auto,
        max_auto_iters: opts.max_auto_iters,
        max_llm_requests: opts.max_llm_requests,
        max_identical_responses: opts.max_identical_responses,
        // JSONL host path: the orchestrator extracts fenced
        // ` ```<path>` blocks from each turn and writes them. Use
        // the artifact-write convention.
        agent_has_native_fs_tools: false,
    };
    let result = run_session(session_opts, host);
    host.in_subsession = false;
    // run_session returns Ok(()) for both clean completion and
    // user-initiated Cancel (the Cancel path emits its own internal
    // SessionEnd and returns Ok). Err is genuine protocol / I/O /
    // state error — surface that as "error" so the dashboard can
    // distinguish.
    let outcome = if result.is_ok() { "completed" } else { "error" };
    info!(step = step_id, kind = ?kind_out, outcome, "sub-session ended");
    // Best-effort: if writing the closing event fails (e.g. host
    // socket already closed), keep the inner `result` to surface.
    let _ = host.write(&Event::SubSessionEnded {
        step: step_id.to_string(),
        kind: kind_out,
        outcome: outcome.into(),
    });
    result
}

fn session_kind_to_protocol(kind: crate::client::SessionKind) -> SessionKindOut {
    match kind {
        crate::client::SessionKind::Work => SessionKindOut::Work,
        crate::client::SessionKind::Critique => SessionKindOut::Critique,
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

fn try_advance_classified<H: Host>(
    project_dir: &Path,
    step_id: &str,
    host: &mut AutoHost<H>,
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
    // Gate dirty. For milestone-walk steps, classify whether the
    // ONLY failing checks are MilestonesAllResolved -- which means
    // the agent is mid-walk and the next iteration should be a
    // fresh Work session for the next milestone, not a flip-to-
    // manual.
    let only_milestones_pending = step.milestone_walk.is_some()
        && !report.failures.is_empty()
        && report.failures.iter().all(|f| {
            // The MilestonesAllResolved gate check uses a
            // distinctive description prefix; its failure reason
            // also starts with "milestone files still have
            // unresolved rows". Match either to be robust.
            f.reason
                .contains("milestone files still have unresolved rows")
                || f.reason.contains("no `") && f.reason.contains("NN-*.md` files found")
        });
    if only_milestones_pending {
        return Ok(AdvanceOutcome::MoreMilestonesPending);
    }
    host.write(&Event::Diagnostic {
        level: DiagnosticLevel::Error,
        message: format!(
            "auto: {step_id} gate is not clean after critique; cannot advance. {} failure(s).",
            report.failures.len()
        ),
    })?;
    for f in &report.failures {
        host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("  - {}: {}", f.description, f.reason),
        })?;
    }
    Ok(AdvanceOutcome::Stuck)
}

fn try_advance<H: Host>(project_dir: &Path, step_id: &str, host: &mut AutoHost<H>) -> Result<bool> {
    use crate::gate;
    let dot = project_dir.join(".sim-flow");
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(step_id).ok_or_else(|| {
        crate::Error::InvalidStep(format!("{} is not a {} step", step_id, state.flow.as_str()))
    })?;
    let report = gate::evaluate(project_dir, &step.gate_checks)?;
    if !report.is_clean() {
        host.write(&Event::Diagnostic {
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

    // Commit step artifacts before mutating sim-flow state so a
    // committed git history reflects each gate-clean checkpoint.
    let outcome = crate::git_commit::commit_step_advance(project_dir, step.id, next);
    if let Some(msg) = crate::git_commit::outcome_message(&outcome) {
        host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: msg,
        })?;
    }

    state.mark_passed(step.id, current_iso8601());
    if let Some(next_step) = next {
        state.current_step = next_step.to_string();
    }
    state.save(&dot)?;
    host.write(&Event::StateAdvanced {
        from: step.id.into(),
        to: next.map(String::from),
    })?;
    Ok(true)
}

// ---------------------------------------------------------------------
// Manual command dispatcher.
// ---------------------------------------------------------------------

fn wait_for_command<H: Host>(
    opts: &AutoOptions,
    auto_host: &mut AutoHost<H>,
) -> Result<ManualOutcome> {
    match auto_host.read()? {
        None => Ok(ManualOutcome::HostClosed),
        Some(HostEvent::Shutdown) => Ok(ManualOutcome::Shutdown),
        Some(HostEvent::RunStep { step, kind }) => {
            let session_kind = match kind {
                SessionKindOut::Work => crate::client::SessionKind::Work,
                SessionKindOut::Critique => crate::client::SessionKind::Critique,
            };
            run_manual_subsession(opts, &step, session_kind, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::RunCritique { step }) => {
            run_manual_subsession(opts, &step, crate::client::SessionKind::Critique, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::RunGate { step }) => {
            run_manual_gate(opts, &step, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::Advance { step }) => {
            run_manual_advance(opts, &step, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(HostEvent::Reset { step }) => {
            run_manual_reset(opts, &step, auto_host)?;
            Ok(ManualOutcome::Continue)
        }
        Some(other) => {
            // Stray events while parked. Most aren't meaningful here
            // (UserMessage with nobody listening, leftover LlmChunk,
            // …). Surface a warning so the host operator can see the
            // event was dropped, then keep parking.
            auto_host.write(&Event::Diagnostic {
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

fn run_manual_subsession<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto_host: &mut AutoHost<H>,
) -> Result<()> {
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
    )
}

fn run_manual_gate<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoHost<H>,
) -> Result<()> {
    use crate::gate;
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.write(&Event::Diagnostic {
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
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("RunGate: `{step_id}` is not a {} step", state.flow.as_str()),
            })?;
            return Ok(());
        }
    };
    let report = gate::evaluate(&opts.project_dir, &step.gate_checks)?;
    auto_host.write(&Event::GateResult {
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

fn run_manual_advance<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoHost<H>,
) -> Result<()> {
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("Advance: failed to load state: {err}"),
            })?;
            return Ok(());
        }
    };
    let registry = registry_for(state.flow);
    if registry.get(step_id).is_none() {
        auto_host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!("Advance: `{step_id}` is not a {} step", state.flow.as_str()),
        })?;
        return Ok(());
    }
    // try_advance handles gate evaluation, git commit, mark passed,
    // bump current_step, and emits Diagnostic / StateAdvanced. On a
    // dirty gate it writes a Diagnostic and returns false; we stay
    // parked.
    let _ = try_advance(&opts.project_dir, step_id, auto_host)?;
    Ok(())
}

fn run_manual_reset<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    auto_host: &mut AutoHost<H>,
) -> Result<()> {
    let dot = opts.project_dir.join(".sim-flow");
    let mut state = match State::load(&dot) {
        Ok(s) => s,
        Err(err) => {
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("Reset: failed to load state: {err}"),
            })?;
            return Ok(());
        }
    };
    let registry = registry_for(state.flow);
    let order: Vec<&'static str> = registry.order_for(state.flow);
    let Some(idx) = order.iter().position(|s| *s == step_id) else {
        auto_host.write(&Event::Diagnostic {
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
        auto_host.write(&Event::Diagnostic {
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
    auto_host.write(&Event::Diagnostic {
        level: DiagnosticLevel::Info,
        message: summary,
    })?;
    for (path, err) in &delete_failures {
        let rel = path
            .strip_prefix(&opts.project_dir)
            .unwrap_or(path)
            .display();
        auto_host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!("Reset: failed to delete {rel}: {err}"),
        })?;
    }
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

    // Build the upstream-protected set: every work_artifact +
    // critique file from steps[0..idx]. Trailing-slash directory
    // markers are normalized off so we can do exact-equality
    // matches against `Path::join(rel)`.
    let mut protected: HashSet<PathBuf> = HashSet::new();
    for upstream in &order[..idx] {
        let Some(step) = registry.get(upstream) else {
            continue;
        };
        for art in step.work_artifacts {
            protected.insert(project_dir.join(art.trim_end_matches('/')));
        }
        protected.insert(project_dir.join(format!("docs/critiques/{}-critique.md", step.id)));
    }

    let mut deleted: Vec<PathBuf> = Vec::new();
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    for downstream in &order[idx..] {
        let Some(step) = registry.get(downstream) else {
            continue;
        };
        for art in step.work_artifacts {
            delete_with_upstream_protection(
                project_dir,
                art,
                &protected,
                &mut deleted,
                &mut failures,
            );
        }
        let critique_rel = format!("docs/critiques/{}-critique.md", step.id);
        delete_with_upstream_protection(
            project_dir,
            &critique_rel,
            &protected,
            &mut deleted,
            &mut failures,
        );
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

fn validate_step_id<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    cmd_label: &str,
    auto_host: &mut AutoHost<H>,
) -> Result<bool> {
    let state = match State::load(&opts.project_dir.join(".sim-flow")) {
        Ok(s) => s,
        Err(err) => {
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("{cmd_label}: failed to load state: {err}"),
            })?;
            return Ok(false);
        }
    };
    let registry = registry_for(state.flow);
    if registry.get(step_id).is_none() {
        auto_host.write(&Event::Diagnostic {
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

/// Findings that prevent the gate from passing. Only `BLOCKER:`
/// lines block advancement. `UNRESOLVED:` is informational -- the
/// model uses it to flag notes/questions/follow-ups it does not
/// consider must-fix. The auto driver MUST match
/// `Finding::is_blocking` in `tools/sim-flow/src/critique.rs` (which
/// the gate's `CritiqueClean` check uses) or it will loop on issues
/// the gate would happily pass.
/// Returns the BLOCKER finding texts the gate's
/// `CritiqueClean` check would flag. Delegates to `Critique::parse`
/// (the gate-side parser) so the auto-driver and gate can never
/// disagree about what counts as a finding. Without this sharing
/// the auto-driver could decide a critique was clean and advance
/// while the gate held it back, or vice versa -- exactly the bug
/// that let DM3b advance past 5 heading-style `## BLOCKER:`
/// findings the gate-side parser missed.
fn read_blockers(project_dir: &Path, step_id: &str) -> Vec<String> {
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
        .map(|f| format!("BLOCKER: {}", f.text()))
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
        HostEvent::LlmChunk { .. } => "LlmChunk",
        HostEvent::LlmEnd { .. } => "LlmEnd",
        HostEvent::LlmError { .. } => "LlmError",
        HostEvent::FollowupSelected { .. } => "FollowupSelected",
        HostEvent::Cancel => "Cancel",
        HostEvent::RunStep { .. } => "RunStep",
        HostEvent::RunCritique { .. } => "RunCritique",
        HostEvent::RunGate { .. } => "RunGate",
        HostEvent::Advance { .. } => "Advance",
        HostEvent::Reset { .. } => "Reset",
        HostEvent::SetStepMode { .. } => "SetStepMode",
        HostEvent::Shutdown => "Shutdown",
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
// AutoHost wrapper
// ---------------------------------------------------------------------

/// `Host` wrapper that lets the auto driver run multiple back-to-back
/// `run_session` calls through one underlying connection while
/// surfacing manual-mode commands and step-mode transitions to the
/// run loop.
pub struct AutoHost<'a, H: Host> {
    inner: &'a mut H,
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
    /// (RunStep, etc.) that arrive in this window are rejected with a
    /// Diagnostic; outside this window the parking loop reads them.
    pub in_subsession: bool,
}

impl<'a, H: Host> AutoHost<'a, H> {
    pub fn new(inner: &'a mut H, step_mode: Arc<AtomicU8>) -> Self {
        Self {
            inner,
            pending_reads: VecDeque::new(),
            consume_session_end: false,
            cap_exceeded: false,
            step_mode,
            shutdown_requested: false,
            in_subsession: false,
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

impl<H: Host> Host for AutoHost<'_, H> {
    fn write(&mut self, event: &Event) -> Result<()> {
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
        if self.consume_session_end && matches!(event, Event::SessionEnd { .. }) {
            self.consume_session_end = false;
            return Ok(());
        }
        self.inner.write(event)
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        loop {
            if let Some(h) = self.pending_reads.pop_front() {
                return Ok(Some(h));
            }
            let next = self.inner.read()?;
            match next {
                Some(HostEvent::SetStepMode { mode }) => {
                    let prev = self.current_step_mode();
                    self.store_step_mode(mode);
                    if prev != mode {
                        info!(from = ?prev, to = ?mode, "step mode flipped by host command");
                        self.inner.write(&Event::StepModeChanged { mode })?;
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
                        self.inner.write(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "ignored {label}: auto mode owns step execution; toggle to manual first."
                            ),
                        })?;
                        continue;
                    }
                    if self.in_subsession {
                        warn!(
                            cmd = label,
                            "rejecting manual command while sub-session in flight"
                        );
                        self.inner.write(&Event::Diagnostic {
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

    #[test]
    fn read_blockers_handles_numbered_bold_markdown() {
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
        let blockers = read_blockers(tmp.path(), "DM0");
        assert_eq!(blockers.len(), 6, "got {blockers:?}");
        assert!(blockers.iter().any(|b| b.contains("list-style")));
        assert!(blockers.iter().any(|b| b.contains("asterisk-list")));
        assert!(blockers.iter().any(|b| b.contains("numbered + bold")));
        assert!(blockers.iter().any(|b| b.contains("bare-line")));
        assert!(blockers.iter().any(|b| b.contains("heading-style")));
        assert!(blockers.iter().any(|b| b.contains("heading + emoji")));
    }

    #[test]
    fn read_blockers_returns_empty_when_critique_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_blockers(tmp.path(), "DM0").is_empty());
    }

    #[test]
    fn read_blockers_extracts_only_blocker_findings() {
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
        let blockers = read_blockers(tmp.path(), "DM0");
        assert_eq!(blockers.len(), 2);
        assert!(blockers[0].starts_with("BLOCKER: missing clock frequency"));
        assert!(blockers[1].starts_with("BLOCKER: bad pinout"));
    }

    #[test]
    fn read_blockers_treats_unresolved_only_critique_as_advanceable() {
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
        let blockers = read_blockers(tmp.path(), "DM0");
        assert!(
            blockers.is_empty(),
            "UNRESOLVED-only critique should not produce blockers, got {blockers:?}",
        );
    }

    // -----------------------------------------------------------------
    // AutoHost interception
    // -----------------------------------------------------------------

    fn auto_host_with_mode(
        mode: StepMode,
        inner: &mut TestHost,
    ) -> (AutoHost<'_, TestHost>, Arc<AtomicU8>) {
        let flag = Arc::new(AtomicU8::new(step_mode_to_u8(mode)));
        (AutoHost::new(inner, flag.clone()), flag)
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
        let r = host.read().unwrap();
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
        let r = host.read().unwrap();
        assert!(matches!(r, Some(HostEvent::Cancel)));
        assert!(host.shutdown_requested);
    }

    #[test]
    fn shutdown_outside_subsession_passes_through() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::Shutdown);
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = false;
        let r = host.read().unwrap();
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
        let r = host.read().unwrap();
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
        let r = host.read().unwrap();
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
    fn manual_command_in_manual_mode_passes_through_to_caller() {
        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::RunStep {
            step: "DM0".into(),
            kind: SessionKindOut::Work,
        });
        let (mut host, _flag) = auto_host_with_mode(StepMode::Manual, &mut inner);
        host.in_subsession = false;
        let r = host.read().unwrap();
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
        host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: "auto: DM0 exceeded max_auto_iters (3); ...".into(),
        })
        .unwrap();
        assert!(host.cap_exceeded);
        // Next read should return the queued Cancel so the inner
        // orchestrator terminates immediately.
        let r = host.read().unwrap();
        assert!(matches!(r, Some(HostEvent::Cancel)));
    }

    #[test]
    fn auto_mode_swallows_session_end_during_subsession() {
        let mut inner = TestHost::new();
        let (mut host, _flag) = auto_host_with_mode(StepMode::Auto, &mut inner);
        host.consume_session_end = true;
        host.write(&Event::SessionEnd {
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
        host.write(&Event::SessionEnd {
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
    /// reset`) must clear `docs/critiques/<step>-critique.md` for
    /// the reset target step AND every downstream step. The bug
    /// this guards against: a stale DM3a-critique.md left on disk
    /// after a reset to DM3a makes the next critique session
    /// trigger focused-retry mode against the prior pass's
    /// blockers, which were rendered moot by the reset.
    #[test]
    fn clear_step_collateral_forward_deletes_critiques_for_target_and_downstream() {
        use crate::__internal::steps::registry_for;
        use crate::state::Flow;

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
        // Stage critiques for several DM steps. Reset to DM3a
        // should clear DM3a / DM3b / DM3c / DM4a / DM4b but leave
        // DM0-DM2d alone.
        for step in [
            "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2d", "DM3a", "DM3b", "DM3c", "DM4a", "DM4b",
        ] {
            std::fs::write(
                project.join(format!("docs/critiques/{step}-critique.md")),
                format!("# {step} critique\n\n- RESOLVED: stub\n"),
            )
            .unwrap();
        }

        let registry = registry_for(Flow::DirectModeling);
        let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);
        let idx = order.iter().position(|s| *s == "DM3a").unwrap();
        let (deleted, failures) = clear_step_collateral_forward(project, idx, &order, &registry);

        assert!(failures.is_empty(), "got failures: {failures:?}");

        // Deleted set must include every DM3a-onwards critique.
        for step in ["DM3a", "DM3b", "DM3c", "DM4a", "DM4b"] {
            let p = project.join(format!("docs/critiques/{step}-critique.md"));
            assert!(
                !p.exists(),
                "{step}-critique.md should be deleted by reset to DM3a"
            );
            assert!(
                deleted.iter().any(|d| d == &p),
                "{step}-critique.md should be in the deleted list"
            );
        }
        // Upstream critiques must survive untouched.
        for step in ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2d"] {
            let p = project.join(format!("docs/critiques/{step}-critique.md"));
            assert!(
                p.exists(),
                "{step}-critique.md should NOT be deleted by reset to DM3a"
            );
        }
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
}
