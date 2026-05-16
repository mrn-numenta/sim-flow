//! Integration tests for manual.
//!
//! Shared helpers live in `tests/common/mod.rs`.

use sim_flow::session::MockAgent;
use sim_flow::session::host::TestHost;
use sim_flow::session::protocol::{Event, HostEvent};

mod common;
use common::{auto_opts, hello, init_project};

#[test]
fn manual_mode_performs_hello_handshake_at_startup() {
    // Regression test: before the handshake-at-run_auto-entry fix,
    // manual mode parked in `wait_for_command` immediately after
    // emitting StepModeChanged, then read the host's Hello and
    // rejected it as "unexpected." That left the dashboard's pump
    // attached but the orchestrator unable to receive any manual-
    // mode command. With the fix, run_auto consumes Hello once at
    // the top and emits HelloAck before the parking loop.
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    // First event written must be HelloAck — before StepModeChanged
    // and before any sub-session activity. The dashboard's pump
    // renders its banner from this HelloAck, so emitting it before
    // anything else also avoids the "no banner" symptom.
    match &host.written[0] {
        Event::HelloAck {
            session,
            step_descriptor,
            ..
        } => {
            assert_eq!(session.step, "DM0");
            assert!(
                step_descriptor
                    .work_artifacts
                    .iter()
                    .any(|p| p == "docs/spec.md")
            );
        }
        other => panic!("expected HelloAck first, got {other:?}"),
    }
    // No "ignored unexpected host event: Hello" diagnostic.
    let saw_unexpected = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { message, .. } if message.contains("unexpected host event: Hello")
        )
    });
    assert!(
        !saw_unexpected,
        "manual mode at startup must not reject Hello as unexpected; events: {:?}",
        host.written,
    );
}

#[test]
fn manual_mode_starts_parked_and_emits_initial_step_mode_changed() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    // The orchestrator reads Hello once on start, sends HelloAck,
    // then enters the parking loop. With no further commands the
    // parking loop reads None and exits via HostClosed.
    host.enqueue(hello());

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    let saw_mode_change = host
        .written
        .iter()
        .any(|e| matches!(e, Event::StepModeChanged { mode } if matches!(mode, StepMode::Manual)));
    assert!(
        saw_mode_change,
        "manual mode should echo StepModeChanged on start so the dashboard toggle aligns",
    );
    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Completed
        ),
        other => panic!("expected SessionEnd, got {other:?}"),
    }
    // Manual-mode parking should never run a sub-session without a
    // command, so the orchestrator should never have dispatched.
    assert!(
        mock.seen.lock().unwrap().is_empty(),
        "no LLM dispatch should fire in a parked manual run"
    );
}

#[test]
fn manual_mode_dispatches_run_gate_and_keeps_parking() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::RunGate { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    // RunGate evaluates the gate and emits a GateResult. DM0's
    // structural gate looks for docs/spec.md — which doesn't exist in
    // a freshly-initialized project — so the report is unclean.
    let saw_gate = host
        .written
        .iter()
        .any(|e| matches!(e, Event::GateResult { step, clean, .. } if step == "DM0" && !*clean));
    assert!(
        saw_gate,
        "RunGate should emit a GateResult; events: {:?}",
        host.written
    );
    // No LLM dispatch should happen for RunGate.
    assert!(
        mock.seen.lock().unwrap().is_empty(),
        "RunGate should not dispatch an LLM call"
    );
}

