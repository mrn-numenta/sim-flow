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

use crate::Result;
use crate::session::host::Host;
use crate::session::orchestrator::{
    OrchestratorOptions, run_session, step_descriptor_for_protocol,
};
use crate::session::protocol::{
    DiagnosticLevel, Event, GateFailureOut, HostEvent, HostInfo, PROTOCOL_VERSION, SessionKindOut,
    SessionTag, StepMode,
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
            StepMode::Auto => match run_auto_loop(&opts, &mut auto_host)? {
                AutoLoopOutcome::Completed => break RunOutcome::Completed,
                AutoLoopOutcome::FlippedToManual => continue,
                AutoLoopOutcome::Shutdown => break RunOutcome::Shutdown,
            },
            StepMode::Manual => match wait_for_command(&opts, &mut auto_host)? {
                ManualOutcome::Continue => continue,
                ManualOutcome::Shutdown => break RunOutcome::Shutdown,
                ManualOutcome::HostClosed => break RunOutcome::HostClosed,
            },
        }
    };

    // Final SessionEnd. AutoHost forwards this to the underlying host
    // (consume_session_end is reset to false here so the user sees a
    // clean end-of-auto-run banner).
    auto_host.consume_session_end = false;
    let (reason, message) = match outcome {
        RunOutcome::Completed => ("completed", Some("auto run finished".into())),
        RunOutcome::Shutdown => ("completed", Some("orchestrator shut down".into())),
        RunOutcome::HostClosed => ("completed", Some("host disconnected".into())),
    };
    auto_host.write(&Event::SessionEnd {
        reason: reason.into(),
        message,
    })?;
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
            if blockers.is_empty() {
                break;
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

        // Try to advance. If gate clean, mark passed + bump
        // current_step. If not clean, flip to manual so the user can
        // inspect and decide.
        let advanced = try_advance(&opts.project_dir, step_id, auto_host)?;
        if !advanced {
            flip_to_manual(auto_host)?;
            return Ok(AutoLoopOutcome::FlippedToManual);
        }
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

fn flip_to_manual<H: Host>(auto_host: &mut AutoHost<H>) -> Result<()> {
    let prev = auto_host.current_step_mode();
    auto_host.store_step_mode(StepMode::Manual);
    if !matches!(prev, StepMode::Manual) {
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
                reason: "protocol-error".into(),
                message: Some(format!("expected Hello first, got {other:?}")),
            })?;
            return Err(crate::Error::State(format!(
                "expected Hello first, got {other:?}"
            )));
        }
        None => {
            return Err(crate::Error::State(
                "session: host closed before Hello".into(),
            ));
        }
    };
    if hello_version != PROTOCOL_VERSION {
        auto_host.write(&Event::SessionEnd {
            reason: "protocol-mismatch".into(),
            message: Some(format!(
                "host sent protocolVersion={hello_version}; orchestrator speaks {PROTOCOL_VERSION}"
            )),
        })?;
        return Err(crate::Error::State(format!(
            "protocol version mismatch: host={hello_version} orchestrator={PROTOCOL_VERSION}"
        )));
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

    // Step 1: delete generated collateral for `step_id` and every
    // downstream step. Source spec, conversation transcript, git
    // history, and `.sim-flow/` are not touched. Files / dirs that
    // don't exist are silently skipped; deletion failures are
    // collected and reported alongside the success summary.
    let mut deleted: Vec<PathBuf> = Vec::new();
    let mut delete_failures: Vec<(PathBuf, String)> = Vec::new();
    for downstream in &order[idx..] {
        let Some(step) = registry.get(downstream) else {
            continue;
        };
        for art in step.work_artifacts {
            collect_collateral_deletion(&opts.project_dir, art, &mut deleted, &mut delete_failures);
        }
        let critique_rel = format!("docs/critiques/{}-critique.md", step.id);
        collect_collateral_deletion(
            &opts.project_dir,
            &critique_rel,
            &mut deleted,
            &mut delete_failures,
        );
    }

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

/// Best-effort delete of one collateral path under `project_dir`. A
/// trailing `/` (or an actual directory on disk) triggers
/// `remove_dir_all`; anything else is treated as a single file.
/// Missing paths are silently skipped — the registry's
/// `work_artifacts` lists every output the step *might* produce, but
/// optional ones (per-candidate dirs, conditional outputs) won't
/// always be present.
fn collect_collateral_deletion(
    project_dir: &Path,
    rel: &str,
    deleted: &mut Vec<PathBuf>,
    failures: &mut Vec<(PathBuf, String)>,
) {
    let path = project_dir.join(rel);
    if !path.exists() {
        return;
    }
    let is_dir = rel.ends_with('/') || path.is_dir();
    let result = if is_dir {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };
    match result {
        Ok(()) => deleted.push(path),
        Err(err) => failures.push((path, err.to_string())),
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
fn read_blockers(project_dir: &Path, step_id: &str) -> Vec<String> {
    let path = project_dir
        .join("docs/critiques")
        .join(format!("{step_id}-critique.md"));
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let mut blockers = Vec::new();
    for line in body.lines() {
        let stripped = strip_finding_prefix(line);
        if let Some(text) = stripped.strip_prefix("BLOCKER:") {
            blockers.push(format!("BLOCKER: {}", text.trim()));
        }
    }
    blockers
}

/// Mirror of the extension's parser: tolerate leading whitespace,
/// list markers (`- `, `* `, `12. `), and bold-wrap (`**...**`).
fn strip_finding_prefix(raw: &str) -> String {
    let mut s = raw.trim_start().to_string();
    if let Some(rest) = strip_list_marker(&s) {
        s = rest;
    }
    if let Some(rest) = s.strip_prefix("**") {
        s = rest.to_string();
        if let Some(close) = s.rfind("**") {
            let mut tmp = String::with_capacity(s.len());
            tmp.push_str(&s[..close]);
            tmp.push_str(&s[close + 2..]);
            s = tmp;
        }
    }
    s
}

fn strip_list_marker(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("- ") {
        return Some(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("* ") {
        return Some(rest.to_string());
    }
    let mut last_digit_end = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            last_digit_end = Some(i + c.len_utf8());
        } else {
            break;
        }
    }
    if let Some(end) = last_digit_end
        && let Some(rest) = s[end..].strip_prefix(". ")
    {
        return Some(rest.to_string());
    }
    None
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
                        self.inner.write(&Event::StepModeChanged { mode })?;
                    }
                    continue;
                }
                Some(HostEvent::Shutdown) => {
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
                        self.inner.write(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "ignored {label}: auto mode owns step execution; toggle to manual first."
                            ),
                        })?;
                        continue;
                    }
                    if self.in_subsession {
                        self.inner.write(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "ignored {label}: a sub-session is currently running; retry after it finishes."
                            ),
                        })?;
                        continue;
                    }
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
    fn strip_finding_prefix_handles_numbered_bold_markdown() {
        // Mirrors the extension's parser regression test in
        // critiques.test.ts ("handles numbered markdown lists with
        // bold-wrapped headings"). Keeps the two parsers in sync.
        let cases = [
            ("- BLOCKER: foo", "BLOCKER: foo"),
            ("* BLOCKER: foo", "BLOCKER: foo"),
            ("  - UNRESOLVED: bar", "UNRESOLVED: bar"),
            (
                "1. **BLOCKER: testbench file missing entirely.**",
                "BLOCKER: testbench file missing entirely.",
            ),
            ("12. **UNRESOLVED: ...**", "UNRESOLVED: ..."),
            ("BLOCKER: raw", "BLOCKER: raw"),
            ("not a finding", "not a finding"),
        ];
        for (input, expected) in cases {
            assert_eq!(strip_finding_prefix(input), expected, "input={input}");
        }
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
            reason: "completed".into(),
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
            reason: "completed".into(),
            message: None,
        })
        .unwrap();
        let saw = inner
            .written
            .iter()
            .any(|e| matches!(e, Event::SessionEnd { .. }));
        assert!(saw, "manual-mode SessionEnd should reach the host");
    }
}
