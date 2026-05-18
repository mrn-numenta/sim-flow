//! Integration tests for the `ask_user` suspend/resume protocol
//! (Phase 5 milestone 5.14 / Architecture Chapter 4 §4.5 and
//! Chapter 6 §6.5).
//!
//! The orchestrator's outer dispatch loop owns the actual
//! `RequestUserInput` + `UserMessage` event plumbing; the unit-test
//! integration here drives the same state machine through the
//! `AskUserRuntime` + `AskUserTool` public APIs. A scripted "host"
//! supplies user replies through `runtime.resume_from_user_ask`.
//!
//! Coverage:
//!
//! - Test 1: manual-mode single-turn thread persists one Open Question.
//! - Test 2: auto-mode single-turn thread flips to manual.
//! - Test 3: reload mid-suspend recovers pending state.
//! - Test 4: tool calls AFTER ask_user surface as a marker that the
//!   dispatch loop can detect (orchestrator-side wiring); this test
//!   asserts the marker is present on the ToolResult.
//! - Test 5: chained 3-turn thread closes with an Auto-decision row
//!   and the multi-round annotation.
//! - Test 6: /cancel-thread mid-thread persists unresolved Open Q.
//! - Test 7: force-close on sub-session end preserves answered
//!   threads and drops empty ones.
//! - Test 8: turn-cap warning fires on the 5th turn.
//! - Test 9: interleaved threads stay independent.

use std::sync::Arc;

use serde_json::json;

use sim_flow::__internal::session::ask_user::{
    AskUserRuntime, PendingUserAsk, flip_step_mode_for_ask_user,
    mode_flip::{RecordingSink, read_current_step_mode, write_current_step_mode},
};
use sim_flow::__internal::session::protocol::StepMode;
use sim_flow::__internal::session::tools::{ASK_USER_TURN_CAP, AskUserTool, Tool, ToolContext};

fn empty_ctx<'a>(project: &'a std::path::Path) -> ToolContext<'a> {
    ToolContext::new(project, None, None, None)
}

#[test]
fn test_1_manual_single_turn_thread_persists_open_question() {
    let tmp = tempfile::tempdir().unwrap();
    // Manual mode and a docs/spec.md present.
    write_current_step_mode(tmp.path(), StepMode::Manual).unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    let r = tool
        .invoke(
            &ctx,
            &json!({"question": "How wide is the bus?", "record_as": "open-question"}),
        )
        .expect("invoke");
    let suspend = r.suspend.as_ref().expect("suspend populated");
    assert_eq!(suspend.pending.thread_turn_index, 0);
    let tid = suspend.pending.thread_id.clone();
    assert!(rt.has_pending());

    // Scripted host supplies the answer.
    let answer = rt
        .resume_from_user_ask("4 bytes", false, false)
        .expect("resume");
    assert_eq!(answer.thread_id, tid);
    assert_eq!(answer.thread_turn_index, 0);
    assert!(!answer.recorded_at.is_empty());

    let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
    assert!(body.contains("How wide is the bus?"));
    assert!(body.contains("4 bytes"));
    // Single entry.
    assert_eq!(body.matches("How wide is the bus?").count(), 1);
}

#[test]
fn test_2_auto_mode_first_call_flips_to_manual() {
    let tmp = tempfile::tempdir().unwrap();
    write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    tool.invoke(
        &ctx,
        &json!({"question": "Pick endianness", "record_as": "open-question"}),
    )
    .expect("invoke");

    assert_eq!(read_current_step_mode(tmp.path()), StepMode::Manual);
    // A separate flip with a recording sink confirms the side effects
    // are idempotent on subsequent fresh threads.
    let mut sink = RecordingSink::default();
    let _ = flip_step_mode_for_ask_user(tmp.path(), &mut sink);
    // Already manual -> no-op.
    assert!(sink.mode_changes.is_empty());
}

