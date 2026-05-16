//! Integration tests for work.
//!
//! Shared helpers live in `tests/common/mod.rs`.

use sim_flow::client::SessionKind;
use sim_flow::session::host::TestHost;
use sim_flow::session::orchestrator::OrchestratorOptions;
use sim_flow::session::protocol::{Event, HostEvent};
use sim_flow::session::{MockAgent, run_session};

mod common;
use common::{hello, init_project, opts};

#[test]
fn work_session_writes_artifacts_and_emits_gate_result() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());

    // First LLM turn: agent emits a fenced docs/spec.md block.
    let spec_body = "# Spec\n\nClock: 2 GHz\nNode: 7 nm\n";
    let response = format!(
        "Here is the spec.\n\n```docs/spec.md\n{spec_body}```\n\nLet me know if you want changes.",
    );
    let mut mock = MockAgent::new();
    mock.enqueue(response.clone());
    // Then end the session cleanly.
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host, &mut mock).unwrap();

    // Verify the spec.md landed on disk.
    let written = std::fs::read_to_string(project.join("docs/spec.md")).unwrap();
    assert!(written.contains("Clock: 2 GHz"));
    assert!(written.contains("Node: 7 nm"));

    // Verify the orchestrator emitted ArtifactWritten. No automatic
    // GateResult is emitted post-artifact: gate runs only when the
    // user explicitly requests it via /gate or the dashboard's
    // "Run Gate" button.
    let saw_artifact = host
        .written
        .iter()
        .any(|e| matches!(e, Event::ArtifactWritten { path, .. } if path == "docs/spec.md"));
    let saw_gate = host
        .written
        .iter()
        .any(|e| matches!(e, Event::GateResult { .. }));
    assert!(saw_artifact, "expected ArtifactWritten for spec.md");
    assert!(
        !saw_gate,
        "post-artifact gate emission was removed; only /gate / /advance should produce GateResult",
    );
}

#[test]
fn critique_session_lists_predecessor_inputs_as_toc() {
    // Inputs are listed as a TOC (path + size for files;
    // one-level expansion for directories). By default the
    // orchestrator does NOT inline file bodies regardless of
    // size -- the agent reads what it needs via `read_file`,
    // matching the "every spec / plan / analysis doc is paginated
    // or referenced via TOC" rule. The inlining machinery is
    // still in place behind `SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES`
    // for callers that want to opt in; the
    // `predecessor_inlining_can_be_enabled_via_env_var` test
    // pins that escape hatch.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let spec_body = "# DM0 spec\n\nClock: 2 GHz\nNode: 7 nm\n";
    std::fs::write(project.join("docs/spec.md"), spec_body).unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());
    let critique_body = "All clean, no blockers.\n";
    let response = format!("```docs/critiques/DM0-critique.md\n{critique_body}```\n",);
    let mut mock = MockAgent::new();
    mock.enqueue(response);
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Critique), &mut host, &mut mock).unwrap();

    let seen = mock.seen.lock().unwrap();
    let messages = seen.first().expect("expected at least one LLM dispatch");
    let system_blob = messages
        .iter()
        .filter(|m| m.role == sim_flow::session::protocol::LlmRole::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");
    let toc_line = format!("- `docs/spec.md` ({} bytes)", spec_body.len());
    assert!(
        system_blob.contains(&toc_line),
        "expected TOC-only line `{toc_line}` in system messages; got:\n{system_blob}",
    );
    assert!(
        !system_blob.contains("# DM0 spec"),
        "spec body MUST NOT be inlined by default; agent should read on demand. Got system blob:\n{system_blob}"
    );
    // No TOC entry should render in `(N bytes, inlined below):` form
    // and no fenced code block should appear right after the spec.md
    // TOC line. The general word "inlined" does appear elsewhere
    // (e.g. critique-body inlining wording is unrelated).
    assert!(
        !system_blob.contains("inlined below):\n\n```"),
        "no TOC entry should render as inlined fenced block when inlining is disabled; got:\n{system_blob}",
    );

    let critique = std::fs::read_to_string(project.join("docs/critiques/DM0-critique.md")).unwrap();
    assert!(critique.contains("All clean"));
}

