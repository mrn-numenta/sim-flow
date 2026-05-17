//! Integration tests for terminal.
//!
//! Shared helpers live in `tests/common/mod.rs`.

use sim_flow::client::SessionKind;
use sim_flow::session::MockAgent;

mod common;
use common::{init_project, opts, serve_one_chat_response};

#[test]
fn terminal_host_drives_a_dm0_work_session_against_mock_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut agent = MockAgent::new();
    // Single LLM turn: write spec.md.
    agent.enqueue("Drafting spec.\n\n```docs/spec.md\n# Spec\n\nClock: 2 GHz\nNode: 7 nm\n```\n");
    // After the artifact lands the orchestrator emits RequestUserInput;
    // simulate the user typing /end-session via stdin.
    let stdin = std::io::Cursor::new(b"/end-session\n".to_vec());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut presenter =
        sim_flow::session::StderrPresenter::new("mock", stdin, &mut stdout, &mut stderr);

    sim_flow::session::run_session(
        opts(&project, SessionKind::Work),
        &mut presenter,
        &mut agent,
    )
    .unwrap();

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
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut agent = MockAgent::new();
    agent.enqueue("Some assistant turn that won't write anything.");
    // First user reply is empty -> Cancel.
    let stdin = std::io::Cursor::new(b"\n".to_vec());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut presenter =
        sim_flow::session::StderrPresenter::new("mock", stdin, &mut stdout, &mut stderr);

    sim_flow::session::run_session(
        opts(&project, SessionKind::Work),
        &mut presenter,
        &mut agent,
    )
    .unwrap();
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
        None,
        None,
    );
    let response = sim_flow::session::CliAgent::dispatch(
        &agent,
        &[LlmMessage {
            role: LlmRole::User,
            content: "ping".into(),
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        }],
    )
    .expect("agent dispatch should succeed against mock server");
    let (text, _metrics) = response;
    assert!(
        text.contains("Hello from mock Ollama."),
        "expected response from mock server, got: {text}"
    );
    server_handle.join().unwrap();
}
