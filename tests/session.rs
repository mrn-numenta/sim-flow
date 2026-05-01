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
            // disk" for a fresh DM0 init), User (opening prompt).
            assert_eq!(messages.len(), 4);
            assert!(messages[0].content.contains("Artifact-write convention"));
            assert!(messages[1].content.contains("Tool catalog"));
            assert!(messages[2].content.contains("docs/spec.md"));
            assert!(messages[2].content.contains("not yet on disk"));
            assert!(messages[3].content.contains("DM0 work session"));
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "cancelled"),
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "protocol-mismatch"),
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "cancelled"),
        other => panic!("expected SessionEnd cancelled, got {other:?}"),
    }
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "completed"),
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "completed"),
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
        Event::SessionEnd { reason, .. } => assert_eq!(reason, "completed"),
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
