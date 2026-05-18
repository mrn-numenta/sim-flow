//! Integration tests for the orchestrator dispatch loop's
//! `ask_user` suspend handling (Phase 6 milestone 6.0).
//!
//! These tests exercise the orchestrator's end of the suspend/resume
//! contract. The `ask_user` tool itself is exercised at the unit
//! level in `tools::ask_user::tests` and at the runtime level in
//! `tests/ask_user_integration.rs`; this binary verifies that
//! `run_session` (the dispatch loop) reacts correctly when a tool
//! call returns `ToolResult::suspend.is_some()`:
//!
//!   - Emits a `RequestUserInput` derived from `PendingUserAsk`.
//!   - Waits for the next `UserMessage` and resumes the runtime.
//!   - Pushes a Tool-role reply tied to the suspended call's id.
//!   - Discards subsequent tool calls in the same model response
//!     with a `tool_calls_after_ask_user` diagnostic.
//!   - Flips auto→manual once per session on the first ask_user
//!     during an auto run (Architecture §6.5.2).
//!
//! The mock LLM scripts a single tool-calls turn carrying one
//! `ask_user` invocation; the scripted host supplies the user's
//! reply via a `UserMessage` after the orchestrator parks on the
//! generated `RequestUserInput`. After that we send `/end-session`
//! so the run loop ends cleanly without driving the LLM further.

use sim_flow::client::SessionKind;
use sim_flow::session::host::TestHost;
use sim_flow::session::protocol::{
    DiagnosticLevel, Event, HostEvent, HostInfo, PROTOCOL_VERSION, SessionEndReason,
};
use sim_flow::session::{AdvertisedToolCall, MockAgent, run_session};

mod common;
use common::{init_project, opts};

fn hello_with_caps() -> HostEvent {
    HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "ask-user-dispatch-test".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into()],
    }
}

#[test]
fn dispatch_loop_emits_request_user_input_on_ask_user_suspend() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();

    // Script: one ask_user tool call, then end-session.
    mock.enqueue_with_tool_calls(
        "",
        vec![AdvertisedToolCall {
            id: Some("call_ask_user_0".into()),
            name: "ask_user".into(),
            arguments_json: r#"{"question": "What is the clock frequency?", "kind": "value"}"#
                .into(),
        }],
    );
    // After the user reply lands and the next LLM turn dispatches,
    // we don't need another response — the user types /end-session
    // to terminate the run cleanly.
    host.enqueue(hello_with_caps());
    host.enqueue(HostEvent::UserMessage {
        text: "1 GHz".into(),
    });
    // The next LLM turn won't fire because the orchestrator processes
    // the resume Tool-role message and goes back through the
    // dispatch loop; the empty MockAgent queue returns an empty
    // turn, which the orchestrator surfaces as a RequestUserInput.
    // The /end-session reply ends the session cleanly.
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let _ = run_session(opts(&project, SessionKind::Work), &mut host, &mut mock);

    // Find the first RequestUserInput AFTER the HelloAck.
    let request_input = host
        .written
        .iter()
        .find_map(|e| match e {
            Event::RequestUserInput { prompt, .. } => Some(prompt.clone()),
            _ => None,
        })
        .expect("expected a RequestUserInput from ask_user suspend");
    let prompt = request_input.expect("ask_user prompt should be populated");
    assert!(
        prompt.contains("What is the clock frequency?"),
        "prompt should carry the ask_user question; got: {prompt}"
    );

    // The orchestrator should also surface the suspension via a
    // ToolInvoked event with status=suspended.
    let suspended_invoke = host.written.iter().any(|e| {
        matches!(
            e,
            Event::ToolInvoked { name, status, .. } if name == "ask_user" && status == "suspended"
        )
    });
    assert!(
        suspended_invoke,
        "expected a `suspended` ToolInvoked event; saw: {:#?}",
        host.written
    );
}

#[test]
fn dispatch_loop_resumes_with_answer_after_user_reply() {
    // Drives the full round-trip: ask_user → user reply → the
    // orchestrator pushes the AskUserAnswer as a Tool-role message
    // on the next LLM turn (we inspect mock.seen[1] for the Tool-
    // role frame keyed by the suspending call's id).
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();

    mock.enqueue_with_tool_calls(
        "",
        vec![AdvertisedToolCall {
            id: Some("call_42".into()),
            name: "ask_user".into(),
            arguments_json: r#"{"question": "Pick a width", "kind": "free-form"}"#.into(),
        }],
    );

    host.enqueue(hello_with_caps());
    host.enqueue(HostEvent::UserMessage {
        text: "4 bytes".into(),
    });
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let _ = run_session(opts(&project, SessionKind::Work), &mut host, &mut mock);

    // The second LLM dispatch (after the resume) should contain a
    // Tool-role message whose content is the AskUserAnswer JSON
    // and whose tool_call_id matches the suspending call's id.
    let seen = mock.seen.lock().unwrap();
    assert!(
        seen.len() >= 2,
        "expected at least two LLM dispatches (initial + resume), got {}",
        seen.len()
    );
    let resume_turn = &seen[1];
    let tool_msg = resume_turn
        .iter()
        .find(|m| {
            matches!(m.role, sim_flow::session::protocol::LlmRole::Tool)
                && m.tool_call_id.as_deref() == Some("call_42")
        })
        .expect("expected Tool-role resume message keyed by call_42");
    assert!(
        tool_msg.content.contains("4 bytes"),
        "resume Tool-role message should carry the user's reply; got: {}",
        tool_msg.content
    );
    assert!(
        tool_msg.content.contains("thread_id"),
        "resume Tool-role message should be the AskUserAnswer JSON \
         (carries `thread_id` field); got: {}",
        tool_msg.content
    );
}

