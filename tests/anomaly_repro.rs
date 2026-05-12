//! Focused unit-level repros for the anomalies catalogued in the
//! model-robustness study (`docs/brainstorming/model-robustness-vllm-anomalies.md`).
//!
//! Goal: each anomaly the live K=3 study surfaces gets a small,
//! deterministic test here that pins the post-fix behavior. This
//! is the fast-iteration loop for orchestrator-side tweaks --
//! `cargo test --test anomaly_repro` is seconds, not the 15+
//! minutes a K=3 needs.
//!
//! These tests use `MockAgent` so no LLM calls fly out. They catch
//! ORCHESTRATOR-level regressions (the empty-response retry storm,
//! the BoxedAgent dispatch_with_tools fall-through). For
//! PROMPT-quality regressions (does the actual model call write_file
//! when told?) the live K=3 study harness stays the source of truth;
//! this suite is the complementary cheap check.
//!
//! Scope today: one regression test per fixed bug. New anomalies
//! should land a corresponding test here BEFORE the fix, so the
//! red-then-green cycle is visible in CI.

use std::path::PathBuf;

use sim_flow::session::protocol::{
    DiagnosticLevel, Event, HostEvent, HostInfo, LlmMessage, LlmToolCall, PROTOCOL_VERSION,
    SessionKindOut, StepMode,
};
use sim_flow::session::{
    AdvertisedToolCall, AutoOptions, CliAgent, LlmCallMetrics, MockAgent, TestHost, ToolAdvertise,
    run_auto,
};
use sim_flow::state::{Flow, State};

// -------------------------------------------------------------------
// BoxedAgent / wrapper fall-through regression (commit 12956e6).
//
// Bug: a CliAgent wrapper that overrides only `name` and `dispatch`
// silently routes native-tool-call requests through the trait's
// default `dispatch_with_tools` impl -- which drops the tool catalog
// and returns empty tool_calls. The original offender was e2e_auto's
// BoxedAgent. The fix is to forward `dispatch_with_tools` (and
// `adaptation_summary`) to the inner agent.
//
// This test pins the post-fix behavior: a passthrough wrapper that
// forwards both methods MUST surface the inner agent's scripted
// tool_calls all the way to the caller.
// -------------------------------------------------------------------

struct PassThroughWrapper {
    inner: Box<dyn CliAgent>,
}

impl CliAgent for PassThroughWrapper {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn dispatch(&self, messages: &[LlmMessage]) -> sim_flow::Result<(String, LlmCallMetrics)> {
        self.inner.dispatch(messages)
    }
    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
    ) -> sim_flow::Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        self.inner.dispatch_with_tools(messages, tools)
    }
}

#[test]
fn boxed_agent_wrapper_forwards_dispatch_with_tools() {
    let mock = MockAgent::new();
    mock.enqueue_with_tool_calls(
        "",
        vec![AdvertisedToolCall {
            id: Some("c1".into()),
            name: "read_file".into(),
            arguments_json: r#"{"path":"a.md"}"#.into(),
        }],
    );

    let wrapper = PassThroughWrapper {
        inner: Box::new(mock),
    };

    let tools = vec![
        ToolAdvertise {
            name: "read_file".into(),
            description: "Read a file.".into(),
            parameters: serde_json::json!({"type":"object"}),
        },
        ToolAdvertise {
            name: "write_file".into(),
            description: "Write a file.".into(),
            parameters: serde_json::json!({"type":"object"}),
        },
    ];

    let (text, calls, _metrics) = wrapper
        .dispatch_with_tools(&[], &tools)
        .expect("dispatch_with_tools should succeed");

    assert_eq!(text, "");
    assert_eq!(
        calls.len(),
        1,
        "wrapper dropped the inner agent's tool_calls"
    );
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].id.as_deref(), Some("c1"));
    // Pre-fix this test would see `calls.len() == 0` because the
    // trait-default `dispatch_with_tools` would silently call the
    // inner `dispatch` (which has no notion of tool_calls).
}

