//! Integration tests for auto.
//!
//! Shared helpers live in `tests/common/mod.rs`.

use sim_flow::client::SessionKind;
use sim_flow::session::host::TestHost;
use sim_flow::session::orchestrator::OrchestratorOptions;
use sim_flow::session::protocol::{Event, HostEvent};
use sim_flow::session::{MockAgent, run_session};
use sim_flow::state::{Flow, State};

mod common;
use common::{foundation_root, hello, init_project, opts};

#[test]
fn auto_mode_ends_when_structural_gate_clean() {
    // Stage a fresh DM0 work session in auto mode. The first LLM
    // response writes a spec.md that satisfies every structural
    // gate check (file_exists, file_matches for clock + node). The
    // orchestrator should emit SessionEnd { completed } without
    // requesting user input — even though the CritiqueClean check
    // is still failing (no critique file yet).
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    let response = "Drafting the spec.\n\n\
        ```docs/spec.md\n\
        # Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n\
        ```\n";
    let mut mock = MockAgent::new();
    mock.enqueue(response);

    let mut o = opts(&project, SessionKind::Work);
    o.auto = true;
    o.max_auto_iters = 3;
    run_session(o, &mut host, &mut mock).unwrap();

    // Spec landed.
    let written = std::fs::read_to_string(project.join("docs/spec.md")).unwrap();
    assert!(written.contains("Clock: 2 GHz"));
    // Session ended cleanly without ever emitting RequestUserInput.
    let saw_request = host
        .written
        .iter()
        .any(|e| matches!(e, Event::RequestUserInput { .. }));
    assert!(
        !saw_request,
        "auto mode should not solicit user input on a clean structural gate",
    );
    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Completed
        ),
        other => panic!("expected SessionEnd completed, got {other:?}"),
    }
}

#[test]
fn auto_mode_caps_iterations_and_drops_to_user_input() {
    // The agent emits an artifact that fails the structural gate
    // (missing the technology-node line). Auto mode feeds the
    // failures back twice, then exceeds max_auto_iters=2 and falls
    // through to RequestUserInput so the higher-level driver can
    // decide whether to hand control to the user.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let bad = "```docs/spec.md\n# Spec\n\nClock: 2 GHz\n```\n";
    let mut host = TestHost::new();
    host.enqueue(hello());
    // Three identical bad responses: original + 2 retries.
    let mut mock = MockAgent::new();
    mock.enqueue(bad);
    mock.enqueue(bad);
    // Once the cap trips, the orchestrator falls through to
    // RequestUserInput; satisfy that with /end-session.
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let mut o = opts(&project, SessionKind::Work);
    o.auto = true;
    o.max_auto_iters = 2;
    run_session(o, &mut host, &mut mock).unwrap();

    // We should have seen exactly one Diagnostic explaining the cap
    // was exceeded.
    let cap_diagnostic = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level, message }
                if matches!(level, sim_flow::session::DiagnosticLevel::Error)
                    && message.contains("max_auto_iters")
        )
    });
    assert!(
        cap_diagnostic,
        "expected a Diagnostic when auto mode hits its iteration cap",
    );
    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Completed
        ),
        other => panic!("expected SessionEnd completed, got {other:?}"),
    }
}

// -------------------------------------------------------------------
// M3: tool dispatch.
// -------------------------------------------------------------------