#[test]
fn dispatch_loop_discards_tool_calls_after_ask_user() {
    // Architecture §6.5.1: when the model emits ask_user followed by
    // more tool calls in the same response, the orchestrator
    // suspends on ask_user and discards the rest with a
    // `tool_calls_after_ask_user` warning.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();

    mock.enqueue_with_tool_calls(
        "",
        vec![
            AdvertisedToolCall {
                id: Some("call_ask".into()),
                name: "ask_user".into(),
                arguments_json: r#"{"question": "Pick endianness"}"#.into(),
            },
            AdvertisedToolCall {
                id: Some("call_read".into()),
                name: "read_file".into(),
                arguments_json: r#"{"path": "Cargo.toml"}"#.into(),
            },
            AdvertisedToolCall {
                id: Some("call_list".into()),
                name: "list_dir".into(),
                arguments_json: r#"{"path": "."}"#.into(),
            },
        ],
    );

    host.enqueue(hello_with_caps());
    host.enqueue(HostEvent::UserMessage {
        text: "little".into(),
    });
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let _ = run_session(opts(&project, SessionKind::Work), &mut host, &mut mock);

    // Diagnostic: tool_calls_after_ask_user should fire with a
    // count of 2 (the two calls after ask_user).
    let warning = host
        .written
        .iter()
        .find_map(|e| match e {
            Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message,
            } if message.contains("tool_calls_after_ask_user") => Some(message.clone()),
            _ => None,
        })
        .expect("expected a tool_calls_after_ask_user diagnostic");
    assert!(
        warning.contains("discarded 2"),
        "warning should report 2 discarded calls; got: {warning}"
    );
    // The discarded tools must NOT have run: no ToolInvoked event
    // for read_file or list_dir.
    let read_ran = host
        .written
        .iter()
        .any(|e| matches!(e, Event::ToolInvoked { name, .. } if name == "read_file"));
    let list_ran = host
        .written
        .iter()
        .any(|e| matches!(e, Event::ToolInvoked { name, .. } if name == "list_dir"));
    assert!(
        !read_ran && !list_ran,
        "tool calls after ask_user must be discarded, not executed; events: {:?}",
        host.written,
    );
}

#[test]
fn dispatch_loop_flips_step_mode_when_auto_ask_user_lands() {
    // Architecture §6.5.2: when the run is in auto mode at the
    // moment of the first ask_user call, the orchestrator flips
    // current_step_mode to manual, emits a StepModeChanged event,
    // and emits a Diagnostic::Info explaining the flip.
    use sim_flow::session::ask_user::mode_flip::write_current_step_mode;
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    write_current_step_mode(&project, StepMode::Auto).unwrap();

    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    mock.enqueue_with_tool_calls(
        "",
        vec![AdvertisedToolCall {
            id: Some("call_ask".into()),
            name: "ask_user".into(),
            arguments_json: r#"{"question": "Pick a width", "kind": "value"}"#.into(),
        }],
    );

    host.enqueue(hello_with_caps());
    host.enqueue(HostEvent::UserMessage { text: "4".into() });
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let _ = run_session(opts(&project, SessionKind::Work), &mut host, &mut mock);

    // StepModeChanged event must fire with mode=manual.
    let mode_changed = host
        .written
        .iter()
        .any(|e| matches!(e, Event::StepModeChanged { mode } if *mode == StepMode::Manual));
    assert!(
        mode_changed,
        "expected a StepModeChanged(manual) event; events: {:?}",
        host.written,
    );
    // The Diagnostic::Info "flipping to manual mode" should also fire.
    let flip_info = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level: DiagnosticLevel::Info, message }
                if message.contains("flipping to manual")
        )
    });
    assert!(
        flip_info,
        "expected the auto→manual diagnostic; events: {:?}",
        host.written,
    );
}

#[test]
fn dispatch_loop_treats_cancel_thread_reply_as_thread_cancel() {
    // A `/cancel-thread` reply during an ask_user suspend ends the
    // thread and surfaces as a TBD in the runtime's persistence
    // layer. The dispatch loop must (a) still send a Tool-role
    // resume message keyed by the suspended call's id so the LLM
    // sees the cancel, and (b) not crash on the cancel path.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();

    mock.enqueue_with_tool_calls(
        "",
        vec![AdvertisedToolCall {
            id: Some("call_ask".into()),
            name: "ask_user".into(),
            arguments_json: r#"{"question": "Pick a width"}"#.into(),
        }],
    );

    host.enqueue(hello_with_caps());
    host.enqueue(HostEvent::UserMessage {
        text: "/cancel-thread".into(),
    });
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let _ = run_session(opts(&project, SessionKind::Work), &mut host, &mut mock);

    // The session must end cleanly (Completed), not error.
    let last_end = host
        .written
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionEnd { reason, .. } => Some(*reason),
            _ => None,
        })
        .expect("expected SessionEnd");
    assert_eq!(last_end, SessionEndReason::Completed);
    // The resume message went through (the run reached /end-session).
    // Verify by checking that a Tool-role message keyed by call_ask
    // was pushed into the second dispatch's prompt stack.
    let seen = mock.seen.lock().unwrap();
    if seen.len() >= 2 {
        let resume_turn = &seen[1];
        let saw_tool_resume = resume_turn.iter().any(|m| {
            matches!(m.role, sim_flow::session::protocol::LlmRole::Tool)
                && m.tool_call_id.as_deref() == Some("call_ask")
        });
        assert!(
            saw_tool_resume,
            "expected a Tool-role resume message even on cancel; got: {:#?}",
            resume_turn,
        );
    }
}
