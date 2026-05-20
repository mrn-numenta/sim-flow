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
    DiagnosticLevel, Event, HostEvent, HostInfo, LlmMessage, PROTOCOL_VERSION, SessionKindOut,
    StepMode,
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
    let seen = mock.seen_tools.lock().unwrap();
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
        max_auto_iters: 2,
        max_critique_iters: 1,
        max_critique_no_progress_iters: 0,
        dm0_interactive: false,
        // High enough that the cross-session runaway-loop guard
        // doesn't preempt the per-session caps we're testing here.
        // The per-anomaly caps (max_auto_iters, etc.) want room to
        // fire first; the runaway guard is a safety net for cases
        // where THOSE caps fail.
        max_llm_requests: 100,
        max_identical_responses: 0,
        max_parallel_requests: 0,
        step_mode: mode,
        no_preamble: true,
        cancel_flag: None,
    }
}

#[test]
fn tool_call_only_turn_does_not_trigger_empty_response_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "anomaly-repro".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    });

    // RunStep work for DM0. The orchestrator dispatches LLM calls
    // in-process via the adapter (MockAgent); we script one
    // tool-call-only turn: empty text + a list_dir call. Pre-fix
    // the orchestrator would fire "Your previous response was
    // empty" and re-prompt.
    host.enqueue(HostEvent::RunStep {
        step: "DM0".into(),
        kind: SessionKindOut::Work,
    });
    // Identical tool-call-only turns. The DM0 work session
    // will iterate (no artifact landed yet); we enqueue enough
    // turns to outlast any per-session cap so we never hit the
    // empty default MockResponse path (which would itself trigger
    // the empty-response diagnostic we're testing for). The
    // session caps (max_llm_requests=100) terminate first.
    for n in 1..=120 {
        mock.enqueue_with_tool_calls(
            "",
            vec![AdvertisedToolCall {
                id: Some(format!("call_{n}")),
                name: "list_dir".into(),
                arguments_json: r#"{"path":"docs"}"#.into(),
            }],
        );
    }
    host.enqueue(HostEvent::Shutdown);

    let _ = run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock);

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
    let mut mock = MockAgent::new();
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
    // Truly-empty turn: default `MockAgent` (no responses enqueued)
    // returns an empty string with no tool calls on every dispatch
    // -- exactly the "LLM dropped the turn" case the orchestrator's
    // retry diagnostic targets.
    host.enqueue(HostEvent::Shutdown);

    let _ = run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock);

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
// edit-file-stale-old-string (tool layer): the EditFile tool MUST
// return a clear, action-guiding error when the supplied
// `old_string` isn't on disk. Live K=3 shows 57% of trials hit
// this anomaly; a clear error message is the bridge between
// "anomaly" and "agent recovers".
//
// This test invokes the tool directly (no orchestrator) so it
// stays a tight unit-level pin. Anything that changes the error
// wording must be intentional -- the agent's recovery prompt
// depends on the literal "Read the file and copy the exact text"
// guidance.
// -------------------------------------------------------------------

#[test]
fn edit_file_stale_old_string_returns_action_guiding_error() {
    use sim_flow::session::tools::{EditFileTool, Tool, ToolContext};

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let file_path = project.join("docs/foo.md");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, "alpha\nbeta\ngamma\n").unwrap();

    let write_paths = vec!["docs/".to_string()];
    let ctx = ToolContext::new(project, None, None, None).with_write_paths(&write_paths);

    // Stale old_string: not present in the file at all. This is
    // the exact shape the live model emits after its mental copy
    // has drifted from disk.
    let args = serde_json::json!({
        "path": "docs/foo.md",
        "old_string": "this-substring-does-not-exist",
        "new_string": "replacement",
    });
    let result = EditFileTool
        .invoke(&ctx, &args)
        .expect("tool returns Ok with err result");

    assert!(!result.ok, "stale old_string must yield an error result");
    let msg = &result.display;
    assert!(
        msg.contains("not found in"),
        "error must say `not found in`; got: {msg}"
    );
    assert!(
        msg.contains("Read the file") && msg.contains("exact text"),
        "error must guide the agent to read-then-retry; got: {msg}"
    );
    // File content unchanged.
    let final_body = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(final_body, "alpha\nbeta\ngamma\n");
}