#[test]
fn tool_call_in_response_is_executed_and_results_feed_back() {
    // Use DM2d.work which advertises read_file. Stage a fake project
    // tree so the read_file tool has something to read.
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM2d");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    std::fs::create_dir_all(project.join("src/model")).unwrap();
    std::fs::write(
        project.join("src/model/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());

    // First LLM turn: agent emits a fenced read_file tool call.
    let mut mock = MockAgent::new();
    mock.enqueue("Let me check the current code.\n\n```tool:read_file\nsrc/model/lib.rs\n```\n");

    // Second LLM turn (after tool result fed back): plain "ok, done".
    mock.enqueue("Got it. Nothing to do.");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let opts = OrchestratorOptions {
        project_dir: project.clone(),
        foundation_root: foundation_root(),
        step_id: "DM2d".into(),
        kind: SessionKind::Work,
        candidate: None,
        llm_backend: "test".into(),
        llm_model: None,
        ..Default::default()
    };
    run_session(opts, &mut host, &mut mock).unwrap();

    // The orchestrator should have executed read_file and emitted a
    // ToolInvoked event for it.
    let invoked = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::ToolInvoked { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        invoked.iter().any(|n| n == "read_file"),
        "expected ToolInvoked(read_file); got: {invoked:?}",
    );

    // The second LLM dispatch should include the tool result as a
    // user message in the messages array.
    let request_payloads = mock.seen.lock().unwrap();
    assert!(
        request_payloads.len() >= 2,
        "expected at least 2 LLM requests"
    );
    let second = &request_payloads[1];
    let saw_tool_result = second.iter().any(|m| {
        m.role == sim_flow::session::protocol::LlmRole::User
            && m.content.contains("Tool results:")
            && m.content.contains("answer()")
    });
    assert!(
        saw_tool_result,
        "expected tool result fed back as a user message; messages were: {second:?}",
    );
}

#[test]
fn near_repeat_streak_injects_loop_guard_hint_into_next_user_message() {
    // Default `max_identical_responses = 3`. Strike-2 (deque len ==
    // cap - 1, all equal) should prepend a one-strike-warning to the
    // next user message the orchestrator builds. Strike-3 still
    // aborts; this hint gives the agent one explicit chance to break
    // the cycle before that.
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM2d");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    std::fs::create_dir_all(project.join("src/model")).unwrap();
    std::fs::write(project.join("src/model/lib.rs"), "pub fn f() {}\n").unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());

    // Identical tool calls for two turns in a row. The orchestrator
    // dispatches read_file each time and feeds the (same) result back
    // as a user message. After turn 2 the strike-2 condition fires
    // and the next user message we build (the tool result for turn 2)
    // gets the hint prefix.
    let identical_call = "Re-reading the file.\n\n```tool:read_file\nsrc/model/lib.rs\n```\n";
    let mut mock = MockAgent::new();
    mock.enqueue(identical_call);
    mock.enqueue(identical_call);

    // Turn 3: emit something different so the streak breaks and we
    // don't trip the strike-3 abort. The /end-session pumps the
    // session to completion so we can inspect the recorded events.
    mock.enqueue("Done with the lookup. /end-session");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let opts = OrchestratorOptions {
        project_dir: project.clone(),
        foundation_root: foundation_root(),
        step_id: "DM2d".into(),
        kind: SessionKind::Work,
        candidate: None,
        llm_backend: "test".into(),
        llm_model: None,
        ..Default::default()
    };
    run_session(opts, &mut host, &mut mock).unwrap();

    // No abort should have fired (strike-3 didn't happen).
    let saw_runaway = host.written.iter().any(|e| {
        matches!(
            e,
            Event::SessionEnd { reason, .. } if *reason == sim_flow::session::protocol::SessionEndReason::RunawayGuard
        )
    });
    assert!(
        !saw_runaway,
        "the streak was broken by turn 3; runaway-guard should NOT have fired",
    );

    // The third LLM dispatch's messages should include a User
    // message whose content begins with the loop-guard hint prefix
    // (the tool result for turn 2 was the message that got the
    // injection).
    let request_payloads = mock.seen.lock().unwrap();
    assert!(
        request_payloads.len() >= 3,
        "expected at least 3 LLM requests (turns lr-1/2/3); got {}",
        request_payloads.len()
    );
    let third = &request_payloads[2];
    let saw_hint = third.iter().any(|m| {
        matches!(m.role, sim_flow::session::protocol::LlmRole::User)
            && m.content.contains("Loop guard warning")
    });
    assert!(
        saw_hint,
        "expected loop-guard hint in the tool-result user message ahead of turn 3; \
         messages were: {third:?}",
    );

    // The SECOND request (turn 2) should NOT have the hint — at that
    // point the deque only had one entry from turn 1.
    let second = &request_payloads[1];
    let second_has_hint = second.iter().any(|m| {
        matches!(m.role, sim_flow::session::protocol::LlmRole::User)
            && m.content.contains("Loop guard warning")
    });
    assert!(
        !second_has_hint,
        "hint should not appear before strike-2 detection; turn-2 messages were: {second:?}",
    );
}

// -------------------------------------------------------------------
// M4: TerminalHost.
// -------------------------------------------------------------------
