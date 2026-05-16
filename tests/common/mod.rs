//! Shared helpers for the orchestrator integration tests.
//!
//! Each `tests/*.rs` integration-test binary declares `mod common;`
//! to pull these helpers in. Cargo doesn't compile `tests/common/`
//! as a standalone binary because it lives under a subdirectory --
//! that's the conventional Cargo idiom for sharing code between
//! integration tests.

use std::path::Path;

use sim_flow::client::SessionKind;
use sim_flow::session::orchestrator::OrchestratorOptions;
use sim_flow::session::protocol::{HostEvent, HostInfo, PROTOCOL_VERSION};
use sim_flow::state::{Flow, State};

#[allow(dead_code)]
pub fn init_project(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM0");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    project
}

#[allow(dead_code)]
pub fn foundation_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[allow(dead_code)]
pub fn opts(project: &Path, kind: SessionKind) -> OrchestratorOptions {
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

#[allow(dead_code)]
pub fn hello() -> HostEvent {
    HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "test-host".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    }
}

#[allow(dead_code)]
pub fn auto_opts(
    project: &Path,
    mode: sim_flow::session::protocol::StepMode,
) -> sim_flow::session::AutoOptions {
    sim_flow::session::AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation_root(),
        llm_backend: "test".into(),
        llm_model: None,
        llm_model_family_id: None,
        llm_runtime_profile_id: None,
        llm_debug_adaptation: false,
        llm_base_url: None,
        critique_llm_backend: None,
        critique_llm_model: None,
        critique_llm_model_family_id: None,
        critique_llm_runtime_profile_id: None,
        critique_llm_base_url: None,
        qa_llm_backend: None,
        qa_llm_model: None,
        qa_llm_model_family_id: None,
        qa_llm_runtime_profile_id: None,
        qa_llm_base_url: None,
        max_auto_iters: 3,
        max_critique_iters: 2,
        // Tests that drive the critique-iter cap explicitly rely
        // on flat-retry behavior (the original semantics). 0
        // disables the no-progress cap so the absolute cap is
        // the only signal.
        max_critique_no_progress_iters: 0,
        dm0_interactive: false,
        max_llm_requests: 50,
        max_identical_responses: 0,
        max_parallel_requests: 0,
        step_mode: mode,
        no_preamble: true,
    }
}

#[allow(dead_code)]
pub fn serve_one_chat_response(listener: std::net::TcpListener, body: String) {
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