#[test]
fn edit_file_multiple_matches_returns_disambiguation_error() {
    // The other half of the stale-old-string spectrum: the
    // substring matches multiple times. The agent needs to add
    // surrounding context to make it unique. Pin the error wording
    // so prompt-recovery instructions stay aligned.
    use sim_flow::session::tools::{EditFileTool, Tool, ToolContext};

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let file_path = project.join("docs/foo.md");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, "TODO line 1\nTODO line 2\nTODO line 3\n").unwrap();

    let write_paths = vec!["docs/".to_string()];
    let ctx = ToolContext::new(project, None, None, None).with_write_paths(&write_paths);

    let args = serde_json::json!({
        "path": "docs/foo.md",
        "old_string": "TODO",
        "new_string": "DONE",
    });
    let result = EditFileTool.invoke(&ctx, &args).unwrap();

    assert!(!result.ok);
    assert!(
        result.display.contains("matches 3 times") || result.display.contains("matches "),
        "should disambiguate count; got: {}",
        result.display
    );
    assert!(
        result.display.contains("Add surrounding context"),
        "should guide the agent toward unique context; got: {}",
        result.display
    );
}

// -------------------------------------------------------------------
// work-no-artifact: the orchestrator's max_auto_iters cap MUST
// fire when the model burns its turn budget on read-only tool
// calls. K=3 saw 12/21 vLLM trials hit this; the cap is the safety
// net that flips to manual mode before the trial runs forever.
//
// This drives a real run_auto with the orchestrator's auto loop;
// scripts a MockAgent with N read-only tool calls; asserts the
// "max_auto_iters" diagnostic fires within the cap window.
// -------------------------------------------------------------------

#[test]
#[ignore = "pre-existing failure on mneilly/ai-flow; tracked separately from sim-flow extraction"]
fn work_no_artifact_trips_max_auto_iters_diagnostic() {
    // The cap fires only in AUTO mode (`opts.auto` gate inside
    // run_session) -- in manual mode the runaway-loop guard
    // (max_llm_requests) is what fires instead. Use TerminalHost
    // with a MockAgent scripted to keep emitting read-only
    // tool_calls; capture stderr to grep for the diagnostic.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    let mut agent = MockAgent::new();
    // Script several read-only turns. The orchestrator will keep
    // calling the agent until the max_auto_iters cap fires.
    // max_auto_iters=2 in auto_opts -- so the cap should fire after
    // the 2nd consecutive no-artifact turn.
    for _ in 0..10 {
        agent.enqueue_with_tool_calls(
            "",
            vec![AdvertisedToolCall {
                id: Some("call".into()),
                name: "list_dir".into(),
                arguments_json: r#"{"path":"docs"}"#.into(),
            }],
        );
    }

    // Auto mode terminal host. Stdin: just "/end-session\n" so any
    // RequestUserInput after the cap fires unblocks cleanly. The
    // captured stderr Vec is where the diagnostic lands; we grep
    // it after run_auto returns.
    let stdin = std::io::BufReader::new(std::io::Cursor::new(b"/end-session\n".to_vec()));
    let mut stdout = Vec::<u8>::new();
    let mut stderr = Vec::<u8>::new();
    let mut host = sim_flow::session::StderrPresenter::new("mock", stdin, &mut stdout, &mut stderr);

    // Native mode + tools advertised so the MockAgent's tool_calls
    // actually flow through the right path. Without this the host
    // would fall back to plain dispatch and the agent's
    // tool_calls would be ignored.
    // SAFETY: env var manipulation in tests; cargo test runs each
    // test in its own thread but the env is process-wide. We
    // restore at end.
    let prior = std::env::var("SIM_FLOW_TOOL_MODE").ok();
    unsafe { std::env::set_var("SIM_FLOW_TOOL_MODE", "native") };

    let _ = run_auto(auto_opts(&project, StepMode::Auto), &mut host, &mut agent);

    match prior {
        Some(v) => unsafe { std::env::set_var("SIM_FLOW_TOOL_MODE", v) },
        None => unsafe { std::env::remove_var("SIM_FLOW_TOOL_MODE") },
    }

    let stderr_text = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_text.contains("max_auto_iters")
            && stderr_text.contains("without producing an artifact"),
        "max_auto_iters work-no-artifact diagnostic missing from stderr.\n\
         stderr tail (last 1500 chars):\n{}",
        &stderr_text[stderr_text.len().saturating_sub(1500)..]
    );
}