#[test]
fn mock_agent_enqueue_with_tool_calls_round_trips() {
    // Pin the MockAgent extension that the test suite needs.
    // `enqueue_with_tool_calls` was added so anomaly-repro tests
    // can canned the native-mode response shape (text + tool_calls)
    // without spinning up a live backend. A regression here means
    // the rest of the suite is silently testing the wrong path.
    let mock = MockAgent::new();
    mock.enqueue_with_tool_calls(
        "I'll read the spec.",
        vec![AdvertisedToolCall {
            id: Some("c1".into()),
            name: "read_file".into(),
            arguments_json: r#"{"path":"docs/spec.md"}"#.into(),
        }],
    );

    let tools = vec![ToolAdvertise {
        name: "read_file".into(),
        description: "Read a file.".into(),
        parameters: serde_json::json!({"type":"object"}),
    }];
    let (text, calls, _metrics) = mock
        .dispatch_with_tools(&[], &tools)
        .expect("mock dispatch_with_tools");

    assert_eq!(text, "I'll read the spec.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_file");
    // The mock records the catalog so tests can assert the
    // orchestrator actually advertised tools (vs the trait
    // fall-through that would skip it entirely).
    let seen = mock.seen_tools.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].len(), 1);
    assert_eq!(seen[0][0].name, "read_file");
}

#[test]
fn mock_agent_default_dispatch_with_tools_returns_empty_calls_via_trait_default() {
    // Sanity test the trait-default impl direction: an agent that
    // doesn't override `dispatch_with_tools` (subprocess CLI
    // agents, vscode.lm, anything pre-Phase-B) MUST keep working
    // when the host calls `dispatch_with_tools`. The default
    // drops the catalog and returns empty tool_calls.
    struct PlainDispatch;
    impl CliAgent for PlainDispatch {
        fn name(&self) -> &str {
            "plain"
        }
        fn dispatch(&self, _messages: &[LlmMessage]) -> sim_flow::Result<(String, LlmCallMetrics)> {
            Ok(("hi".into(), LlmCallMetrics::default()))
        }
    }
    let agent = PlainDispatch;
    let tools = vec![ToolAdvertise {
        name: "list_dir".into(),
        description: "List a directory.".into(),
        parameters: serde_json::json!({"type":"object"}),
    }];
    let (text, calls, _m) = agent
        .dispatch_with_tools(&[], &tools)
        .expect("trait default succeeds");
    assert_eq!(text, "hi");
    assert!(calls.is_empty(), "trait default must drop the catalog");
}

// -------------------------------------------------------------------
// empty-response retry storm (commit bb26a02).
//
// Bug: when the model returned `content: null, tool_calls: [...]`
// (the normal native-tool-only shape), the orchestrator's empty-
// response detection treated the turn as "empty" and re-prompted
// with "Your previous response was empty. Produce your answer now
// as plain text...". K=3 measurement saw median 16 / max 32 such
// retry events per trial, each paired with a NON-empty tool_calls
// array. The retry storms burned the iteration budget and capped
// the shortest trial at DM1.
//
// Fix: the empty-response detection AND the "skip post-processing"
// gate both require BOTH `text.is_empty()` AND
// `native_tool_calls.is_empty()` now. Tool-call-only turns route
// through normal dispatch unchanged.
//
// This test drives a real run_auto in manual mode with a TestHost
// scripted to deliver one tool-call-only turn, then asserts NO
// "LLM returned no content" diagnostic appears in the captured
// event stream. Pre-fix this would have one such diagnostic.
// -------------------------------------------------------------------

fn foundation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn init_project(tmp: &tempfile::TempDir) -> PathBuf {
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM0");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    project
}

fn auto_opts(project: &std::path::Path, mode: StepMode) -> AutoOptions {
    AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation_root(),
        llm_backend: "mock".into(),
        llm_model: None,
        llm_model_family_id: None,
        llm_runtime_profile_id: None,
        llm_debug_adaptation: false,
        llm_base_url: None,
        max_auto_iters: 2,
        max_critique_iters: 1,
        max_critique_no_progress_iters: 0,
        dm0_interactive: false,
        max_llm_requests: 4,
        max_identical_responses: 0,
        step_mode: mode,
        no_preamble: true,
    }
}

