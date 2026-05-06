//! End-to-end tests for the orchestrator (Phase 9 M2).
//!
//! These tests drive `run_session` against a `TestHost` to exercise
//! the protocol surface without spawning a real LLM. The host
//! pre-scripts the LLM responses and inspects the orchestrator's
//! emitted events afterward.

use std::path::Path;

use sim_flow::client::SessionKind;
use sim_flow::session::host::TestHost;
use sim_flow::session::orchestrator::OrchestratorOptions;
use sim_flow::session::protocol::{Event, HostEvent, HostInfo, PROTOCOL_VERSION};
use sim_flow::session::run_session;
use sim_flow::state::{Flow, State};

fn init_project(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM0");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    project
}

fn foundation_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn opts(project: &Path, kind: SessionKind) -> OrchestratorOptions {
    OrchestratorOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation_root(),
        step_id: "DM0".into(),
        kind,
        candidate: None,
        llm_backend: "test".into(),
        llm_model: None,
        ..Default::default()
    }
}

fn hello() -> HostEvent {
    HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "test-host".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    }
}

#[test]
fn handshake_emits_hello_ack_and_phase_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello()).enqueue(HostEvent::Cancel);

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    // First event: HelloAck. Second: PhaseChanged. Third: RequestLlmResponse.
    match &host.written[0] {
        Event::HelloAck {
            protocol_version,
            session,
            step_descriptor,
            ..
        } => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            assert_eq!(session.step, "DM0");
            assert_eq!(
                step_descriptor.work_artifacts,
                vec!["docs/spec.md".to_string()]
            );
        }
        other => panic!("expected HelloAck first, got {other:?}"),
    }
    match &host.written[1] {
        Event::PhaseChanged { phase } => assert_eq!(phase, "chat"),
        other => panic!("expected PhaseChanged second, got {other:?}"),
    }
    match &host.written[2] {
        Event::RequestLlmResponse {
            messages, tools, ..
        } => {
            // System (convention + instructions), System (tool catalog),
            // System (current artifact state — spec.md is "not yet on
            // disk" for a fresh DM0 init), optional System (framework
            // API TOC when bundled docs are available), User (opening
            // prompt).
            assert!(messages.len() == 4 || messages.len() == 5);
            assert!(messages[0].content.contains("Artifact-write convention"));
            assert!(messages[1].content.contains("Tool catalog"));
            assert!(messages[2].content.contains("docs/spec.md"));
            assert!(messages[2].content.contains("not yet on disk"));
            let opening_idx = messages.len() - 1;
            assert!(messages[opening_idx].content.contains("DM0 work session"));
            if messages.len() == 5 {
                assert!(messages[3].content.contains("Framework API TOC"));
            }
            // Tool catalog also surfaces as a structured field for
            // backends that support native tool-use. Every step now
            // gets the same universal set.
            for expected in ["read_file", "write_file", "list_dir", "search"] {
                assert!(tools.iter().any(|t| t.name == expected));
            }
        }
        other => panic!("expected RequestLlmResponse third, got {other:?}"),
    }

    // Final event: SessionEnd { reason: cancelled } from the cancel
    // we enqueued during the LLM-await loop.
    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Cancelled
        ),
        other => panic!("expected SessionEnd, got {other:?}"),
    }
}