#[test]
fn large_predecessor_inputs_stay_as_toc_only() {
    // Files at or above the 4 KB inline threshold are still
    // listed as bare TOC entries; the body must NOT appear in
    // the system prompt. Use a real >4 KB file rather than env
    // var manipulation so this test stays parallel-safe with the
    // small-file inlining test.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let unique_marker = "MARKER_LARGE_SPEC_BODY_DO_NOT_INLINE";
    let big_chunk = "x".repeat(5000);
    let spec_body = format!("# DM0 spec\n\n{unique_marker}\n\n{big_chunk}\n");
    std::fs::write(project.join("docs/spec.md"), &spec_body).unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());
    let critique_body = "All clean, no blockers.\n";
    let response = format!("```docs/critiques/DM0-critique.md\n{critique_body}```\n",);
    let mut mock = MockAgent::new();
    mock.enqueue(response);
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Critique), &mut host, &mut mock).unwrap();

    let seen = mock.seen.lock().unwrap();
    let messages = seen.first().expect("expected at least one LLM dispatch");
    let system_blob = messages
        .iter()
        .filter(|m| m.role == sim_flow::session::protocol::LlmRole::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");
    let toc_line = format!("- `docs/spec.md` ({} bytes)", spec_body.len());
    assert!(
        system_blob.contains(&toc_line),
        "expected bare TOC line; got:\n{system_blob}",
    );
    assert!(
        !system_blob.contains(unique_marker),
        "files over threshold should NOT inline body content",
    );
}

#[test]
fn work_session_expands_directory_inputs_one_level_in_the_toc() {
    // Regression: with a bare `(directory)` TOC entry, Qwen3-Coder
    // hallucinated "src/ is empty" without calling list_dir.
    // Directories must expand one level so the model can SEE the
    // file list in the prompt and stop guessing.
    //
    // DM2d is the smallest DM step whose work_artifacts includes a
    // directory (`src/`), which is what we want to exercise.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    std::fs::create_dir_all(project.join("src/model")).unwrap();
    std::fs::write(project.join("src/lib.rs"), "// lib\n").unwrap();
    std::fs::write(project.join("src/main.rs"), "// main\n").unwrap();
    std::fs::write(project.join("src/model/mod.rs"), "// mod\n").unwrap();
    // Bump the project's current_step to DM2d so the TOC pulls
    // src/ in (it's a DM2d work_artifact).
    let state_path = project.join(".sim-flow/state.toml");
    let original = std::fs::read_to_string(&state_path).unwrap();
    let updated = original.replace("current_step = \"DM0\"", "current_step = \"DM2d\"");
    std::fs::write(&state_path, updated).unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());
    let mut mock = MockAgent::new();
    mock.enqueue("ok");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let dm2d_opts = OrchestratorOptions {
        step_id: "DM2d".into(),
        ..opts(&project, SessionKind::Work)
    };
    let _ = run_session(dm2d_opts, &mut host, &mut mock);

    let seen = mock.seen.lock().unwrap();
    let first = seen.first().expect("at least one LLM dispatch");
    let system_blob = first
        .iter()
        .filter(|m| m.role == sim_flow::session::protocol::LlmRole::System)
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n---\n");

    assert!(
        system_blob.contains("- `src/` (directory, 3 entries)"),
        "expected `src/` TOC header to expand to 3 entries; got:\n{system_blob}",
    );
    assert!(
        system_blob.contains("- lib.rs (7 bytes)"),
        "expected `src/lib.rs` size in directory expansion; got:\n{system_blob}",
    );
    assert!(
        system_blob.contains("- main.rs (8 bytes)"),
        "expected `src/main.rs` size in directory expansion; got:\n{system_blob}",
    );
    assert!(
        system_blob.contains("- model/ (directory, 1 entry)"),
        "expected nested `src/model/` to appear as sub-directory with entry count; got:\n{system_blob}",
    );
}