#[test]
fn tool_call_only_turn_does_not_trigger_empty_response_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    let mut host = TestHost::new();
    host.enqueue(HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "anomaly-repro".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    });

    // RunStep work for DM0. The orchestrator will emit
    // RequestLlmResponse; we deliver one tool-call-only turn:
    // empty content + a list_dir call. Pre-fix the orchestrator
    // would fire "Your previous response was empty" and re-prompt.
    host.enqueue(HostEvent::RunStep {
        step: "DM0".into(),
        kind: SessionKindOut::Work,
    });
    host.enqueue(HostEvent::LlmChunk {
        request_id: "lr-1".into(),
        text: String::new(),
    });
    host.enqueue(HostEvent::LlmEnd {
        request_id: "lr-1".into(),
        stop_reason: Some("stop".into()),
        tool_calls: vec![LlmToolCall {
            id: Some("call_1".into()),
            name: "list_dir".into(),
            arguments_json: r#"{"path":"docs"}"#.into(),
        }],
    });
    // The DM0 work session will iterate (no artifact landed yet)
    // and emit another RequestLlmResponse. We give it the same
    // shape twice more so the loop wind-down doesn't depend on
    // hitting a specific cap.
    for n in 2..=3 {
        host.enqueue(HostEvent::LlmChunk {
            request_id: format!("lr-{n}"),
            text: String::new(),
        });
        host.enqueue(HostEvent::LlmEnd {
            request_id: format!("lr-{n}"),
            stop_reason: Some("stop".into()),
            tool_calls: vec![LlmToolCall {
                id: Some(format!("call_{n}")),
                name: "list_dir".into(),
                arguments_json: r#"{"path":"docs"}"#.into(),
            }],
        });
    }
    host.enqueue(HostEvent::Shutdown);

    let _ = run_auto(auto_opts(&project, StepMode::Manual), &mut host);

    // KEY ASSERTION: no diagnostic should mention "LLM returned no
    // content" -- that's the exact string the empty-response retry
    // path emits. If the retry storm fires, it'd show up here.
    let empty_response_diagnostics: Vec<&str> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message,
            } if message.contains("LLM returned no content") => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        empty_response_diagnostics.is_empty(),
        "tool-call-only turns must not trigger the empty-response retry. \
         Got {} diagnostic(s): {:?}",
        empty_response_diagnostics.len(),
        empty_response_diagnostics,
    );
}

#[test]
fn truly_empty_response_still_triggers_diagnostic() {
    // The fix must NOT go too far the other way: a truly-empty
    // response (no text AND no tool_calls) is still a real failure
    // mode -- the LLM dropped the turn -- and the existing retry
    // nudge is the right behavior. Pin this so a future refactor
    // doesn't accidentally swallow the genuine case.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    let mut host = TestHost::new();
    host.enqueue(HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "anomaly-repro".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    });
    host.enqueue(HostEvent::RunStep {
        step: "DM0".into(),
        kind: SessionKindOut::Work,
    });
    // Truly-empty turn: empty content, empty tool_calls.
    host.enqueue(HostEvent::LlmChunk {
        request_id: "lr-1".into(),
        text: String::new(),
    });
    host.enqueue(HostEvent::LlmEnd {
        request_id: "lr-1".into(),
        stop_reason: Some("stop".into()),
        tool_calls: Vec::new(),
    });
    host.enqueue(HostEvent::Shutdown);

    let _ = run_auto(auto_opts(&project, StepMode::Manual), &mut host);

    let empty_response_diagnostics: Vec<&str> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message,
            } if message.contains("LLM returned no content") => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !empty_response_diagnostics.is_empty(),
        "truly-empty turn must still emit the retry diagnostic; got 0",
    );
}

// -------------------------------------------------------------------
// Future tests to add as anomalies get fixes:
//
// - empty-response retry path: needs a `run_one_turn` helper or
//   integration via TestHost driving a single Event::RequestLlmResponse
//   round-trip. Today's `run_auto` is heavy enough that the cheaper
//   level is to assert the new joint check (`text.is_empty() &&
//   native_tool_calls.is_empty()`) at the unit level. Skeleton
//   placeholder: see commit bb26a02 for the joint-check fix; a
//   focused test would feed the orchestrator one tool-call-only
//   turn and assert NO "Your previous response was empty"
//   diagnostic appears in the captured events.
//
// - edit-file-stale-old-string: feed the orchestrator a sequence
//   like (write_file foo content_A) -> (edit_file foo old=content_A
//   new=content_B) and assert the second call SUCCEEDS (i.e. the
//   tool's mental-model matches disk). Then induce drift -- a
//   second write_file in between -- and assert the edit fails with
//   the clear "old_string not found" message rather than something
//   ambiguous.
//
// - work-no-artifact: feed a script of read_file-only turns and
//   assert the orchestrator's no-artifact cap fires at the
//   configured iter limit (and not earlier or later). Pins the
//   max_auto_iters / no-progress bookkeeping.
//
// - critique-no-progress: feed two identical (count of blockers
//   unchanged) critique responses in a row and assert the no-progress
//   cap fires at iter 3, not at the absolute cap.
// -------------------------------------------------------------------