#[test]
fn protocol_version_mismatch_ends_session_with_error() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(HostEvent::Hello {
        protocol_version: "999".into(),
        host: HostInfo {
            name: "old-host".into(),
            version: "0.0".into(),
        },
        capabilities: vec![],
    });

    let err = run_session(opts(&project, SessionKind::Work), &mut host).unwrap_err();
    assert!(format!("{err}").contains("protocol version mismatch"));
    let last = host.written.last().expect("should have emitted SessionEnd");
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::ProtocolMismatch
        ),
        other => panic!("expected SessionEnd, got {other:?}"),
    }
}

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
    // The orchestrator generates request id `lr-1` for the first turn.
    host.enqueue_llm_response("lr-1", response.clone());
    // Then end the session cleanly.
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

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
    // Predecessors used to be inlined verbatim into the critique
    // session's system prompt; that burned tokens on long iteration
    // loops. We now emit a TOC (path + size for files; one-level
    // expansion for directories) and tell the agent to fetch content
    // via `read_file`. This test pins the new shape:
    // - File entries must include the file path and its byte size.
    // - Directory entries (e.g. `src/`) MUST expand one level deep
    //   so the model sees the file list and can't hallucinate
    //   "empty" without calling `list_dir`.
    // - The verbatim file content must NOT appear in any system
    //   message.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let spec_body = "# DM0 spec\n\nClock: 2 GHz\nNode: 7 nm\n";
    std::fs::write(project.join("docs/spec.md"), spec_body).unwrap();

    let mut host = TestHost::new();
    host.enqueue(hello());
    let critique_body = "All clean, no blockers.\n";
    let response = format!("```docs/critiques/DM0-critique.md\n{critique_body}```\n",);
    host.enqueue_llm_response("lr-1", response);
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Critique), &mut host).unwrap();

    let messages = host
        .written
        .iter()
        .find_map(|e| match e {
            Event::RequestLlmResponse { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .expect("expected at least one RequestLlmResponse");
    let system_blob = messages
        .iter()
        .filter(|m| m.role == sim_flow::session::protocol::LlmRole::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");
    let toc_line = format!("- `docs/spec.md` ({} bytes)", spec_body.len());
    assert!(
        system_blob.contains(&toc_line),
        "expected TOC line `{toc_line}` in system messages; got:\n{system_blob}",
    );
    assert!(
        !system_blob.contains("# DM0 spec"),
        "spec.md content should NOT be inlined under the TOC scheme",
    );

    let critique = std::fs::read_to_string(project.join("docs/critiques/DM0-critique.md")).unwrap();
    assert!(critique.contains("All clean"));
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
    host.enqueue_llm_response("lr-1", "ok");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let dm2d_opts = OrchestratorOptions {
        step_id: "DM2d".into(),
        ..opts(&project, SessionKind::Work)
    };
    let _ = run_session(dm2d_opts, &mut host);

    let system_blob = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::RequestLlmResponse { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .next()
        .expect("at least one RequestLlmResponse")
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

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, .. } => assert_eq!(
            *reason,
            sim_flow::session::protocol::SessionEndReason::Cancelled
        ),
        other => panic!("expected SessionEnd cancelled, got {other:?}"),
    }
}

#[test]
fn llm_error_emits_retry_followups_and_rich_user_input_prompt() {
    // When the host returns LlmError, the orchestrator must surface
    // the failure inline (Diagnostic), advertise quick-actions
    // (Followup retry / cancel), and prompt the user with a populated
    // RequestUserInput rather than the bare prompt-less form. This
    // gives operators agency on every failure instead of dropping
    // them at an empty input box with no context.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello()).enqueue(HostEvent::LlmError {
        request_id: "lr-1".into(),
        kind: "rate-limit".into(),
        message: "429 too many requests".into(),
    });
    // After the failure prompt, user cancels.
    host.enqueue(HostEvent::Cancel);

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    // Diagnostic carries the error verbatim.
    let saw_diag = host.written.iter().any(|e| match e {
        Event::Diagnostic { message, .. } => message.contains("rate-limit"),
        _ => false,
    });
    assert!(saw_diag, "expected error Diagnostic with kind=rate-limit");

    // Followup quick-actions: Retry + Cancel.
    let actions: Vec<&str> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::Followup { action, .. } => Some(action.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        actions.contains(&"/retry"),
        "expected /retry Followup, got actions={actions:?}",
    );
    assert!(
        actions.contains(&"/end-session"),
        "expected /end-session Followup, got actions={actions:?}",
    );

    // RequestUserInput prompt mentions the error and the available
    // commands so terminal hosts (no Followup rendering) still see
    // them.
    let prompt = host.written.iter().find_map(|e| match e {
        Event::RequestUserInput { prompt, .. } => prompt.clone(),
        _ => None,
    });
    let prompt = prompt.expect("expected a populated RequestUserInput prompt");
    assert!(
        prompt.contains("rate-limit"),
        "prompt missing kind: {prompt}"
    );
    assert!(prompt.contains("/retry"), "prompt missing /retry: {prompt}");
    assert!(
        prompt.contains("/end-session"),
        "prompt missing /end-session: {prompt}",
    );
}