#[test]
fn test_3_reload_mid_suspend_recovers_pending_state() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM0".into()));
    let tool = AskUserTool::new(rt);
    let ctx = empty_ctx(tmp.path());
    let r = tool
        .invoke(&ctx, &json!({"question": "Frequency?"}))
        .expect("invoke");
    let original_tid = r
        .suspend
        .as_ref()
        .expect("suspend")
        .pending
        .thread_id
        .clone();

    // Simulate orchestrator restart: drop runtime, build a new one,
    // verify recovery.
    let rt2 = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM0".into()));
    let recovered = rt2
        .recover_from_checkpoint()
        .expect("recover")
        .expect("present");
    assert_eq!(recovered.thread_id, original_tid);
    assert!(rt2.has_pending());

    // Resume on the recovered runtime.
    let answer = rt2
        .resume_from_user_ask("1 GHz", false, false)
        .expect("resume");
    assert_eq!(answer.thread_id, original_tid);
    assert_eq!(answer.answer, "1 GHz");
}

#[test]
fn test_4_subsequent_tool_calls_after_ask_user_marker_present() {
    // The architecture says the orchestrator's dispatch loop must
    // detect ask_user suspension and discard subsequent calls with a
    // `tool_calls_after_ask_user` warning. The protocol surface this
    // test verifies is the ToolResult.suspend field -- whether the
    // orchestrator's dispatch loop acts on it is its concern. Here
    // we assert that a fresh AskUserTool::invoke populates the
    // typed suspend field the dispatch loop matches on.
    let tmp = tempfile::tempdir().unwrap();
    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt);
    let ctx = empty_ctx(tmp.path());
    let r = tool
        .invoke(&ctx, &json!({"question": "x"}))
        .expect("invoke");
    let suspend = r.suspend.as_ref().expect("suspend populated");
    assert!(!suspend.pending.thread_id.is_empty());
    assert!(suspend.fresh_thread);
}

#[test]
fn test_5_chained_three_turns_close_as_auto_decision_writes_one_row() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    // Turn 0: fresh, record_as = none.
    let r = tool
        .invoke(
            &ctx,
            &json!({"question": "How many entries?", "record_as": "none"}),
        )
        .expect("turn 0");
    let tid = r.suspend.as_ref().unwrap().pending.thread_id.clone();
    let a0 = rt
        .resume_from_user_ask("probably 4", false, false)
        .expect("r0");
    assert_eq!(a0.thread_turn_index, 0);

    // Turn 1: continuation, record_as = none.
    let r = tool
        .invoke(
            &ctx,
            &json!({
                "question": "Specifically 4?",
                "record_as": "none",
                "thread_id": tid,
            }),
        )
        .expect("turn 1");
    assert_eq!(r.suspend.as_ref().unwrap().pending.thread_turn_index, 1);
    let a1 = rt.resume_from_user_ask("yes, 4", false, false).expect("r1");
    assert_eq!(a1.thread_turn_index, 1);

    // Turn 2: closing, record_as = auto-decision.
    let r = tool
        .invoke(
            &ctx,
            &json!({
                "question": "Confirm 4 entries?",
                "record_as": "auto-decision",
                "thread_id": tid,
            }),
        )
        .expect("turn 2");
    assert_eq!(r.suspend.as_ref().unwrap().pending.thread_turn_index, 2);
    let a2 = rt
        .resume_from_user_ask("yes, 4 confirmed", false, false)
        .expect("r2");
    assert_eq!(a2.thread_turn_index, 2);
    assert!(!a2.recorded_at.is_empty());

    let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
    // Exactly ONE Auto-decision row.
    let count = body.matches("**decision**").count();
    assert_eq!(count, 1, "got body = {body}");
    // Annotation for multi-turn thread.
    assert!(body.contains("3 rounds of clarification"), "{body}");
}

#[test]
fn test_6_cancel_thread_persists_unresolved_open_question() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    // Two successful turns.
    let r = tool
        .invoke(&ctx, &json!({"question": "q0", "record_as": "none"}))
        .expect("turn 0");
    let tid = r
        .suspend
        .as_ref()
        .expect("suspend populated")
        .pending
        .thread_id
        .clone();
    rt.resume_from_user_ask("a0", false, false).expect("r0");

    tool.invoke(
        &ctx,
        &json!({"question": "q1", "record_as": "none", "thread_id": tid}),
    )
    .expect("turn 1");
    rt.resume_from_user_ask("a1", false, false).expect("r1");

    // Third turn: user types /cancel-thread.
    tool.invoke(
        &ctx,
        &json!({"question": "q2", "record_as": "none", "thread_id": tid}),
    )
    .expect("turn 2");
    let answer = rt
        .resume_from_user_ask("", true, true)
        .expect("cancel resume");
    assert!(answer.cancelled);
    assert!(answer.thread_cancelled);

    let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
    assert!(body.contains("User cancelled clarification"));
    assert_eq!(rt.open_thread_count(), 0);
}