#[test]
fn manual_mode_dispatches_run_step_and_runs_a_real_subsession() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::RunStep {
        step: "DM0".into(),
        kind: sim_flow::session::protocol::SessionKindOut::Work,
    });
    // The work sub-session needs an LLM response. A clean spec.md
    // satisfies the structural gate so the orchestrator ends with
    // SessionEnd { completed }.
    let response = "Drafting the spec.\n\n\
        ```docs/spec.md\n\
        # Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n\
        ```\n";
    mock.enqueue(response);
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    // Spec landed.
    let written = std::fs::read_to_string(project.join("docs/spec.md")).unwrap();
    assert!(written.contains("Clock: 2 GHz"));
    // Manual-mode sub-sessions swallow their own SessionEnd so the
    // host (extension) doesn't treat sub-session completion as
    // "the orchestrator process is gone" and clear its activeSession
    // reference. Only one final SessionEnd fires when run_auto
    // returns on Shutdown.
    let session_ends: Vec<_> = host
        .written
        .iter()
        .filter(|e| matches!(e, Event::SessionEnd { .. }))
        .collect();
    assert_eq!(
        session_ends.len(),
        1,
        "expected exactly one final SessionEnd from run_auto on Shutdown; got {} ({:?})",
        session_ends.len(),
        host.written,
    );
    // The dispatched sub-session must be bracketed by
    // SubSessionStarted / SubSessionEnded so the dashboard can
    // disable per-step buttons during the busy span. Started
    // appears BEFORE the inner run_session emits its events
    // (HelloAck for the synthetic Hello, PhaseChanged, etc.) and
    // Ended appears AFTER them.
    let started_idx = host
        .written
        .iter()
        .position(|e| matches!(e, Event::SubSessionStarted { .. }))
        .expect("expected SubSessionStarted");
    let ended_idx = host
        .written
        .iter()
        .position(|e| matches!(e, Event::SubSessionEnded { .. }))
        .expect("expected SubSessionEnded");
    assert!(
        started_idx < ended_idx,
        "SubSessionStarted must precede SubSessionEnded (got {started_idx} vs {ended_idx})",
    );
    let phase_idx = host
        .written
        .iter()
        .position(|e| matches!(e, Event::PhaseChanged { .. }))
        .expect("expected PhaseChanged from inner run_session");
    assert!(
        started_idx < phase_idx && phase_idx < ended_idx,
        "PhaseChanged should fall inside the SubSession bracket",
    );
    // Verify the bracket carries the right step + kind.
    if let Event::SubSessionStarted { step, kind } = &host.written[started_idx] {
        assert_eq!(step, "DM0");
        assert!(matches!(
            kind,
            sim_flow::session::protocol::SessionKindOut::Work
        ));
    }
    if let Event::SubSessionEnded {
        step,
        kind,
        outcome,
    } = &host.written[ended_idx]
    {
        assert_eq!(step, "DM0");
        assert!(matches!(
            kind,
            sim_flow::session::protocol::SessionKindOut::Work
        ));
        assert_eq!(outcome, "completed");
    }
}

#[test]
fn manual_mode_reset_deletes_generated_collateral_for_step_and_downstream() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    // Stage some collateral to delete: DM0's spec.md and a critique
    // file. Also stage a file under DM1a's expected work-artifact
    // path so we can verify downstream-step deletion.
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(project.join("docs/spec.md"), "# spec\nClock: 2 GHz\n").unwrap();
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    std::fs::write(
        project.join("docs/critiques/DM0-critique.md"),
        "# DM0 Critique\n",
    )
    .unwrap();
    // Pre-mark DM0 passed so we can verify the gate flag clears.
    {
        let dot = project.join(".sim-flow");
        let mut state = sim_flow::state::State::load(&dot).unwrap();
        state.mark_passed("DM0", "1");
        state.save(&dot).unwrap();
        assert!(state.is_passed("DM0"));
    }

    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::Reset { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    // Files removed.
    assert!(
        !project.join("docs/spec.md").exists(),
        "Reset should delete docs/spec.md",
    );
    assert!(
        !project.join("docs/critiques/DM0-critique.md").exists(),
        "Reset should delete docs/critiques/DM0-critique.md",
    );
    // State.toml gate flag cleared.
    let dot = project.join(".sim-flow");
    let state = sim_flow::state::State::load(&dot).unwrap();
    assert!(
        !state.is_passed("DM0"),
        "Reset should clear DM0's gate flag",
    );
    assert_eq!(state.current_step, "DM0");
    // Diagnostic emitted summarizing the deletion.
    let saw_summary = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level, message }
                if matches!(level, sim_flow::session::DiagnosticLevel::Info)
                    && message.contains("Reset to `DM0`")
                    && message.contains("docs/spec.md")
        )
    });
    assert!(
        saw_summary,
        "Reset should emit an Info diagnostic listing deleted files; got {:?}",
        host.written,
    );
}

#[test]
fn manual_mode_reset_handles_missing_collateral_gracefully() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    // Fresh project: no spec.md, no critique. Reset should still
    // succeed; the diagnostic summary says "no generated collateral
    // found to delete."
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::Reset { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    let saw_summary = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level, message }
                if matches!(level, sim_flow::session::DiagnosticLevel::Info)
                    && message.contains("Reset to `DM0`")
                    && message.contains("no generated collateral")
        )
    });
    assert!(
        saw_summary,
        "Reset on a clean project should emit an Info diagnostic noting nothing was deleted; got {:?}",
        host.written,
    );
}

#[test]
fn manual_mode_set_step_mode_to_auto_resumes_iteration() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    // SetStepMode flips the flag (intercepted by AutoHost) and emits
    // StepModeChanged. The auto loop then takes over and tries to
    // run a DM0 work sub-session. We don't enqueue an LLM response,
    // so the orchestrator's read returns None and the run terminates
    // — but only AFTER we observe StepModeChanged { auto }.
    host.enqueue(HostEvent::SetStepMode {
        mode: StepMode::Auto,
    });

    let _ =
        sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock);

    let saw_to_auto = host
        .written
        .iter()
        .any(|e| matches!(e, Event::StepModeChanged { mode } if matches!(mode, StepMode::Auto)));
    assert!(
        saw_to_auto,
        "SetStepMode {{ auto }} should emit StepModeChanged; events: {:?}",
        host.written
    );
}