#[test]
fn slash_retry_after_llm_error_reissues_request_without_user_turn() {
    // `/retry` must re-issue the *same* RequestLlmResponse without
    // pushing a User turn (the LLM should never see the literal
    // "/retry" text). Verify by counting RequestLlmResponse events
    // and inspecting the second one's messages: the message stack
    // must be byte-identical to the first.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello())
        .enqueue(HostEvent::LlmError {
            request_id: "lr-1".into(),
            kind: "transport".into(),
            message: "socket reset".into(),
        })
        .enqueue(HostEvent::UserMessage {
            text: "/retry".into(),
        });
    // Second turn succeeds and the user ends the session.
    host.enqueue_llm_response("lr-2", "no changes needed");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    let llm_requests: Vec<_> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::RequestLlmResponse { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        llm_requests.len(),
        2,
        "expected exactly two RequestLlmResponse events (initial + post-retry)",
    );
    assert_eq!(
        llm_requests[0].len(),
        llm_requests[1].len(),
        "/retry must not push a User turn into messages",
    );
    for (a, b) in llm_requests[0].iter().zip(llm_requests[1].iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

#[test]
fn write_outside_step_allowlist_is_rejected() {
    // DM0's work-session allowlist is `["docs/"]`. A fenced
    // artifact-write block targeting `src/lib.rs` must be rejected
    // (Diagnostic emitted, no file written) so the per-step write
    // scope binds the artifact-write convention, not just the
    // tool-call API. Otherwise the convention would be a bypass.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    let body = "Implementing the DM0 sketch.\n\n```src/lib.rs\nfn main() {}\n```\n";
    host.enqueue_llm_response("lr-1", body);
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    assert!(
        !project.join("src/lib.rs").exists(),
        "src/lib.rs must not have been written by a DM0 work session",
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
}

#[test]
fn end_session_signal_completes_without_emitting_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    // Empty LLM response; no artifacts written. User signals end.
    host.enqueue_llm_response("lr-1", "no changes needed");
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

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
        # Spec\n\nClock: 2 GHz\nNode: 7 nm\n\
        ```\n";
    host.enqueue_llm_response("lr-1", response);

    let mut o = opts(&project, SessionKind::Work);
    o.auto = true;
    o.max_auto_iters = 3;
    run_session(o, &mut host).unwrap();

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
    host.enqueue_llm_response("lr-1", bad);
    host.enqueue_llm_response("lr-2", bad);
    // Once the cap trips, the orchestrator falls through to
    // RequestUserInput; satisfy that with /end-session.
    host.enqueue(HostEvent::UserMessage {
        text: "/end-session".into(),
    });

    let mut o = opts(&project, SessionKind::Work);
    o.auto = true;
    o.max_auto_iters = 2;
    run_session(o, &mut host).unwrap();

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
    host.enqueue_llm_response(
        "lr-1",
        "Let me check the current code.\n\n```tool:read_file\nsrc/model/lib.rs\n```\n",
    );

    // Second LLM turn (after tool result fed back): plain "ok, done".
    host.enqueue_llm_response("lr-2", "Got it. Nothing to do.");
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
    run_session(opts, &mut host).unwrap();

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

    // The second RequestLlmResponse should include the tool result
    // as a user message in the messages array.
    let request_payloads: Vec<_> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::RequestLlmResponse { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .collect();
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
    host.enqueue_llm_response("lr-1", identical_call);
    host.enqueue_llm_response("lr-2", identical_call);

    // Turn 3: emit something different so the streak breaks and we
    // don't trip the strike-3 abort. The /end-session pumps the
    // session to completion so we can inspect the recorded events.
    host.enqueue_llm_response("lr-3", "Done with the lookup. /end-session");
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
    run_session(opts, &mut host).unwrap();

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

    // The third RequestLlmResponse's messages should include a User
    // message whose content begins with the loop-guard hint prefix
    // (the tool result for turn 2 was the message that got the
    // injection).
    let request_payloads: Vec<_> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::RequestLlmResponse { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .collect();
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

#[test]
fn terminal_host_drives_a_dm0_work_session_against_mock_agent() {
    use sim_flow::session::{MockAgent, TerminalHost};

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let agent = MockAgent::new();
    // Single LLM turn: write spec.md.
    agent.enqueue("Drafting spec.\n\n```docs/spec.md\n# Spec\n\nClock: 2 GHz\nNode: 7 nm\n```\n");
    // After the artifact lands the orchestrator emits RequestUserInput;
    // simulate the user typing /end-session via stdin.
    let stdin = std::io::Cursor::new(b"/end-session\n".to_vec());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut host = TerminalHost::new(agent, stdin, &mut stdout, &mut stderr);

    sim_flow::session::run_session(opts(&project, SessionKind::Work), &mut host).unwrap();

    let spec = std::fs::read_to_string(project.join("docs/spec.md")).unwrap();
    assert!(spec.contains("Clock: 2 GHz"), "spec.md should be written");
    let stderr_str = String::from_utf8(stderr).unwrap();
    assert!(stderr_str.contains("DM0 work session"));
    assert!(stderr_str.contains("[wrote docs/spec.md"));
    assert!(stderr_str.contains("session end (completed)"));
    let stdout_str = String::from_utf8(stdout).unwrap();
    assert!(
        stdout_str.contains("Drafting spec"),
        "assistant text should appear on stdout",
    );
}

#[test]
fn terminal_host_empty_user_line_cancels_the_session() {
    use sim_flow::session::{MockAgent, TerminalHost};

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let agent = MockAgent::new();
    agent.enqueue("Some assistant turn that won't write anything.");
    // First user reply is empty -> Cancel.
    let stdin = std::io::Cursor::new(b"\n".to_vec());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut host = TerminalHost::new(agent, stdin, &mut stdout, &mut stderr);

    sim_flow::session::run_session(opts(&project, SessionKind::Work), &mut host).unwrap();
    let stderr_str = String::from_utf8(stderr).unwrap();
    assert!(stderr_str.contains("session end (cancelled)"));
}

// -------------------------------------------------------------------
// M4 follow-up: Ollama / LM Studio HTTP agents.
//
// We hand-roll a tiny single-connection HTTP server here instead of
// pulling in another dep just for tests. It accepts one POST to
// /v1/chat/completions, returns a canned chat-completions JSON body,
// and shuts down. The OllamaAgent runs against it end-to-end.
// -------------------------------------------------------------------

// Flaky on macOS dev machines (`read body: Invalid argument (os error
// 22)` from the hand-rolled single-connection mock server). The agent
// itself is exercised end-to-end via LM Studio in real use; gate this
// behind `--ignored` so a clean `cargo test` doesn't fail. Re-run
// explicitly with `cargo test -- --ignored ollama_agent_round_trips`
// when iterating on the OpenAI-compat client.
#[test]
#[ignore = "flaky local-mock socket on macOS; LM Studio path is the primary OpenAI-compat coverage"]
fn ollama_agent_round_trips_against_mock_chat_completions_server() {
    use sim_flow::session::OllamaAgent;
    use sim_flow::session::protocol::{LlmMessage, LlmRole};

    let canned_body = r#"{
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from mock Ollama."
                },
                "finish_reason": "stop"
            }
        ]
    }"#;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body_owned = canned_body.to_string();
    let server_handle = std::thread::spawn(move || serve_one_chat_response(listener, body_owned));

    let agent = OllamaAgent::new(
        Some(format!("http://127.0.0.1:{port}/v1")),
        Some("llama3.1".into()),
    );
    let response = sim_flow::session::CliAgent::dispatch(
        &agent,
        &[LlmMessage {
            role: LlmRole::User,
            content: "ping".into(),
            attachments: Vec::new(),
        }],
    )
    .expect("agent dispatch should succeed against mock server");
    assert!(
        response.contains("Hello from mock Ollama."),
        "expected response from mock server, got: {response}"
    );
    server_handle.join().unwrap();
}