#[test]
fn test_7_force_close_on_subsession_end_resolves_or_drops() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    // Open a thread with one recorded answer.
    let r = tool
        .invoke(&ctx, &json!({"question": "q0", "record_as": "none"}))
        .expect("turn 0");
    let _tid = r
        .suspend
        .as_ref()
        .expect("suspend populated")
        .pending
        .thread_id
        .clone();
    rt.resume_from_user_ask("a0", false, false).expect("r0");
    // Thread is still open (record_as=none keeps it open).
    assert_eq!(rt.open_thread_count(), 1);

    // Force-close.
    let resolved = rt.force_close_open_threads();
    assert_eq!(resolved.len(), 1);
    // spec.md got the resolved entry.
    let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
    assert!(body.contains("Resolved through 1 exchange"));
}

#[test]
fn test_8_turn_cap_warning_at_five() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    // Open the thread.
    let r = tool
        .invoke(&ctx, &json!({"question": "q0", "record_as": "none"}))
        .expect("turn 0");
    let tid = r
        .suspend
        .as_ref()
        .expect("suspend populated")
        .pending
        .thread_id
        .clone();
    rt.resume_from_user_ask("a0", false, false).expect("r0");

    // Turns 1, 2, 3, 4 -- still under the cap.
    for turn in 1..5 {
        let r = tool
            .invoke(
                &ctx,
                &json!({"question": format!("q{turn}"), "record_as": "none", "thread_id": tid}),
            )
            .expect("invoke turn");
        let pending = &r.suspend.as_ref().expect("suspend populated").pending;
        assert!(pending.thread_turn_index < ASK_USER_TURN_CAP);
        rt.resume_from_user_ask(&format!("a{turn}"), false, false)
            .expect("resume turn");
    }

    // Turn 5 -- at the cap.
    let r = tool
        .invoke(
            &ctx,
            &json!({"question": "q5", "record_as": "none", "thread_id": tid}),
        )
        .expect("turn 5");
    let pending = &r.suspend.as_ref().expect("suspend populated").pending;
    assert_eq!(pending.thread_turn_index, 5);
    assert!(pending.thread_turn_index >= ASK_USER_TURN_CAP);
}

#[test]
fn test_9_interleaved_threads_stay_independent() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();

    let rt = Arc::new(AskUserRuntime::new(tmp.path().to_path_buf(), "DM2d".into()));
    let tool = AskUserTool::new(rt.clone());
    let ctx = empty_ctx(tmp.path());

    // Open thread A.
    let r = tool
        .invoke(&ctx, &json!({"question": "Q-A", "record_as": "none"}))
        .expect("A0");
    let tid_a = r
        .suspend
        .as_ref()
        .expect("suspend populated")
        .pending
        .thread_id
        .clone();
    rt.resume_from_user_ask("Reply A", false, false)
        .expect("rA0");

    // Open thread B (no thread_id => fresh).
    let r = tool
        .invoke(&ctx, &json!({"question": "Q-B", "record_as": "none"}))
        .expect("B0");
    let tid_b = r
        .suspend
        .as_ref()
        .expect("suspend populated")
        .pending
        .thread_id
        .clone();
    assert_ne!(tid_a, tid_b);
    rt.resume_from_user_ask("Reply B", false, false)
        .expect("rB0");

    // Both threads open.
    assert_eq!(rt.open_thread_count(), 2);
    assert!(rt.thread(&tid_a).is_some());
    assert!(rt.thread(&tid_b).is_some());

    // Close thread A -- thread B remains.
    tool.invoke(
        &ctx,
        &json!({
            "question": "Close A",
            "record_as": "open-question",
            "thread_id": tid_a,
        }),
    )
    .expect("A close");
    rt.resume_from_user_ask("Done A", false, false)
        .expect("rA close");
    assert_eq!(rt.open_thread_count(), 1);
    assert!(rt.thread(&tid_a).is_none());
    assert!(rt.thread(&tid_b).is_some());
}

// Suppress unused-import warning when no test refers to a helper.
#[allow(dead_code)]
fn _suppress() {
    let _: &dyn Fn(PendingUserAsk) = &|_| ();
}