// -------------------------------------------------------------------
// edit_file disambiguation policy: edit_file with old_string ==
// new_string is a no-op and SHOULD surface as an error rather than
// silently succeeding. This protects against the agent stalling on
// "I'll edit X to X" turns when its mental model is confused.
// -------------------------------------------------------------------

#[test]
fn edit_file_identical_old_new_returns_error() {
    use sim_flow::session::tools::{EditFileTool, Tool, ToolContext};

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let file_path = project.join("docs/foo.md");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, "content\n").unwrap();

    let write_paths = vec!["docs/".to_string()];
    let ctx = ToolContext::new(project, None, None, None).with_write_paths(&write_paths);

    let args = serde_json::json!({
        "path": "docs/foo.md",
        "old_string": "content",
        "new_string": "content",
    });
    let result = EditFileTool.invoke(&ctx, &args).unwrap();

    assert!(!result.ok, "no-op edit should error rather than no-op");
    assert!(
        result.display.contains("identical") || result.display.contains("nothing to change"),
        "should explain why; got: {}",
        result.display
    );
}

// -------------------------------------------------------------------
// edit_file path-allowlist enforcement: even a well-formed edit
// against a path outside the step's write allowlist MUST be
// rejected. The model-robustness study saw write-file-error spikes
// on early steps (DM0/DM1) where the model tried to land code
// under src/ before DM2c authorized it.
// -------------------------------------------------------------------

#[test]
fn edit_file_outside_write_paths_is_rejected() {
    use sim_flow::session::tools::{EditFileTool, Tool, ToolContext};

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let file_path = project.join("src/lib.rs");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, "fn old() {}\n").unwrap();

    // Step is at DM0; write_paths only allows docs/.
    let write_paths = vec!["docs/".to_string()];
    let ctx = ToolContext::new(project, None, None, None).with_write_paths(&write_paths);

    let args = serde_json::json!({
        "path": "src/lib.rs",
        "old_string": "fn old() {}",
        "new_string": "fn new() {}",
    });
    let result = EditFileTool.invoke(&ctx, &args).unwrap();

    assert!(!result.ok, "edit outside write_paths must be rejected");
    // The file on disk MUST NOT have changed.
    let body = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(body, "fn old() {}\n", "rejected edit must not touch disk");
}

// -------------------------------------------------------------------
// Future tests to add as anomalies get fixes:
//
// - critique-no-progress: feed two identical-blocker-count critique
//   responses in a row and assert the no-progress cap fires at the
//   threshold (max_critique_no_progress_iters), not at the absolute
//   critique-iter cap. Needs auto_opts to use Auto mode (not the
//   Manual mode the work-no-artifact test uses) and a fully-shaped
//   critique JSON via write_file in the MockAgent script. The
//   no-progress detector lives in auto.rs's critique-retry loop;
//   the absolute cap fires only after the no-progress cap if both
//   are configured.
//
// - bare-json-no-fence salvage path: pre-Phase-D fenced mode emitted
//   critique JSON as bare prose without a fenced wrapper. Salvage
//   recovered it; pin that the salvage path still works when a
//   future cleanup removes the fenced-blocks code path.
// -------------------------------------------------------------------