#[test]
fn cancel_during_llm_call_ends_session_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello()).enqueue(HostEvent::Cancel);
    let mut mock = MockAgent::new();

    run_session(opts(&project, SessionKind::Work), &mut host, &mut mock).unwrap();

    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Cancelled
        ),
        other => panic!("expected SessionEnd cancelled, got {other:?}"),
    }
}

// TODO: Re-add the LLM-error / `/retry` regression tests once
// MockAgent gains an `enqueue_error` API to script per-dispatch
// failures (the orchestrator no longer reads `HostEvent::LlmError`,
// and the error path is exercised through the `LlmAdapter` return
// value instead).

#[test]
fn write_outside_step_allowlist_is_rejected_and_fed_back_to_agent() {
    // DM0's work-session allowlist is `["docs/"]`. A fenced
    // artifact-write block targeting `src/lib.rs` must be rejected
    // (Diagnostic emitted, no file written, error ToolInvoked) so
    // the per-step write scope binds the artifact-write convention,
    // not just the tool-call API. The rejection must also be
    // threaded back to the agent as a User turn so it can correct
    // on the next iteration instead of marching into validators
    // assuming the write succeeded.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    // Turn 1: bad path — gets rejected and fed back.
    let bad = "Implementing the DM0 sketch.\n\n```src/lib.rs\nfn main() {}\n```\n";
    let mut mock = MockAgent::new();
    mock.enqueue(bad);
    // Turn 2: agent corrects to a docs/ path.
    let good = "```docs/spec.md\n# Spec\nClock: 2 GHz, Node: 7 nm\n```\n";
    mock.enqueue(good);
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host, &mut mock).unwrap();

    assert!(
        !project.join("src/lib.rs").exists(),
        "src/lib.rs must not have been written by a DM0 work session",
    );
    assert!(
        project.join("docs/spec.md").exists(),
        "the corrected docs/spec.md write must have landed",
    );
    let saw_rejection = host.written.iter().any(|e| match e {
        Event::Diagnostic { message, .. } => {
            message.contains("write allowlist") || message.contains("outside the per-step")
        }
        _ => false,
    });
    assert!(
        saw_rejection,
        "expected a Diagnostic explaining the allowlist rejection",
    );

    // The rejection must surface as an `error` ToolInvoked event so
    // hosts that render tool activity (the dashboard, the chat UI)
    // show a failure marker instead of silently dropping the call.
    let saw_error_tool = host.written.iter().any(|e| match e {
        Event::ToolInvoked { status, .. } => status == "error",
        _ => false,
    });
    assert!(
        saw_error_tool,
        "expected a ToolInvoked with status=error for the rejected write",
    );

    // The rejection must be threaded back to the LLM as a User turn
    // before the orchestrator re-issues. Without this the agent
    // marches into validators / gates believing the write landed.
    let llm_requests = mock.seen.lock().unwrap();
    assert_eq!(
        llm_requests.len(),
        2,
        "expected re-issue after rejection (initial + corrected)",
    );
    let last_user_in_second = llm_requests[1]
        .iter()
        .rev()
        .find(|m| matches!(m.role, sim_flow::session::protocol::LlmRole::User))
        .expect("second request must include a User turn");
    assert!(
        last_user_in_second
            .content
            .contains("Artifact-write rejections"),
        "feedback turn must explain the rejection: {}",
        last_user_in_second.content,
    );
    assert!(
        last_user_in_second.content.contains("src/lib.rs"),
        "feedback turn must name the rejected path: {}",
        last_user_in_second.content,
    );
}

#[test]
fn end_session_signal_completes_without_emitting_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    // Empty LLM response; no artifacts written. User signals end.
    let mut mock = MockAgent::new();
    mock.enqueue("no changes needed");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host, &mut mock).unwrap();

    // /end-session should not auto-emit GateResult: the user runs
    // gate explicitly via /gate or the dashboard's "Run Gate".
    let saw_gate = host
        .written
        .iter()
        .any(|e| matches!(e, Event::GateResult { .. }));
    assert!(!saw_gate, "/end-session should not emit a GateResult");
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
// Auto-mode (Phase 1): per-session structural-gate convergence.
// -------------------------------------------------------------------