fn serve_one_chat_response(listener: std::net::TcpListener, body: String) {
    use std::io::{Read, Write};
    let (mut stream, _) = listener.accept().expect("accept incoming");
    // Drain headers + body until we see "\r\n\r\n" then a content-
    // length-bounded body. The actual content doesn't matter for
    // the test - we just need to reply.
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).expect("write reply");
    stream.flush().ok();
}

// -------------------------------------------------------------------
// Schema drift check: regenerate the schema and compare against the
// committed file. CI fails when they diverge - fix by rerunning
// `cargo run -p sim-flow --bin session_protocol_schema >
// tools/sim-flow/docs/flow/session-protocol.schema.json`.
// -------------------------------------------------------------------

#[test]
fn session_protocol_schema_matches_committed_file() {
    let generated = sim_flow::session::protocol::protocol_schema();
    let pretty = serde_json::to_string_pretty(&generated).unwrap();

    let committed_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/flow/session-protocol.schema.json");
    let committed =
        std::fs::read_to_string(&committed_path).expect("committed schema file should exist");

    assert_eq!(
        pretty.trim_end(),
        committed.trim_end(),
        "session-protocol.schema.json is out of date. Regenerate with:\n\
         cargo run -p sim-flow --bin session_protocol_schema > {}",
        committed_path.display(),
    );
}

// -------------------------------------------------------------------
// Manual-mode (`run_auto` with `step_mode = Manual`).
//
// `run_auto` runs in one of two step-axis modes. These tests exercise
// the manual-mode parking loop and its command dispatchers, plus the
// auto-loop -> manual-loop transitions on `SetStepMode { manual }` and
// the cap-exceeded path. The transport here is a `TestHost`; the
// driver wraps it in an `AutoHost` to intercept SetStepMode / Shutdown
// and route per-step commands.
// -------------------------------------------------------------------

fn auto_opts(
    project: &Path,
    mode: sim_flow::session::protocol::StepMode,
) -> sim_flow::session::AutoOptions {
    sim_flow::session::AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation_root(),
        llm_backend: "test".into(),
        llm_model: None,
        max_auto_iters: 3,
        max_critique_iters: 2,
        dm0_interactive: false,
        max_llm_requests: 50,
        max_identical_responses: 0,
        step_mode: mode,
    }
}

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
    host.enqueue(hello());

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    // The orchestrator reads Hello once on start, sends HelloAck,
    // then enters the parking loop. With no further commands the
    // parking loop reads None and exits via HostClosed.
    host.enqueue(hello());

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    // command, so no LLM request should ever go out.
    let saw_llm = host
        .written
        .iter()
        .any(|e| matches!(e, Event::RequestLlmResponse { .. }));
    assert!(
        !saw_llm,
        "no LLM request should fire in a parked manual run"
    );
}

#[test]
fn manual_mode_dispatches_run_gate_and_keeps_parking() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::RunGate { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    let saw_llm = host
        .written
        .iter()
        .any(|e| matches!(e, Event::RequestLlmResponse { .. }));
    assert!(!saw_llm, "RunGate should not dispatch an LLM call");
}

#[test]
fn manual_mode_dispatches_run_step_and_runs_a_real_subsession() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
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
        # Spec\n\nClock: 2 GHz\nNode: 7 nm\n\
        ```\n";
    host.enqueue_llm_response("lr-1", response);
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    host.enqueue(hello());
    host.enqueue(HostEvent::Reset { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    host.enqueue(hello());
    host.enqueue(HostEvent::Reset { step: "DM0".into() });
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

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
    host.enqueue(hello());
    // SetStepMode flips the flag (intercepted by AutoHost) and emits
    // StepModeChanged. The auto loop then takes over and tries to
    // run a DM0 work sub-session. We don't enqueue an LLM response,
    // so the orchestrator's read returns None and the run terminates
    // — but only AFTER we observe StepModeChanged { auto }.
    host.enqueue(HostEvent::SetStepMode {
        mode: StepMode::Auto,
    });

    let _ = sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host);

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

#[test]
fn auto_mode_cap_exceeded_flips_to_manual_and_emits_step_mode_changed() {
    // The orchestrator's per-session cap fires after max_auto_iters
    // bad responses. Today the auto driver flips the shared step-
    // mode flag to manual and emits StepModeChanged so the dashboard
    // toggle matches reality. The parking loop then takes over;
    // here we have no further script so the run exits cleanly.
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    // The very first sub-session in run_auto reads Hello from the
    // actual host (every sub-session AFTER the first uses a
    // synthetic Hello queued by AutoHost).
    host.enqueue(hello());
    let bad = "```docs/spec.md\n# Spec\n\nClock: 2 GHz\n```\n";
    host.enqueue_llm_response("lr-1", bad);
    host.enqueue_llm_response("lr-2", bad);
    // No /end-session needed: AutoHost queues a Cancel on cap and
    // the orchestrator stops itself.

    let mut opts = auto_opts(&project, StepMode::Auto);
    opts.max_auto_iters = 2;
    sim_flow::session::run_auto(opts, &mut host).unwrap();

    let saw_to_manual = host
        .written
        .iter()
        .any(|e| matches!(e, Event::StepModeChanged { mode } if matches!(mode, StepMode::Manual)));
    assert!(
        saw_to_manual,
        "cap-exceeded path should emit StepModeChanged {{ manual }}; events: {:?}",
        host.written,
    );
    let saw_diag = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level, message }
                if matches!(level, sim_flow::session::DiagnosticLevel::Error)
                    && message.contains("flipping to manual mode")
        )
    });
    assert!(
        saw_diag,
        "cap-exceeded path should emit a clarifying Diagnostic; events: {:?}",
        host.written,
    );
}

#[test]
fn manual_mode_shutdown_terminates_cleanly() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host).unwrap();

    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, message } => {
            assert_eq!(
                *reason,
                sim_flow::session::protocol::SessionEndReason::Completed
            );
            assert!(
                message.as_deref().unwrap_or("").contains("shut down"),
                "shutdown SessionEnd should mention shutdown; got {message:?}"
            );
        }
        other => panic!("expected SessionEnd, got {other:?}"),
    }
}
