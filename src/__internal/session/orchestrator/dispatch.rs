//! Session state machine.
//!
//! `run_session` drives a single work or critique session through
//! handshake -> instruction loading -> opening LLM turn -> turn loop
//! (LLM request / artifact write / user reply) -> session end. The
//! orchestrator never touches a real LLM: it asks the host via
//! `RequestLlmResponse` and waits for `LlmChunk` / `LlmEnd` events.
//!
//! Phase 9 M2 implements the basic chat-driven loop. Phase 9 M3 adds
//! the multi-phase iteration loop (author / build / test / coverage)
//! for code-authoring steps.

use std::sync::Arc;

use crate::client::SessionKind;
use crate::config::Config;
use crate::session::ask_user::{AskUserRuntime, mode_flip::read_current_step_mode};
use crate::session::auto::session_kind_to_protocol;
use crate::session::llm_adapter::LlmAdapter;
use crate::session::presenter::Presenter;
use crate::session::protocol::{
    DiagnosticLevel, Event, HostEvent, LlmMessage, LlmRole, PROTOCOL_VERSION, SessionEndReason,
    SessionKindOut, SessionTag, StepMode,
};
use crate::session::tools::{self};
use crate::state::State;
use crate::steps::registry_for;
use crate::{Error, Result};

use super::artifacts::{
    ExtractedArtifact, detect_framework_docs_root, detect_framework_root, detect_library_root,
    extract_artifacts, write_artifact,
};
use super::gates::{
    FindingKind, can_auto_wind_down_clean_work_session, effective_artifacts_empty,
    evaluate_structural_gate, line_kind, parse_blocker_lines, retry_gate_finding_blocks,
    salvage_critique_json,
};
use super::messages::{MessageBundle, build_initial_messages, step_descriptor_for_protocol};
use super::options::{LOOP_HINT_PREFIX, OrchestratorOptions, unix_seconds_now};
use super::progress::{
    ProgressClass, classify_progress, normalized_response_hash, should_emit_expectation_nudge,
};
use super::tools_dispatch::{
    base64_encode, invoke_tool, run_phase_validator, tool_args_summary, tool_call_persists_output,
};

pub fn run_session<P, L>(opts: OrchestratorOptions, presenter: &mut P, llm: &mut L) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    let log = crate::session::debug_log::DebugLog::open(&opts.project_dir);
    let mut wrapped = crate::session::debug_log::LoggingPresenter::new(presenter, &log);
    run_session_inner(opts, &mut wrapped, llm)
}

fn run_session_inner<P, L>(opts: OrchestratorOptions, host: &mut P, llm: &mut L) -> Result<()>
where
    P: Presenter + ?Sized,
    L: LlmAdapter + ?Sized,
{
    // Structured per-turn LLM metrics. Lives next to the debug log
    // (`.sim-flow/logs/llm-metrics.jsonl`); created lazily on the
    // first record so a session that never reaches an LLM dispatch
    // leaves no empty file.
    let llm_metrics = crate::session::llm_metrics::LlmMetricsLog::for_project(&opts.project_dir);

    // Per-tool-invocation wall-clock timings (LLM-driven tools). Same
    // lazy-open / best-effort shape as `LlmMetricsLog`. Captures the
    // `Event::ToolInvoked` payloads with `caller_kind = "llm"` so
    // reports can split out tool time from model-wait time per step.
    let tool_timings = crate::session::tool_timings::ToolTimingsLog::for_project(&opts.project_dir);

    // 1. Load state + step descriptor.
    let dot = opts.project_dir.join(".sim-flow");
    let state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry
        .get(&opts.step_id)
        .ok_or_else(|| {
            Error::InvalidStep(format!(
                "{} is not a {} step",
                opts.step_id,
                state.flow.as_str()
            ))
        })?
        .clone();
    // Config is not used in M2 (no client subprocess); load eagerly
    // to fail fast if `.sim-flow/config.toml` is malformed.
    let _config = Config::load(&dot)?;

    // 2. Handshake. Require a `Hello` first.
    let hello = match host.recv()? {
        Some(HostEvent::Hello {
            protocol_version, ..
        }) => protocol_version,
        Some(other) => {
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::ProtocolError,
                message: Some(format!("expected Hello, got {other:?}")),
            })?;
            return Err(Error::Protocol(format!(
                "expected Hello first, got {other:?}"
            )));
        }
        None => {
            return Err(Error::HostClosed("before Hello".into()));
        }
    };
    if hello != PROTOCOL_VERSION {
        host.send(&Event::SessionEnd {
            reason: SessionEndReason::ProtocolMismatch,
            message: Some(format!(
                "host sent protocolVersion={hello}; orchestrator speaks {PROTOCOL_VERSION}"
            )),
        })?;
        return Err(Error::ProtocolVersionMismatch {
            host: hello,
            orchestrator: PROTOCOL_VERSION.into(),
        });
    }

    // 3. Reply with HelloAck containing the step descriptor.
    let kind_out = match opts.kind {
        SessionKind::Work => SessionKindOut::Work,
        SessionKind::Critique => SessionKindOut::Critique,
    };
    let descriptor_out = step_descriptor_for_protocol(&step, kind_out, &opts.foundation_root);
    host.send(&Event::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        sim_flow_version: env!("CARGO_PKG_VERSION").into(),
        session: SessionTag {
            step: step.id.into(),
            kind: kind_out,
            candidate: opts.candidate.clone(),
        },
        step_descriptor: descriptor_out,
    })?;

    // 4a. Tool catalog (still needed in this scope so the turn loop's
    //     in-process tool dispatcher can run fenced tool calls). The
    //     library / framework root detectors stay here too because
    //     `invoke_tool` builds a `ToolContext` that references them.
    //
    //     The `ask_user` tool (Phase 5) needs a per-session
    //     `AskUserRuntime` to hold its pending-ask slot and thread
    //     registry. We construct one here and pass it through
    //     `build_dispatcher_with_runtime`; the orchestrator also
    //     keeps a clone so the suspend/resume cycle below can drive
    //     `resume_from_user_ask` directly. Recovery from a
    //     checkpoint runs eagerly so a reload mid-suspend restores
    //     the previous turn's pending state (Architecture §6.5.4).
    let ask_user_runtime: Arc<AskUserRuntime> = Arc::new(AskUserRuntime::new(
        opts.project_dir.clone(),
        step.id.to_string(),
    ));
    let _recovered_pending = ask_user_runtime.recover_from_checkpoint().ok().flatten();

    // Retrieval service powers `spec_semantic_search`,
    // `signal_table_query`, and `api_semantic_search`. Construction is
    // best-effort: when the project has no embedder config (or the
    // embedder construction fails — e.g. LM Studio isn't running) the
    // service is `None` and `build_dispatcher_with_runtime` silently
    // drops those three tools from the agent's catalog. The agent then
    // never sees the tool in its function list and treats it as
    // unavailable. Surface a Diagnostic on failure so the operator
    // knows why retrieval tools aren't available in this run, rather
    // than wondering why the agent is searching `lib:docs` for the
    // tool name instead of calling it.
    let retrieval = build_retrieval_service(&opts.project_dir, host)?;
    let dispatcher = tools::build_dispatcher_with_runtime(
        crate::steps::UNIVERSAL_TOOLS,
        retrieval.clone(),
        Some(ask_user_runtime.clone()),
    );
    // Tracks whether the auto→manual flip has already fired in this
    // session. The `ask_user` tool flips `state.toml` itself; the
    // orchestrator owns the matching `StepModeChanged` +
    // `Diagnostic::Info` emission (Architecture §6.5.2). One flip per
    // session: subsequent `ask_user` calls in the same run see the
    // already-manual mode and don't re-emit.
    let mut ask_user_mode_flip_emitted: bool = false;
    let library_root = detect_library_root(&opts.project_dir);
    let framework_root = detect_framework_root(&opts.foundation_root);
    let framework_docs_root = detect_framework_docs_root(&opts.foundation_root);

    // Pre-warm rust-analyzer for implementation-heavy steps so the
    // agent's first `api_search` / `api_hover` / etc. call doesn't
    // block 2-3 min on cold indexing. DM2d / DM3b / DM3c are the
    // steps where the agent reaches into the framework's public
    // API; earlier steps (DM0 / DM1 / DM2a-c / DM3a / DM4a) read
    // analysis docs and rarely need rust-analyzer.
    //
    // Best-effort: spawn fails silently with a tracing::warn (see
    // `lsp::prewarm`), so if the workspace can't be derived from
    // `framework_root` or the rust-analyzer binary is missing, the
    // session continues with whatever `api_*` calls eventually
    // pay the cold-start tax themselves.
    if matches!(step.id, "DM2d" | "DM3b" | "DM3c")
        && let Some(fr) = framework_root.as_deref()
        && let Some(workspace_root) = fr.parent().and_then(|p| p.parent())
    {
        let _ = crate::__internal::session::lsp::prewarm(workspace_root.to_path_buf());
    }
    let write_paths: Vec<String> = crate::steps::allowed_write_paths(&step, opts.kind);
    let work_retry_has_prior_blockers = opts.kind == SessionKind::Work
        && !retry_gate_finding_blocks(&opts.project_dir, step.id).is_empty();

    // The current milestone's project-relative path, when this is a
    // milestone-walk step. WriteFileTool reads the body fresh each
    // call to autocorrect paths -- if the agent writes `src/foo.rs`
    // but the milestone references `src/model/foo.rs`, the tool
    // moves the file and surfaces the redirect in the result.
    let current_milestone_path: Option<std::path::PathBuf> = step.milestone_walk.and_then(|walk| {
        let resolved = match &opts.milestone_name {
            // Pinned worker (parallel plan-detail dispatcher):
            // bypass the walker's first-pending / highest-touched
            // heuristics and scope to the exact stub this session
            // was assigned. `find_milestone_by_name` returns the
            // stub regardless of its pending state, so the orchestrator
            // still inlines the right file when a Critique runs after
            // its paired Work cleared the placeholder.
            Some(name) => {
                crate::__internal::steps::find_milestone_by_name(&opts.project_dir, &walk, name)
            }
            None => {
                // pick_touched is only meaningful for
                // find_current_milestone; computing it eagerly in
                // the Some(...) branch was dead.
                let pick_touched = match opts.kind {
                    SessionKind::Critique => true,
                    SessionKind::Work => work_retry_has_prior_blockers,
                };
                crate::__internal::steps::find_current_milestone(
                    &opts.project_dir,
                    &walk,
                    pick_touched,
                )
            }
        };
        match resolved {
            crate::__internal::steps::CurrentMilestone::File(rel) => {
                Some(opts.project_dir.join(rel))
            }
            _ => None,
        }
    });

    // 4b. Build the message stack + LLM-side tool descriptors via the
    //     shared helper so the interactive PTY driver can produce the
    //     exact same prompt without going through this loop.
    let MessageBundle {
        messages: mut_messages,
        tools: llm_tools,
    } = build_initial_messages(&opts, &step)?;
    let mut messages: Vec<LlmMessage> = mut_messages;

    // 5b. Phase pipeline. M3 implements `chat`, `author`, `build`,
    //     `test` (and treats `coverage` like `chat`). Phase order
    //     comes from the descriptor.
    let phases: &[&'static str] = match opts.kind {
        SessionKind::Work => step.work_phases,
        SessionKind::Critique => step.critique_phases,
    };
    let mut phase_idx: usize = 0;
    if let Some(p) = phases.first() {
        host.send(&Event::PhaseChanged { phase: (*p).into() })?;
    }
    let mut phase_iterations: u32 = 0;
    const MAX_ITER_PER_PHASE: u32 = 5;

    // Empty-response handling. Some models (notably vscode.lm-backed
    // Copilot) silently return zero text fragments when asked for a
    // large structured response. We surface that, retry once with a
    // direct nudge, and if the retry is still empty we hand control
    // back to the user with a clear notice rather than presenting an
    // empty bubble. Reset on every UserMessage so the budget is
    // per-user-turn, not per-session.
    let mut empty_response_retries: u32 = 0;
    const MAX_EMPTY_RETRIES: u32 = 1;

    // Auto-mode iteration counter. Increments each time we feed a
    // structural-gate failure list back to the agent without user
    // input. Capped by `opts.max_auto_iters`; the cap is per-session
    // (the cross-session critique loop has its own cap in the
    // CLI-side `auto` driver).
    let mut auto_iterations: u32 = 0;
    // Did THIS session persist any project outputs yet (artifact
    // block write, `write_file`, or `edit_file`)? This is
    // intentionally narrower than "any successful tool call":
    // read-only tools like `read_file` must NOT qualify a
    // critique-blocker retry for the no-artifact wind-down path.
    // Used by the per-milestone wind-down exit too so we don't
    // misfire when the agent's first turn is just reading inputs:
    // without this flag, a milestone-walk step where milestone-N-1
    // is already on disk (from a prior killed run) would see
    // `find_current_milestone(retry=true) != find_current_milestone(retry=false)`
    // on turn 1 and end the session prematurely.
    let mut session_persisted_writes: bool = false;

    // Backstop guards against runaway loops that don't trip the more
    // specific `max_auto_iters` / `max_critique_iters` caps:
    //
    //   - `total_llm_requests` enforces a hard ceiling on dispatched
    //     LLM calls in this session. New failure modes that retry in
    //     a way the existing caps don't see still get bounded.
    //   - `recent_response_hashes` tracks the last
    //     `opts.max_identical_responses` assistant responses. When
    //     the model returns the same bytes that many times in a row
    //     it's clearly stuck; we abort instead of paying for more.
    //   - `loop_hint_pending` is a one-strike-warning. When the
    //     deque has `cap - 1` identical entries (one more identical
    //     response would trip the abort), we prepend a "you've
    //     already sent this exact request; check the error before
    //     retrying" notice to the NEXT user message we build (tool
    //     results, build-output feedback, etc.). Saves the third
    //     turn when the agent would otherwise burn it on a fourth
    //     identical retry.
    let mut recent_response_hashes: std::collections::VecDeque<u64> =
        std::collections::VecDeque::with_capacity(opts.max_identical_responses.max(1) as usize);
    let mut loop_hint_pending: bool = false;

    // No-progress cap. The auto driver's existing `max_auto_iters`
    // counter measures turns that produced no artifact -- but a code
    // step that's iterating on `cargo test` fixes per turn DOES
    // produce artifacts (the rewritten test / source file) every
    // turn, so that counter never fires. The agent can spend
    // arbitrarily many turns chasing the same set of failures.
    //
    // This second counter watches `run_cargo` test outcomes
    // (populated by `RunCargoTool`) and counts CONSECUTIVE turns
    // where the failing-test count did NOT strictly decrease. A
    // strictly-decreasing count resets the counter (the agent is
    // making real progress, give it room to keep going). Hitting
    // `max_auto_iters` consecutive no-progress turns trips the same
    // flip-to-manual path the older cap uses (the diagnostic
    // intentionally includes `max_auto_iters` so the AutoHost
    // wrapper's existing substring match cancels the in-flight
    // sub-session cleanly).
    let mut last_test_failure_count: Option<usize> = None;
    let mut no_progress_iters: u32 = 0;

    // Pre-session manifest snapshot: paths the step has already
    // touched in prior work sessions / iterations. Used by the
    // no-progress classifier to distinguish a fix attempt
    // (modifies a path that's already step-owned) from a data
    // collection turn (only creates net-new paths -- e.g. a
    // diagnostic test file -- or only reads). A turn that
    // doesn't touch anything previously-owned gets a free pass on
    // the no-progress counter; the agent is investigating, not
    // failing to fix. The investigation-only streak is bounded
    // separately so the agent can't loop on diagnostics forever.
    let pre_session_manifest: std::collections::HashSet<std::path::PathBuf> =
        crate::manifest::step_paths(&opts.project_dir, step.id);
    // Target failing test set: the names of tests that were
    // failing the FIRST time `run_cargo test` reported failures
    // this session. Subsequent runs are scored against this
    // target -- the count alone hides "fixed A, broke B" 1-for-1
    // swaps, so we track names and check: (a) does the target
    // intersect-with-current shrink (progress)? (b) is there a
    // new failing test that's NOT in target (regression)?
    let mut target_failing_set: Option<std::collections::HashSet<String>> = None;
    // Investigation-only streak: cargo-test runs since the LAST
    // fix attempt where the agent didn't modify any pre-session-
    // known path SINCE THE PREVIOUS TEST. (Many agents -- vLLM
    // openai-compat with `qwen3.6` is the observed case -- emit one
    // tool call per LLM response, so an edit and a follow-up
    // `cargo test` land in separate turns. Per-turn check alone
    // misclassifies the test-only turn as investigation; we need
    // to remember whether a touch happened in EITHER a prior turn
    // since the last test, OR this turn.)
    let mut touched_existing_since_last_test: bool = false;
    let mut investigation_only_iters: u32 = 0;
    // Initial investigation budget. Bumped from 5 to 10 so the
    // agent has reasonable room to read framework docs / probe
    // behavior before declaring a fix; `declare_fix` resets this
    // counter so each declared attempt earns a fresh budget.
    const MAX_INVESTIGATION_ONLY_TURNS: u32 = 10;
    // Sticky flag set by the `declare_fix` tool. Reset (consumed)
    // when the next `cargo test` runs and the classifier scores
    // the turn. The agent's commit point: "the next test run is
    // my intentional fix attempt, score it accordingly even if
    // the file-op heuristic missed."
    let mut declared_fix_pending: bool = false;
    // Total `declare_fix` calls this session. Capped separately so
    // an agent that keeps declaring without progress still bails.
    let mut declared_fixes_count: u32 = 0;
    const MAX_DECLARED_FIXES: u32 = 8;

    // Test-expectation nudge: after several declared fixes that
    // haven't shrunk the target failing set, surface a one-time
    // diagnostic suggesting the agent reconsider whether the TEST
    // expectations are wrong (rather than the implementation).
    // This is option C from the classifier critique: give the agent
    // one explicit reframing before the declared-fix cap fires and
    // we switch to interactive. Fires at most once per session.
    let mut expectation_nudge_emitted: bool = false;
    const EXPECTATION_NUDGE_AFTER_FIXES: u32 = 4;

    // Tool-error-streak guard. Catches a failure mode the
    // identical-response check misses: the model keeps emitting
    // *slightly different* tool calls (different prose, different
    // reasoning, same broken `path:` or same allowlist-rejected
    // target) so the response-hash comparison sees variation but
    // no useful work happens. We count CONSECUTIVE turns where
    // every tool dispatch / artifact write the agent attempted
    // failed; a single success on either path resets the counter,
    // and a pure-chat turn (no tool / artifact actions) leaves it
    // unchanged so the agent isn't penalized for thinking.
    const MAX_CONSECUTIVE_TOOL_ERROR_TURNS: u32 = 5;
    let mut consecutive_tool_error_turns: u32 = 0;
    // Project-relative paths the user explicitly approved for
    // `delete_file` even though they sit outside the step's
    // `write_paths` allowlist. Populated by the scope-override
    // RequestUserInput flow below (interactive mode only --
    // `opts.auto` runs keep the silent-refuse behavior the user
    // explicitly chose). One-shot per path: a path stays approved
    // for the rest of the session so the very next delete_file
    // call from the agent can succeed, but a fresh out-of-scope
    // path triggers a new prompt.
    let mut approved_deletes: Vec<String> = Vec::new();

    // Cursor into `messages` tracking which entries have already been
    // surfaced to the host via `LlmRequest` events. Each turn, any
    // newly-appended non-Assistant message (User / Tool) is emitted
    // before dispatch so the chat UI can render the running prompt
    // stack alongside the assistant replies. System messages stay
    // off-wire -- they're constant per session and would be noisy.
    let mut last_emitted_message_index: usize = 0;

    // 5c. Turn loop.
    let mut turn_index: u32 = 0;
    loop {
        turn_index += 1;
        // Hard cap on total LLM requests. Hitting this aborts the
        // session before another paid call goes out.
        if opts.max_llm_requests > 0 && turn_index > opts.max_llm_requests {
            host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!(
                    "session aborted: hit max_llm_requests cap ({}) -- runaway-loop guard. \
                     Raise `--max-llm-requests` if your flow legitimately needs more turns; \
                     otherwise inspect the recent dispatch history for a stuck retry.",
                    opts.max_llm_requests,
                ),
            })?;
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::RunawayGuard,
                message: Some(format!(
                    "max_llm_requests cap ({}) reached after {} turns",
                    opts.max_llm_requests,
                    turn_index - 1
                )),
            })?;
            return Ok(());
        }
        // Tool-error-streak cap. See the declaration of
        // `consecutive_tool_error_turns` for the failure mode this
        // catches.
        if consecutive_tool_error_turns >= MAX_CONSECUTIVE_TOOL_ERROR_TURNS {
            host.send(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!(
                    "session aborted: agent burned {} consecutive turns where every tool / \
                     artifact-write attempt failed -- tool-error-streak guard. Inspect the \
                     recent ToolInvoked events; the agent is likely retrying the same broken \
                     call shape (wrong path, allowlist rejection, malformed args) without \
                     correcting it.",
                    consecutive_tool_error_turns,
                ),
            })?;
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::RunawayGuard,
                message: Some(format!(
                    "{} consecutive failed-tool turns",
                    consecutive_tool_error_turns
                )),
            })?;
            return Ok(());
        }
        let request_id = format!("lr-{turn_index}");
        // Per-turn wall-time tracking. The orchestrator measures
        // end-to-end round-trip including the dispatch and any
        // post-processing the adapter does (HTTP serialize, parse
        // response, etc.); the adapter-returned `LlmCallMetrics`
        // gives the inner-call timing if a host wants to compare.
        let turn_started = std::time::Instant::now();
        // Measured BEFORE the request goes out so a dispatch
        // failure can't bias the count. Sum of content bytes across
        // every message in the prompt; matches what the model
        // server's tokenizer sees byte-wise. Attachments and the
        // small JSON envelope per message are not counted.
        let prompt_bytes: u64 = messages.iter().map(|m| m.content.len() as u64).sum();

        // Native-mode tool catalog: when the orchestrator advertises
        // tools and the agent supports native function calls, we
        // route through `dispatch_with_tools` so the model sees the
        // structured catalog. Adapters that don't implement native
        // tools inherit the default impl which drops the catalog
        // and returns no tool calls -- same shape, just empty
        // `native_tool_calls` afterwards.
        let advertise: Vec<crate::session::agent::ToolAdvertise> = llm_tools
            .iter()
            .map(|t| crate::session::agent::ToolAdvertise {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.args_schema.clone(),
            })
            .collect();

        // Surface any non-Assistant messages that were appended to
        // the prompt stack since the previous dispatch. Typically:
        // System (session-opening system prompt), the initial user
        // message, or tool result messages added after the prior
        // assistant turn. The host is expected to render System
        // bubbles collapsed-by-default so the standing prompt
        // doesn't dominate the scroll. Assistant messages are
        // still skipped here because they were already surfaced
        // via `AssistantText` last turn.
        for (offset, msg) in messages[last_emitted_message_index..].iter().enumerate() {
            if matches!(msg.role, LlmRole::Assistant) {
                continue;
            }
            let idx = last_emitted_message_index + offset;
            host.send(&Event::LlmRequest {
                role: msg.role,
                content: msg.content.clone(),
                turn_index,
                request_id: request_id.clone(),
                message_id: Some(crate::session::compaction::position_id(idx)),
            })?;
        }
        last_emitted_message_index = messages.len();

        // Phase 1a deterministic compaction: scan the assembled
        // prompt stack for redundancy + apply stubs in place
        // before dispatch. Each evicted message's content shrinks
        // to a placeholder so the agent still sees that something
        // existed at that slot, but the LLM no longer carries the
        // original body forward. `Event::ContextEvicted` lets the
        // chat panel mark the matching transcript rows.
        //
        // Rule order matters: dedup runs first (replaces older
        // duplicate reads with stubs), then mutation invalidation
        // (replaces pre-write reads with stubs). Order doesn't
        // matter for correctness today since the rules target
        // different message subsets, but keeping dedup first
        // means a duplicate read of a path that's later mutated
        // gets the dedup stub, not the mutation stub -- which is
        // closer to "the live read is current" than "the live
        // read is also now stale", which it is, because anything
        // before the mutation isn't authoritative.
        {
            use crate::session::compaction::{
                dedup_reason, mutation_reason, position_id_index, position_pairs,
                run_mutation_invalidation, run_path_keyed_dedup,
            };
            // Dedup pass.
            let pairs = position_pairs(&messages);
            let report = run_path_keyed_dedup(&pairs);
            if !report.is_empty() {
                for (id, stub) in &report.stubs {
                    if let Some(idx) = position_id_index(id)
                        && idx < messages.len()
                    {
                        messages[idx].content = stub.clone();
                    }
                }
                host.send(&Event::ContextEvicted {
                    ids: report.dropped.clone(),
                    reason: dedup_reason(),
                })?;
            }
            // Mutation-invalidation pass. Recompute pairs because
            // dedup may have rewritten content, though it doesn't
            // affect this rule's matching (which keys on assistant
            // tool calls + path arg, not on the tool result body).
            let pairs = position_pairs(&messages);
            let report = run_mutation_invalidation(&pairs);
            if !report.is_empty() {
                for (id, stub) in &report.stubs {
                    if let Some(idx) = position_id_index(id)
                        && idx < messages.len()
                    {
                        messages[idx].content = stub.clone();
                    }
                }
                host.send(&Event::ContextEvicted {
                    ids: report.dropped.clone(),
                    reason: mutation_reason(),
                })?;
            }
        }

        // Stream the dispatch: forward each `StreamingChunk::Text`
        // as an `AssistantText { final_chunk: false }` event so the
        // chat panel can render tokens live. Buffer text into
        // `streamed_text` so callers that need the full body (the
        // existing post-turn artifact extraction below) see the
        // same string a non-streaming dispatch would have returned.
        // Backends that don't implement streaming fall back via the
        // trait default impl, which emits exactly one synthetic
        // chunk after the buffered dispatch -- semantically a no-op
        // for those callers.
        let mut streamed_text = String::new();
        let mut streamed_reasoning = String::new();
        let dispatch_result = {
            let host_for_chunks = &mut *host;
            let streamed_reasoning_for_chunks = &mut streamed_reasoning;
            let mut on_chunk = |chunk: crate::session::agent::StreamingChunk| match chunk {
                crate::session::agent::StreamingChunk::Text(t) => {
                    streamed_text.push_str(&t);
                    // Best-effort: if the host transport closes
                    // mid-stream we'll see the same error on the
                    // next event emit and surface it then; silently
                    // dropping here keeps the per-chunk callback
                    // infallible.
                    let _ = host_for_chunks.send(&Event::AssistantText {
                        text: t,
                        final_chunk: false,
                        tool_calls: Vec::new(),
                    });
                }
                crate::session::agent::StreamingChunk::Reasoning(t) => {
                    streamed_reasoning_for_chunks.push_str(&t);
                    let _ = host_for_chunks.send(&Event::AssistantReasoning {
                        text: t,
                        final_chunk: false,
                    });
                }
            };
            llm.dispatch_streaming(&messages, &advertise, &mut on_chunk)
        };

        // `Error::Cancelled` from the LLM adapter means the control
        // socket flipped the cancel flag while a dispatch was in
        // flight (subprocess SIGTERM'd, HTTP worker abandoned BEFORE
        // it could surface partial content). The semantic is
        // identical to a `HostEvent::Cancel` received at the next
        // `host.recv()` boundary: end the session cleanly with
        // `SessionEnd::Cancelled`. Doing so here -- before the
        // generic `agent-failed` error path -- avoids a misleading
        // "LLM error" diagnostic for what's actually a user-driven
        // cancel.
        if matches!(dispatch_result, Err(crate::Error::Cancelled)) {
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::Cancelled,
                message: Some("cancelled mid-dispatch via control socket".into()),
            })?;
            return Ok(());
        }

        let mut assistant_text = String::new();
        let mut native_tool_calls: Vec<crate::session::protocol::LlmToolCall> = Vec::new();
        let mut llm_failed = false;
        let mut llm_error_kind: Option<String> = None;
        let mut llm_error_message: Option<String> = None;
        // Streaming-cancel case: backend returned `metrics.cancelled
        // = true` with a partial response. Capture so the post-match
        // block can commit the partial turn and then end the session
        // cleanly with `SessionEnd::Cancelled` -- semantically the
        // same as the `Err(Error::Cancelled)` short-circuit above,
        // but preserving the streamed prose (and any tool calls that
        // arrived before the cancel) instead of discarding them.
        let mut dispatch_cancelled = false;
        match dispatch_result {
            Ok((text, calls, metrics)) => {
                // Per-call structured metrics so live tailing
                // (`RUST_LOG=sim_flow::metrics=info`) catches every
                // dispatch. Mirrors what TerminalHost used to log
                // pre-rewire so existing log-scraping tools don't
                // see a behavior change.
                tracing::info!(
                    target: "sim_flow::metrics",
                    event = "llm_call",
                    request_id = %request_id,
                    agent = %llm.name(),
                    tokens_in = ?metrics.tokens_in,
                    tokens_out = ?metrics.tokens_out,
                    wall_ms = metrics.wall_ms,
                    content_bytes = text.len(),
                    native_tool_calls = calls.len(),
                );
                assistant_text = text;
                native_tool_calls = calls
                    .into_iter()
                    .map(|c| crate::session::protocol::LlmToolCall {
                        id: c.id,
                        name: c.name,
                        arguments_json: c.arguments_json,
                    })
                    .collect();
                let llm_stop_reason: Option<String> = if native_tool_calls.is_empty() {
                    Some("stop".into())
                } else {
                    Some("tool_calls".into())
                };
                let llm_usage = match (metrics.tokens_in, metrics.tokens_out) {
                    (Some(p), Some(c)) => Some(crate::session::protocol::LlmUsage {
                        prompt_tokens: p,
                        completion_tokens: c,
                    }),
                    _ => None,
                };
                // Final-chunk close. The full prose was already
                // streamed via incremental `final_chunk: false`
                // events from the `on_chunk` callback above (or, for
                // backends that fall back to the buffered default
                // impl of `dispatch_streaming`, as a single synthetic
                // chunk after the dispatch returned). Either way the
                // panel has the complete body; this final event just
                // closes the turn and carries any native tool calls
                // the model emitted. We pass through whatever text
                // the backend reported as the final accumulated body
                // ONLY when streaming was bypassed -- if streamed
                // chunks already covered the same content, sending
                // it again would double the rendered text on
                // standard hosts.
                let final_text = if streamed_text == assistant_text {
                    String::new()
                } else {
                    // Backend skipped the streaming surface and
                    // returned text the chunks didn't cover -- emit
                    // the remainder so no content is lost.
                    let mut remainder = assistant_text.clone();
                    if let Some(stripped) = remainder.strip_prefix(streamed_text.as_str()) {
                        remainder = stripped.to_string();
                    }
                    remainder
                };
                // Close the reasoning bubble before the assistant
                // text bubble. Reasoning is streamed exclusively via
                // `StreamingChunk::Reasoning` (no return-tuple
                // component), so all the body has already been
                // forwarded as incremental `final_chunk: false`
                // events. This empty `final_chunk: true` flips the
                // panel's `streaming: false` so the collapsed
                // bubble's "thinking..." indicator clears. Emit only
                // when the turn actually produced reasoning so
                // turns from non-thinking backends stay quiet.
                if !streamed_reasoning.is_empty() {
                    host.send(&Event::AssistantReasoning {
                        text: String::new(),
                        final_chunk: true,
                    })?;
                }
                host.send(&Event::AssistantText {
                    text: final_text,
                    final_chunk: true,
                    tool_calls: native_tool_calls.clone(),
                })?;
                let turn_wall_ms = turn_started.elapsed().as_millis() as u64;
                // Native tool calls are part of the model's
                // completion, not separate plumbing -- their
                // serialized JSON is what the server actually
                // generated this turn. A `write_file`-only turn
                // has `assistant_text.len() == 0` but emitted
                // potentially thousands of bytes of
                // `arguments_json`; counting only the text
                // under-reports completion size by orders of
                // magnitude for tool-heavy turns. Include the
                // tool call payload (name + arguments + id) so
                // tokens_out matches the model's actual output.
                let tool_calls_bytes: u64 = native_tool_calls
                    .iter()
                    .map(|c| {
                        (c.id.as_deref().map(str::len).unwrap_or(0)
                            + c.name.len()
                            + c.arguments_json.len()) as u64
                    })
                    .sum();
                let completion_bytes = assistant_text.len() as u64 + tool_calls_bytes;
                tracing::info!(
                    target: "sim_flow::metrics",
                    event = "turn_end",
                    step = step.id,
                    kind = ?opts.kind,
                    request_id = %request_id,
                    turn_index,
                    assistant_bytes = assistant_text.len(),
                    tool_calls_bytes,
                    wall_ms = turn_wall_ms,
                );
                let mut metric = crate::session::llm_metrics::LlmMetricsRecord::from_byte_estimate(
                    unix_seconds_now(),
                    step.id,
                    session_kind_to_protocol(opts.kind),
                    &opts.llm_backend,
                    opts.llm_model.as_deref(),
                    &request_id,
                    turn_index,
                    turn_wall_ms,
                    llm_stop_reason.as_deref(),
                    prompt_bytes,
                    completion_bytes,
                );
                if let Some(u) = &llm_usage {
                    metric =
                        metric.with_exact_usage(u.prompt_tokens.into(), u.completion_tokens.into());
                }
                llm_metrics.record(&metric);
                dispatch_cancelled = metrics.cancelled;
            }
            Err(err) => {
                let kind = "agent-failed".to_string();
                let message = format!("{err}");
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!("LLM error ({kind}): {message}"),
                })?;
                llm_failed = true;
                llm_error_kind = Some(kind);
                llm_error_message = Some(message);
                let turn_wall_ms = turn_started.elapsed().as_millis() as u64;
                // On the error path no tool calls were received;
                // whatever (empty) `assistant_text` we have is the
                // partial completion, count only that.
                llm_metrics.record(
                    &crate::session::llm_metrics::LlmMetricsRecord::from_byte_estimate(
                        unix_seconds_now(),
                        step.id,
                        session_kind_to_protocol(opts.kind),
                        &opts.llm_backend,
                        opts.llm_model.as_deref(),
                        &request_id,
                        turn_index,
                        turn_wall_ms,
                        Some("error"),
                        prompt_bytes,
                        assistant_text.len() as u64,
                    ),
                );
            }
        }

        // Streaming-cancel short-circuit. The backend returned a
        // partial response with `metrics.cancelled = true`; the
        // chunks have already been streamed to the host and the
        // final AssistantText {final_chunk: true, ...} was sent
        // above. End the session cleanly so the orchestrator
        // doesn't process the partial output's tool calls / write
        // any artifacts from it -- the user explicitly cancelled
        // and we should not act on incomplete intent.
        if dispatch_cancelled {
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::Cancelled,
                message: Some(
                    "cancelled mid-dispatch via control socket; partial response preserved in transcript".into(),
                ),
            })?;
            return Ok(());
        }

        // Stuck-loop detection: hash a NORMALIZED version of each
        // non-empty response (digits replaced with `<N>` so
        // timestamps, byte counts, retry indices, etc. don't defeat
        // the comparison) and abort if the last
        // `max_identical_responses` hashes are all equal. Catches
        // "model keeps spewing the same error with shifting numbers"
        // cases the iteration caps don't see. The fixed-size deque
        // self-resets the streak: when a different hash arrives the
        // all-equal check fails until enough new identicals roll in.
        if !llm_failed && !assistant_text.trim().is_empty() && opts.max_identical_responses >= 2 {
            let cap = opts.max_identical_responses as usize;
            let h = normalized_response_hash(&assistant_text);
            recent_response_hashes.push_back(h);
            while recent_response_hashes.len() > cap {
                recent_response_hashes.pop_front();
            }
            let all_equal = recent_response_hashes.iter().all(|x| *x == h);
            if recent_response_hashes.len() == cap && all_equal {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "session aborted: agent produced {} structurally-identical responses in a row -- runaway-loop guard. \
                         The structural content (after stripping digits / timestamps) was the same; \
                         feeding it back another identical prompt is unlikely to help. \
                         Inspect `.sim-flow/logs/sim-flow-chat.log` for the recent transcript.",
                        cap,
                    ),
                })?;
                host.send(&Event::SessionEnd {
                    reason: SessionEndReason::RunawayGuard,
                    message: Some(format!("{} identical responses in a row", cap)),
                })?;
                return Ok(());
            }
            // Strike-(cap-1): one more identical response will trip
            // the abort above. Flag the next user message we build
            // so the agent gets one explicit chance to break the
            // cycle before we burn another turn. Only fires when
            // cap >= 3 so the strike-2 case isn't immediately
            // followed by an abort with no prior warning.
            if cap >= 3 && recent_response_hashes.len() + 1 == cap && all_equal {
                tracing::warn!(
                    streak = recent_response_hashes.len(),
                    cap,
                    "near-repeat detected; injecting loop-guard hint into next user message",
                );
                loop_hint_pending = true;
            } else if !all_equal {
                // Streak broken — either by a fresh response or by
                // a different one rolling into the deque. Drop any
                // pending hint so we don't carry it past recovery.
                loop_hint_pending = false;
            }
        }

        // Empty-response handling: detect zero-content responses,
        // surface a notice, retry once with a nudge before giving up.
        //
        // Critical caveat for native-tool-call mode: a turn that
        // returns native tool_calls but no text content is the
        // CORRECT shape (the model called tools instead of speaking).
        // Treating it as "empty" and re-prompting "Your response was
        // empty" confused the model after every successful tool turn
        // -- Phase D K=3 measurement saw `empty-response` median=16
        // events per trial, all of them paired with non-empty
        // tool_calls. Skip the empty handling when this turn produced
        // any native tool calls; the dispatch loop below will run
        // them and feed back the Tool-role results as normal.
        if !llm_failed && assistant_text.trim().is_empty() && native_tool_calls.is_empty() {
            if empty_response_retries < MAX_EMPTY_RETRIES {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Warning,
                    message: "LLM returned no content. Retrying once with an explicit nudge."
                        .into(),
                })?;
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: "Your previous response was empty. Produce your answer now \
                              as plain text or as a fenced artifact-write block per the \
                              instructions above. Do not return an empty response."
                        .into(),
                    attachments: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    reasoning: None,
                });
                empty_response_retries += 1;
                continue;
            } else {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Warning,
                    message: "LLM returned no content twice in a row. Pausing for your input \
                              - try rephrasing or running /step again."
                        .into(),
                })?;
                empty_response_retries = 0;
                // Fall through to RequestUserInput below; skip post-processing.
            }
        } else if !llm_failed {
            empty_response_retries = 0;
        }

        // Skip post-processing only when there was nothing actionable
        // -- LLM failed OR (empty text AND no native tool calls).
        // A turn with native tool_calls but empty text is the normal
        // tool-call-only shape; we MUST run post-processing so the
        // dispatcher invokes them and pushes the results back.
        if llm_failed || (assistant_text.trim().is_empty() && native_tool_calls.is_empty()) {
            // Skip post-processing on this turn; ask user for input.
        } else {
            // Echo the assistant's native tool_calls back into history
            // so the next outbound request shows the model its own
            // prior call requests. OpenAI's spec requires this for
            // the next-turn tool-result messages to bind correctly.
            //
            // `reasoning` carries the model's thinking text captured
            // from `delta.reasoning_content` during this turn. Backends
            // that thread reasoning back on the wire (openai_compat
            // with `--reasoning-parser qwen3`) replay it as the
            // assistant message's `reasoning_content` field on the
            // NEXT request so the model has continuity of thought.
            // Backends that flatten messages drop it.
            let assistant_reasoning = if streamed_reasoning.is_empty() {
                None
            } else {
                Some(streamed_reasoning.clone())
            };
            messages.push(LlmMessage {
                role: LlmRole::Assistant,
                content: assistant_text.clone(),
                attachments: Vec::new(),
                tool_call_id: None,
                tool_calls: native_tool_calls.clone(),
                reasoning: assistant_reasoning,
            });

            // 5d. Extract artifacts and write them.
            let mut artifacts = extract_artifacts(&assistant_text);

            // Critique-session fallback: when the agent emits the
            // critique body inline (markdown prose / tables / lists)
            // instead of wrapping it in a fenced ` ```<path>` block as
            // the artifact-write convention requires, the extractor
            // sees nothing and the auto driver loops until the cap
            // fires -- even when the critique itself is fine
            // ("UNRESOLVED items only, no BLOCKERs"). The legacy
            // markdown form lets us recover by saving the prose body
            // when it carries `BLOCKER:` / `UNRESOLVED:` /
            // `RESOLVED:` line markers; the new JSON form makes that
            // recovery less common because the agent typically calls
            // `write_file` natively, but we keep the marker-based
            // path as a safety net for projects mid-migration. We
            // only apply this when the turn also produced no tool
            // calls -- a turn that's purely `read_file` calls is the
            // agent gathering input, not emitting the critique.
            //
            // Crucial pre-check: if the canonical critique file is
            // already on disk for this step, the agent's job is
            // done. The current turn is then just a prose summary
            // of a tool-driven write that landed in a PRIOR turn
            // (typical with native function-calling backends:
            // turn N writes via `write_file`, turn N+1 says "the
            // critique JSON is written at <path>"). Treating that
            // follow-up as "no findings, ghost-pass risk" and
            // re-prompting is wrong -- the file exists and the
            // session should end normally.
            let mut pre_check_tool_calls = tools::extract_tool_calls(&assistant_text);
            for call in &native_tool_calls {
                pre_check_tool_calls.push(tools::ParsedToolCall {
                    name: call.name.clone(),
                    body: call.arguments_json.clone(),
                });
            }
            let critique_already_on_disk = opts.kind == SessionKind::Critique
                && (opts
                    .project_dir
                    .join(format!("docs/critiques/{}-critique.json", step.id))
                    .exists()
                    || opts
                        .project_dir
                        .join(format!("docs/critiques/{}-critique.md", step.id))
                        .exists());
            if artifacts.is_empty()
                && opts.kind == SessionKind::Critique
                && pre_check_tool_calls.is_empty()
                && !assistant_text.trim().is_empty()
                && !critique_already_on_disk
            {
                // Auto-save the response as the critique IFF it
                // contains at least one structured finding marker
                // (BLOCKER / RESOLVED / UNRESOLVED). A pure-prose
                // response with no markers ("everything looks fine,
                // nothing to flag.") would otherwise be saved as
                // an empty critique that the gate parses as zero
                // blockers and waves through -- exactly the
                // ghost-pass mode we don't want. Reject those and
                // feed the agent back a corrective User turn.
                let has_findings = parse_blocker_lines(&assistant_text).len()
                    + assistant_text
                        .lines()
                        .filter(|l| {
                            matches!(
                                line_kind(l),
                                Some(FindingKind::Resolved | FindingKind::Unresolved)
                            )
                        })
                        .count()
                    > 0;
                if has_findings {
                    let path = format!("docs/critiques/{}-critique.md", step.id);
                    host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Warning,
                        message: format!(
                            "{}: critique response had no fenced artifact-write block; \
                             saving the full response body as `{}`. The agent ignored \
                             the artifact-write convention -- tighten the critique \
                             system prompt if this is recurrent.",
                            step.id, path,
                        ),
                    })?;
                    artifacts.push(ExtractedArtifact {
                        relative_path: path,
                        content: assistant_text.clone(),
                    });
                } else if let Some(salvaged) = salvage_critique_json(&assistant_text, step.id) {
                    // Salvage: the agent emitted a structurally
                    // valid critique JSON but with a wrong fence
                    // (e.g. ```json instead of
                    // ```docs/critiques/DM0-critique.json) or as
                    // bare prose surrounding a JSON literal. Save
                    // it directly rather than asking the agent to
                    // retry -- a retry burns LLM tokens to
                    // reproduce content we already have.
                    //
                    // Level depends on the model family: families
                    // known to *routinely* emit bare-JSON critiques
                    // (qwen3_6, gemma4, kimi_vl_thinking) get an
                    // Info diagnostic -- it's the expected path,
                    // and a per-critique Warning floods the chat
                    // panel with yellow banners for behavior we
                    // already adapt to. Families that should be
                    // emitting fenced blocks (claude_messages,
                    // generic_chat) still get Warning so a
                    // regression is loud.
                    let family = crate::session::agent::resolve_model_family(
                        opts.llm_model_family_id.as_deref(),
                        opts.llm_model.as_deref(),
                    );
                    let path = format!("docs/critiques/{}-critique.json", step.id);
                    let (level, message) = if family.prefers_bare_json_critique {
                        (
                            DiagnosticLevel::Info,
                            format!(
                                "{}: salvaged critique JSON from bare prose -> `{path}` \
                                 (expected for the `{}` model family).",
                                step.id, family.id,
                            ),
                        )
                    } else {
                        (
                            DiagnosticLevel::Warning,
                            format!(
                                "{}: salvaged critique JSON from a non-fenced response and \
                                 saved to `{path}`. Tighten the critique system prompt if \
                                 this is recurrent (the agent should emit the JSON inside a \
                                 fenced block whose info-string is the path).",
                                step.id,
                            ),
                        )
                    };
                    host.send(&Event::Diagnostic { level, message })?;
                    artifacts.push(ExtractedArtifact {
                        relative_path: path,
                        content: salvaged,
                    });
                } else {
                    // No fenced block AND no findings AND no critique
                    // already on disk AND no salvageable JSON:
                    // refuse to commit. Push a corrective User turn
                    // so the agent retries with proper structure
                    // rather than the gate clearing on a no-op.
                    //
                    // Diagnostic level is Warning, not Error: this
                    // is a recovery / retry path, not a
                    // session-fatal error. The chat panel
                    // classifier reads `**Error**:` as "Session
                    // error" (error tone, scary banner) which
                    // overstates a routine re-prompt.
                    let json_path = format!("docs/critiques/{}-critique.json", step.id);
                    host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Warning,
                        message: format!(
                            "{}: critique response produced no critique file (no `write_file` \
                             call, no fenced artifact-write block, no `BLOCKER:` / \
                             `RESOLVED:` / `UNRESOLVED:` finding lines) and `{}` is not on \
                             disk. Re-prompting the agent for a properly fenced critique.",
                            step.id, json_path,
                        ),
                    })?;
                    messages.push(LlmMessage {
                        role: LlmRole::User,
                        content: format!(
                            "Your previous response did not produce a critique file. The \
                             expected output is `{json_path}` (canonical JSON form -- \
                             schema in the system instructions: `step`, `summary`, \
                             `findings[]` with `kind` in `blocker`/`unresolved`/`resolved`, \
                             optional `notes`). The orchestrator will render the markdown \
                             sibling automatically; do not write the markdown yourself. \
                             Emit the critique now -- preferably via the `write_file` \
                             tool, or as a fenced artifact-write block whose info-string \
                             is `{json_path}`."
                        ),
                        attachments: Vec::new(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        reasoning: None,
                    });
                    continue;
                }
            }
            // Critique session, no artifacts this turn, no tool calls,
            // text on the wire, and the canonical critique JSON IS
            // already on disk: the agent finished writing the critique
            // in a prior turn and is now emitting a prose summary. End
            // the session so the auto driver moves on to gate
            // evaluation; otherwise we fall through to the auto pump /
            // RequestUserInput tail (`effective_artifacts_empty`
            // returns false for critique+text+no-tools by design,
            // which keeps the wind-down branch closed), and an auto
            // run parks waiting for a user reply that will never come.
            // The matching comment at the top of the salvage block
            // ("the file exists and the session should end normally")
            // describes the intent; this is the missing exit.
            if opts.auto
                && opts.kind == SessionKind::Critique
                && artifacts.is_empty()
                && pre_check_tool_calls.is_empty()
                && !assistant_text.trim().is_empty()
                && critique_already_on_disk
            {
                host.send(&Event::SessionEnd {
                    reason: SessionEndReason::Completed,
                    message: Some(format!(
                        "auto: {} critique already on disk; ending after prose-summary turn",
                        step.id
                    )),
                })?;
                return Ok(());
            }
            // Per-turn touched-paths set, populated from BOTH the
            // artifact-extract path below AND the tool-call dispatch
            // further down. Used by the no-progress classifier to
            // tell a fix attempt (modified an existing path from the
            // step's manifest snapshot) from a data-collection turn
            // (only added new files / only read). Declared here so
            // it's in scope across both write surfaces.
            let mut this_turn_touched_paths: std::collections::HashSet<std::path::PathBuf> =
                std::collections::HashSet::new();
            // Track artifact-write failures so we can feed them back
            // as a User turn below. Without this, a rejected write
            // surfaces as a host-side `Diagnostic` only; the agent's
            // turn ends believing the write succeeded, the validator
            // / gate then fails because the file isn't on disk, and
            // the agent has to reverse-engineer what went wrong from
            // missing-file errors. Threading the rejection back is
            // the same pattern the tool-call dispatcher already uses
            // for tool errors: visible failure → next-turn correction.
            let mut artifact_write_failures: Vec<(String, String)> = Vec::new();
            let mut artifact_write_successes: u32 = 0;
            for art in &artifacts {
                let started_unix = crate::session::tool_timings::now_unix_seconds();
                let started = std::time::Instant::now();
                let outcome = write_artifact(&opts.project_dir, &write_paths, art);
                let wall_ms = started.elapsed().as_millis() as u64;
                let (status, detail_for_failures) = match &outcome {
                    Ok(_) => ("ok", None),
                    Err(err) => ("error", Some(format!("{err}"))),
                };
                tool_timings.record(&crate::session::tool_timings::ToolTimingRecord {
                    started_unix,
                    step: Some(step.id.to_string()),
                    caller_kind: crate::session::tool_timings::CallerKind::Llm,
                    tool_name: "write_file".to_string(),
                    args_summary: art.relative_path.clone(),
                    status: status.to_string(),
                    wall_ms,
                    exit_code: None,
                    request_id: None,
                    turn_index: None,
                });
                match outcome {
                    Ok(bytes) => {
                        artifact_write_successes += 1;
                        crate::manifest::record_write(
                            &opts.project_dir,
                            step.id,
                            &art.relative_path,
                        );
                        this_turn_touched_paths.insert(opts.project_dir.join(&art.relative_path));
                        host.send(&Event::ArtifactWritten {
                            path: art.relative_path.clone(),
                            bytes,
                        })?;
                        host.send(&Event::ToolInvoked {
                            name: "write_file".into(),
                            args_summary: art.relative_path.clone(),
                            status: "ok".into(),
                            duration_ms: wall_ms,
                        })?;
                    }
                    Err(_) => {
                        let detail = detail_for_failures
                            .expect("status='error' implies detail_for_failures is Some");
                        host.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Error,
                            message: format!("failed to write {}: {detail}", art.relative_path),
                        })?;
                        host.send(&Event::ToolInvoked {
                            name: "write_file".into(),
                            args_summary: art.relative_path.clone(),
                            status: "error".into(),
                            duration_ms: wall_ms,
                        })?;
                        artifact_write_failures.push((art.relative_path.clone(), detail));
                    }
                }
            }
            // Tool-error-streak: clean turn (>=1 artifact written,
            // no failures) is real progress -- reset the counter.
            // Mixed-or-all-failed cases are handled in the
            // failure branch below.
            if artifact_write_successes > 0 && artifact_write_failures.is_empty() {
                consecutive_tool_error_turns = 0;
            }
            if artifact_write_successes > 0 {
                session_persisted_writes = true;
            }
            if !artifact_write_failures.is_empty() {
                // Any-failed turn with no successes -> bump streak;
                // mixed turn (some succeeded) -> reset.
                if artifact_write_successes == 0 {
                    consecutive_tool_error_turns += 1;
                } else {
                    consecutive_tool_error_turns = 0;
                }
                // Feed rejections back as the next User turn so the
                // agent can correct (e.g. retarget a path, drop a
                // disallowed write) instead of marching into the
                // validator / gate phase under false assumptions.
                // We `continue` BEFORE running tool calls and
                // validators so the next turn starts from the
                // failure-aware message stack.
                let mut feedback = String::new();
                if loop_hint_pending {
                    feedback.push_str(LOOP_HINT_PREFIX);
                    loop_hint_pending = false;
                }
                feedback.push_str(
                    "Artifact-write rejections (the orchestrator did NOT persist these files; \
                     re-emit the affected blocks at allowed paths or surface why a wider \
                     write scope is needed):\n\n",
                );
                for (path, detail) in &artifact_write_failures {
                    feedback.push_str(&format!("- `{path}`: {detail}\n"));
                }
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: feedback,
                    attachments: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    reasoning: None,
                });
                continue;
            }

            // 5e. Extract + dispatch tool calls. The model can emit
            //     them two ways:
            //     - Native: tool_calls arrived on the matching
            //       HostEvent::LlmEnd (Phase B+ of the native-tool-
            //       calls migration; gated on SIM_FLOW_TOOL_MODE=native
            //       at the host). Their replies push as Tool-role
            //       messages tied to the call id.
            //     - Fenced: the legacy ```tool:<name> /
            //       ```json {"name":...} convention extracted from
            //       the assistant text body. Their replies bundle
            //       into one User-role feedback message (no per-call
            //       id binding).
            //     Each form dispatches through the same
            //     `ParsedToolCall` / `invoke_tool` machinery; only
            //     the conversation-history shape differs at the end.
            //
            //     `native_dispatch_count` lets the existing
            //     non-empty checks below treat "tool calls happened
            //     this turn" without re-collecting both lists.
            let fenced_tool_calls = tools::extract_tool_calls(&assistant_text);
            let mut tool_calls: Vec<tools::ParsedToolCall> = native_tool_calls
                .iter()
                .map(|c| tools::ParsedToolCall {
                    name: c.name.clone(),
                    body: c.arguments_json.clone(),
                })
                .collect();
            let native_dispatch_count = tool_calls.len();
            tool_calls.extend(fenced_tool_calls);
            if !tool_calls.is_empty() {
                let mut tool_attachments: Vec<crate::session::protocol::LlmAttachment> = Vec::new();
                // Take the LAST test_failure_count seen this turn.
                // Multiple `run_cargo test` calls in one turn (e.g.
                // the agent re-runs after a fix) collapse to the
                // most recent measurement.
                let mut this_turn_test_count: Option<usize> = None;
                // Parallel to `this_turn_test_count`: the set of
                // failing test names from the LAST `run_cargo test`
                // this turn. Drives the no-progress classifier's
                // "did the target failing set shrink?" check; a raw
                // count alone hides the "fixed A, broke B" 1-for-1
                // swap (count constant, but set changed).
                let mut this_turn_test_failures: Option<Vec<String>> = None;
                let mut tool_successes: u32 = 0;
                let mut tool_failures: u32 = 0;
                let mut this_turn_persisted_write: bool = false;
                // Per-call outcomes, in dispatch order. Native calls
                // (indices < native_dispatch_count) get one
                // Tool-role message each tied to the matching
                // native_tool_calls[i].id. Fenced calls bundle into
                // a User-role feedback message.
                let mut per_call_displays: Vec<String> = Vec::with_capacity(tool_calls.len());
                // When `ask_user` suspends the turn, capture the index
                // of the suspending call here and exit the dispatch
                // loop. Calls AFTER this index are discarded with a
                // `tool_calls_after_ask_user` diagnostic per
                // Architecture §6.5.1. The suspension itself is
                // handled below: RequestUserInput, wait for
                // UserMessage, push Tool-role result, restart the
                // outer turn loop without going through the normal
                // tool-result feedback bundling for the suspended
                // call (its result is the user's reply, not the
                // tool's display string).
                let mut suspended_call: Option<(
                    usize,
                    crate::__internal::session::ask_user::SuspendOutcome,
                )> = None;
                let mut discarded_after_ask_user: u32 = 0;
                for (call_idx, call) in tool_calls.iter().enumerate() {
                    if suspended_call.is_some() {
                        // Architecture §6.5.1: discard subsequent
                        // tool calls in the same model response so
                        // the suspending `ask_user` is effectively
                        // the LAST call of the turn.
                        discarded_after_ask_user += 1;
                        continue;
                    }
                    let _ = call_idx;
                    let started_unix = crate::session::tool_timings::now_unix_seconds();
                    let started = std::time::Instant::now();
                    // Read the current milestone's body fresh for
                    // each call -- the agent may have just edited it.
                    let milestone_body = current_milestone_path
                        .as_deref()
                        .and_then(|p| std::fs::read_to_string(p).ok());
                    // Project-relative form of the milestone path,
                    // for `log_bug` to record which milestone
                    // surfaced the bug.
                    let milestone_rel: Option<String> = current_milestone_path
                        .as_deref()
                        .and_then(|p| p.strip_prefix(&opts.project_dir).ok())
                        .map(|p| p.display().to_string());
                    let ctx = tools::ToolContext::new(
                        &opts.project_dir,
                        library_root.as_deref(),
                        framework_root.as_deref(),
                        framework_docs_root.as_deref(),
                    )
                    .with_write_paths(&write_paths)
                    .with_milestone_body(milestone_body.as_deref())
                    .with_milestone_path(milestone_rel.as_deref())
                    .with_approved_deletes(&approved_deletes)
                    .with_step_id(step.id);
                    // Snapshot the step-mode before dispatch so we
                    // can detect the `ask_user` auto→manual flip
                    // (Architecture §6.5.2). The `ask_user` tool
                    // performs the flip itself via `state.toml`;
                    // the orchestrator owns the matching
                    // `StepModeChanged` + `Diagnostic::Info`
                    // emission. We snapshot for ALL calls to keep
                    // the branchy logic simple; only an actual
                    // change drives the emit.
                    let mode_before = read_current_step_mode(&opts.project_dir);
                    let outcome = invoke_tool(&dispatcher, &ctx, call);
                    // Detect suspend BEFORE the success/failure
                    // counters: a suspended ask_user is neither a
                    // success nor a failure in the streak sense --
                    // its outcome depends on what the user types.
                    if let Some(susp) = outcome.suspend.as_ref() {
                        // Auto→manual flip side-effects per
                        // Architecture §6.5.2. The tool already
                        // wrote `state.toml`; emit the user-
                        // visible events here exactly once per
                        // session.
                        let mode_after = read_current_step_mode(&opts.project_dir);
                        if mode_before == StepMode::Auto
                            && mode_after == StepMode::Manual
                            && !ask_user_mode_flip_emitted
                        {
                            host.send(&Event::StepModeChanged {
                                mode: StepMode::Manual,
                            })?;
                            host.send(&Event::Diagnostic {
                                level: DiagnosticLevel::Info,
                                message:
                                    "ask_user invoked during auto run; flipping to manual mode. \
                                    Re-enable auto via the chat panel toggle when ready."
                                        .into(),
                            })?;
                            ask_user_mode_flip_emitted = true;
                        }
                        // Tool dispatch event for visibility. The
                        // `display` is empty (suspended); the
                        // status string captures the suspension so
                        // hosts can render a "waiting for user"
                        // affordance if they want.
                        let wall_ms = started.elapsed().as_millis() as u64;
                        let args_summary = tool_args_summary(call);
                        tool_timings.record(&crate::session::tool_timings::ToolTimingRecord {
                            started_unix,
                            step: Some(step.id.to_string()),
                            caller_kind: crate::session::tool_timings::CallerKind::Llm,
                            tool_name: call.name.clone(),
                            args_summary: args_summary.clone(),
                            status: "suspended".to_string(),
                            wall_ms,
                            exit_code: None,
                            request_id: None,
                            turn_index: None,
                        });
                        host.send(&Event::ToolInvoked {
                            name: call.name.clone(),
                            args_summary,
                            status: "suspended".into(),
                            duration_ms: wall_ms,
                        })?;
                        per_call_displays.push(String::new());
                        suspended_call = Some((call_idx, susp.clone()));
                        continue;
                    }
                    let status = if outcome.ok { "ok" } else { "error" };
                    if outcome.ok {
                        tool_successes += 1;
                        if tool_call_persists_output(&call.name) {
                            this_turn_persisted_write = true;
                        }
                    } else {
                        tool_failures += 1;
                    }
                    let wall_ms = started.elapsed().as_millis() as u64;
                    let args_summary = tool_args_summary(call);
                    tool_timings.record(&crate::session::tool_timings::ToolTimingRecord {
                        started_unix,
                        step: Some(step.id.to_string()),
                        caller_kind: crate::session::tool_timings::CallerKind::Llm,
                        tool_name: call.name.clone(),
                        args_summary: args_summary.clone(),
                        status: status.to_string(),
                        wall_ms,
                        exit_code: None,
                        request_id: None,
                        turn_index: None,
                    });
                    host.send(&Event::ToolInvoked {
                        name: call.name.clone(),
                        args_summary,
                        status: status.into(),
                        duration_ms: wall_ms,
                    })?;
                    per_call_displays.push(outcome.display.clone());
                    if let Some(c) = outcome.test_failure_count {
                        this_turn_test_count = Some(c);
                    }
                    if let Some(names) = &outcome.test_failures {
                        this_turn_test_failures = Some(names.clone());
                    }
                    for rel in &outcome.touched_paths {
                        this_turn_touched_paths.insert(opts.project_dir.join(rel));
                    }
                    if outcome.declared_fix {
                        declared_fix_pending = true;
                        declared_fixes_count = declared_fixes_count.saturating_add(1);
                    }
                    for att in outcome.attachments {
                        tool_attachments.push(crate::session::protocol::LlmAttachment {
                            mime: att.mime,
                            data: base64_encode(&att.bytes),
                            source: Some(att.source_path),
                        });
                    }
                }
                // `ask_user` suspend handling (Architecture §6.5 +
                // §6.5.1). When the dispatch loop detected a
                // suspending call, we:
                //
                //   1. Warn about any calls discarded after it (per
                //      §6.5.1 "subsequent calls in the same model
                //      response are discarded with a
                //      tool_calls_after_ask_user warning").
                //   2. Emit `RequestUserInput` derived from the
                //      pending ask so the host surfaces the
                //      question, with a `kind`-appropriate
                //      placeholder.
                //   3. Wait for the next `UserMessage` (or `Cancel`
                //      per existing patterns).
                //   4. Call `runtime.resume_from_user_ask(...)` and
                //      push a Tool-role message whose content is
                //      the serialized `AskUserAnswer` JSON keyed
                //      by the suspending call's `tool_call_id` so
                //      the next LLM turn sees the answer as the
                //      synthetic tool-result of the suspended
                //      `ask_user` call (Architecture §6.5.1 final
                //      bullet).
                //   5. Restart the turn loop. We skip the normal
                //      tool-result feedback bundling for this turn
                //      because the suspended call's "result" is the
                //      AskUserAnswer JSON we just pushed, not the
                //      tool's empty display string.
                if let Some((suspended_idx, suspend_outcome)) = suspended_call.take() {
                    if discarded_after_ask_user > 0 {
                        tracing::warn!(
                            target: "sim_flow::ask_user",
                            event = "tool_calls_after_ask_user",
                            step = step.id,
                            discarded_calls = discarded_after_ask_user,
                            "discarded {discarded_after_ask_user} tool call(s) after ask_user; \
                             ask_user must be the LAST tool call of the turn"
                        );
                        host.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Warning,
                            message: format!(
                                "tool_calls_after_ask_user: discarded {discarded_after_ask_user} \
                                 tool call(s) emitted after `ask_user` in the same turn. \
                                 `ask_user` must be the LAST tool call of a turn (Architecture §6.5.1)."
                            ),
                        })?;
                    }
                    let pending = &suspend_outcome.pending;
                    let placeholder = ask_user_placeholder(pending);
                    let prompt = build_ask_user_prompt(pending);
                    host.send(&Event::RequestUserInput {
                        prompt: Some(prompt),
                        placeholder: Some(placeholder),
                    })?;
                    let (reply_text, cancelled, thread_cancelled) = match host.recv()? {
                        Some(HostEvent::UserMessage { text }) => {
                            let trimmed = text.trim();
                            if trimmed == "/cancel-thread" {
                                (String::new(), true, true)
                            } else if trimmed == "/cancel" {
                                (String::new(), true, false)
                            } else {
                                (text, false, false)
                            }
                        }
                        Some(HostEvent::Cancel) | None => {
                            host.send(&Event::SessionEnd {
                                reason: SessionEndReason::Cancelled,
                                message: None,
                            })?;
                            return Ok(());
                        }
                        Some(other) => {
                            host.send(&Event::Diagnostic {
                                level: DiagnosticLevel::Warning,
                                message: format!(
                                    "unexpected host event during ask_user suspend: {other:?}; \
                                     treating as cancel-thread"
                                ),
                            })?;
                            (String::new(), true, true)
                        }
                    };
                    let answer = ask_user_runtime
                        .resume_from_user_ask(&reply_text, cancelled, thread_cancelled)
                        .map_err(|e| Error::Protocol(format!("ask_user resume failed: {e}")))?;
                    let answer_json = serde_json::to_string(&answer).unwrap_or_else(|_| {
                        // Serialization of `AskUserAnswer` is
                        // structurally infallible (all fields are
                        // primitives / Strings) but the fallback
                        // string keeps the agent unblocked even on
                        // an OOM-style failure.
                        format!(
                            "{{\"answer\":\"{}\",\"thread_id\":\"{}\"}}",
                            answer.answer.replace('"', "\\\""),
                            answer.thread_id,
                        )
                    });
                    // Bind the suspended ParsedToolCall's index back
                    // to the originating native_tool_call to recover
                    // the LLM's tool_call_id. Only native calls have
                    // ids; a fenced ask_user call (rare in practice)
                    // gets an empty tool_call_id which the openai-
                    // compat converter flattens to a User-role line.
                    let suspended_tool_call_id = if suspended_idx < native_tool_calls.len() {
                        native_tool_calls[suspended_idx].id.clone()
                    } else {
                        None
                    };
                    messages.push(LlmMessage {
                        role: LlmRole::Tool,
                        content: answer_json,
                        attachments: Vec::new(),
                        tool_call_id: suspended_tool_call_id,
                        tool_calls: Vec::new(),
                        reasoning: None,
                    });
                    // The remaining tool-result emission (Tool-role
                    // messages for prior native calls, fenced
                    // bundling) still has to fire for calls that
                    // succeeded BEFORE the suspension. The for loop
                    // collected their per_call_displays already.
                    // Cap per_call_displays so the existing emit
                    // loop only sees pre-suspend entries.
                    per_call_displays.truncate(suspended_idx);
                    for (i, native) in native_tool_calls.iter().enumerate() {
                        if i >= per_call_displays.len() {
                            break;
                        }
                        messages.push(LlmMessage {
                            role: LlmRole::Tool,
                            content: per_call_displays[i].clone(),
                            attachments: Vec::new(),
                            tool_call_id: native.id.clone(),
                            tool_calls: Vec::new(),
                            reasoning: None,
                        });
                    }
                    // No fenced-bundle / no scope-override / no
                    // no-progress classifier: ask_user is a turn-
                    // boundary tool, and the next LLM turn starts
                    // fresh with the answer in its working memory.
                    continue;
                }
                // Tool-error-streak tracking. All-failed turn -> bump
                // streak; any-succeeded turn -> reset.
                if tool_failures > 0 && tool_successes == 0 {
                    consecutive_tool_error_turns += 1;
                } else if tool_successes > 0 {
                    consecutive_tool_error_turns = 0;
                }
                if this_turn_persisted_write {
                    session_persisted_writes = true;
                }
                // Emit Tool-role messages for native calls (one per
                // call, tied to the originating tool_call_id). The
                // openai_compat converter serializes these as
                // `{role: "tool", tool_call_id, content}`; non-tool-
                // aware backends flatten them to user-side text.
                for (i, native) in native_tool_calls.iter().enumerate() {
                    if i >= per_call_displays.len() {
                        break;
                    }
                    messages.push(LlmMessage {
                        role: LlmRole::Tool,
                        content: per_call_displays[i].clone(),
                        attachments: Vec::new(),
                        tool_call_id: native.id.clone(),
                        tool_calls: Vec::new(),
                        reasoning: None,
                    });
                }
                // Bundle fenced-mode replies (no id binding) plus any
                // loop-hint into a single User-role feedback message.
                // Skip emission entirely when there are no fenced
                // calls AND no loop hint, so a pure native-tool-call
                // turn doesn't leave a stray "Tool results:" prose
                // message in the history.
                let fenced_displays = &per_call_displays[native_dispatch_count..];
                if !fenced_displays.is_empty() || loop_hint_pending {
                    let mut feedback = String::new();
                    if loop_hint_pending {
                        feedback.push_str(LOOP_HINT_PREFIX);
                        loop_hint_pending = false;
                    }
                    if !fenced_displays.is_empty() {
                        feedback.push_str("Tool results:\n\n");
                        for display in fenced_displays {
                            feedback.push_str(display);
                            feedback.push_str("\n\n---\n\n");
                        }
                    }
                    messages.push(LlmMessage {
                        role: LlmRole::User,
                        content: feedback,
                        attachments: tool_attachments,
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        reasoning: None,
                    });
                } else if !tool_attachments.is_empty() {
                    // Native-only turn that produced attachments
                    // (e.g. images via a future tool). The Tool-role
                    // message shape doesn't carry attachments today;
                    // emit them as a bare User-role attachment turn
                    // so they don't get dropped.
                    messages.push(LlmMessage {
                        role: LlmRole::User,
                        content: String::new(),
                        attachments: tool_attachments,
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        reasoning: None,
                    });
                }

                // delete_file scope-override prompt. The tool returns
                // a stable `DELETE_SCOPE_VIOLATION_MARKER` prefix on
                // its err display whenever the requested path falls
                // outside the step's write allowlist AND the user has
                // not already approved that path this session. In
                // interactive mode we ask the user to confirm the
                // override; on a positive reply we add the path(s)
                // to `approved_deletes` so the agent's next attempt
                // succeeds without code changes.
                //
                // Auto mode keeps the silent-refuse behavior per the
                // explicit design decision -- unattended runs must not
                // pause for tool approvals (there's no operator there
                // to answer).
                if !opts.auto {
                    let scope_violations: Vec<String> = per_call_displays
                        .iter()
                        .filter_map(|d| {
                            d.lines().find_map(|line| {
                                line.strip_prefix(tools::DELETE_SCOPE_VIOLATION_MARKER)
                                    .map(|p| p.trim().to_string())
                            })
                        })
                        // De-duplicate: a single turn could in
                        // principle issue the same out-of-scope path
                        // twice; one prompt covers both.
                        .fold(Vec::<String>::new(), |mut acc, p| {
                            if !p.is_empty() && !acc.contains(&p) {
                                acc.push(p);
                            }
                            acc
                        });
                    if !scope_violations.is_empty() {
                        let listed = scope_violations
                            .iter()
                            .map(|p| format!("`{p}`"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let prompt = format!(
                            "The agent attempted `delete_file` for path(s) outside this step's write \
                             allowlist: {listed}. Allowlist for {}.{:?}: {}.\n\n\
                             Approve the override and remove the file(s)?\n\
                             - Reply `yes` (or `y`/`approve`) to grant a one-shot override for the listed path(s).\n\
                             - Reply `no` (or anything else) to refuse; the agent will see your reply verbatim and proceed without deleting.",
                            step.id,
                            opts.kind,
                            if write_paths.is_empty() {
                                "(none)".to_string()
                            } else {
                                write_paths.join(", ")
                            },
                        );
                        host.send(&Event::RequestUserInput {
                            prompt: Some(prompt),
                            placeholder: Some("yes / no, or course-correction text".into()),
                        })?;
                        match host.recv()? {
                            Some(HostEvent::UserMessage { text }) => {
                                let trimmed = text.trim().to_ascii_lowercase();
                                let approved = matches!(
                                    trimmed.as_str(),
                                    "y" | "yes" | "approve" | "ok" | "okay"
                                );
                                if approved {
                                    for p in &scope_violations {
                                        if !approved_deletes.iter().any(|q| q == p) {
                                            approved_deletes.push(p.clone());
                                        }
                                    }
                                    host.send(&Event::Diagnostic {
                                        level: DiagnosticLevel::Info,
                                        message: format!(
                                            "scope override granted for {} path(s): {listed}. \
                                             The agent's next delete_file call for the listed path(s) will proceed.",
                                            scope_violations.len()
                                        ),
                                    })?;
                                }
                                // Push the user's reply into the
                                // conversation so the model sees it
                                // verbatim. On "yes" this nudges the
                                // model to retry delete_file (the
                                // approved_deletes side-channel is
                                // already populated). On "no" the
                                // model sees the refusal and can
                                // course-correct.
                                messages.push(LlmMessage {
                                    role: LlmRole::User,
                                    content: text,
                                    attachments: Vec::new(),
                                    tool_call_id: None,
                                    tool_calls: Vec::new(),
                                    reasoning: None,
                                });
                            }
                            Some(HostEvent::Cancel) | None => {
                                host.send(&Event::SessionEnd {
                                    reason: SessionEndReason::Cancelled,
                                    message: None,
                                })?;
                                return Ok(());
                            }
                            Some(other) => {
                                host.send(&Event::Diagnostic {
                                    level: DiagnosticLevel::Warning,
                                    message: format!(
                                        "unexpected host event during delete_file scope-override prompt: {other:?}; treating as no-approve"
                                    ),
                                })?;
                            }
                        }
                    }
                }

                // No-progress tracker. Two complementary signals
                // decide whether this turn counts toward the bail:
                //
                //   1. **Touched pre-existing paths**: did the agent
                //      modify a path that's already in the step's
                //      manifest snapshot (i.e. a path the step has
                //      already created in this or a prior session)?
                //      A turn that only writes net-new files
                //      (e.g. `tests/diag_timing.rs` -- a probe) or
                //      only reads is a DATA COLLECTION turn; we
                //      give it a free pass on the no-progress
                //      counter so the agent has space to
                //      investigate before committing to a fix.
                //
                //   2. **Target failing-set delta**: did the *names*
                //      of the failing tests improve relative to the
                //      session's target set? A strictly-shrinking
                //      `target ∩ current` is progress (we fixed a
                //      target test). New failures NOT in target are
                //      regressions and disqualify a progress reset.
                //      This is strictly better than count-only: it
                //      distinguishes "fixed A, broke B" (count
                //      constant, but the set changed = no progress)
                //      from genuine fixes.
                //
                // Combined rule:
                //   fix attempt   + target shrank   -> reset counter (+ rebase target)
                //   fix attempt   + no shrink       -> increment counter
                //   data collection (no validation, or no touch of
                //     existing path)                -> free pass, but track
                //                                      investigation_only_iters
                // Update the "touched an existing path since last
                // test" flag from this turn's writes BEFORE we
                // classify. The flag is sticky across turns until
                // the next cargo-test fires (where the classifier
                // consumes and clears it). Without this stickiness,
                // a transport that emits edits and tests in
                // separate turns would always look like
                // investigation on the test turn.
                if !this_turn_touched_paths.is_disjoint(&pre_session_manifest) {
                    touched_existing_since_last_test = true;
                }
                if let Some(cur) = this_turn_test_count {
                    let current_failing: std::collections::HashSet<String> =
                        this_turn_test_failures
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .collect();
                    if target_failing_set.is_none() {
                        target_failing_set = Some(current_failing.clone());
                    }
                    let target = target_failing_set.as_ref().expect("set above");
                    match classify_progress(
                        target,
                        &current_failing,
                        touched_existing_since_last_test,
                        declared_fix_pending,
                    ) {
                        ProgressClass::Progress => {
                            no_progress_iters = 0;
                            investigation_only_iters = 0;
                            target_failing_set = Some(current_failing.clone());
                        }
                        ProgressClass::FixAttemptNoProgress => {
                            no_progress_iters += 1;
                            // Reset the investigation counter -- this
                            // wasn't pure investigation, the agent
                            // did attempt a fix.
                            investigation_only_iters = 0;
                        }
                        ProgressClass::Investigation => {
                            // Data collection turn: ran tests but
                            // didn't touch anything that was already
                            // step-owned. Free pass on
                            // no_progress_iters; the investigation
                            // cap catches runaway diagnostics.
                            investigation_only_iters += 1;
                        }
                    }
                    last_test_failure_count = Some(cur);
                    // Reset the sticky flags now that the classifier
                    // consumed them. Future turns start fresh: their
                    // touches / declare_fix must accumulate before
                    // the NEXT test run for the next classification
                    // to register as a fix attempt.
                    touched_existing_since_last_test = false;
                    declared_fix_pending = false;
                }

                // Test-expectation nudge (option C from the
                // classifier critique). Once the agent has declared
                // a meaningful number of fixes that still haven't
                // shrunk the target failing set, drop a one-time
                // reframing into the next turn's context: "have you
                // considered that the test EXPECTATION might be
                // wrong, not the implementation?" Fires at most once
                // per session and stays well below MAX_DECLARED_FIXES
                // so the agent gets a real chance to act on it
                // before the cap bails to interactive.
                if should_emit_expectation_nudge(
                    declared_fixes_count,
                    no_progress_iters,
                    expectation_nudge_emitted,
                    EXPECTATION_NUDGE_AFTER_FIXES,
                ) {
                    host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Info,
                        message: format!(
                            "auto: expectation nudge -- you have called `declare_fix` {} times \
                             this session and the target failing-test set has not shrunk. \
                             Before declaring another fix on the implementation, pause and \
                             consider whether the TEST EXPECTATION itself might be wrong: \
                             does the test assert the right value? Is the cycle count / port \
                             id / payload shape it expects actually what the spec says? If a \
                             test is asserting against a stale expectation, fix the test \
                             instead of chasing the impl.",
                            declared_fixes_count,
                        ),
                    })?;
                    expectation_nudge_emitted = true;
                }

                // Bail if we've burned `max_auto_iters` consecutive
                // FIX ATTEMPTS with no target-set improvement, OR if
                // the agent has spent `MAX_INVESTIGATION_ONLY_TURNS`
                // turns running diagnostics without committing to a
                // fix. Either diagnostic embeds `max_auto_iters` so
                // the AutoHost wrapper's existing substring matcher
                // cancels the in-flight sub-session and the auto
                // driver flips to manual mode -- no separate signal-
                // path to keep in sync.
                let hit_fix_cap =
                    this_turn_test_count.is_some() && no_progress_iters >= opts.max_auto_iters;
                let hit_investigation_cap =
                    investigation_only_iters >= MAX_INVESTIGATION_ONLY_TURNS;
                let hit_declared_cap = declared_fixes_count >= MAX_DECLARED_FIXES;
                if opts.auto && (hit_fix_cap || hit_investigation_cap || hit_declared_cap) {
                    let cur = last_test_failure_count.unwrap_or(0);
                    let message = if hit_fix_cap {
                        format!(
                            "auto: {} hit no-progress cap (max_auto_iters={}): {} consecutive fix-attempt turns \
                             with the target failing-test set not shrinking (current: {} test(s) failing). \
                             The agent is iterating on existing artifacts without measurable improvement; \
                             switching to interactive.",
                            step.id, opts.max_auto_iters, no_progress_iters, cur,
                        )
                    } else if hit_declared_cap {
                        format!(
                            "auto: {} hit declared-fix cap (max_auto_iters={}): the agent has called \
                             `declare_fix` {} times this session without the target failing-test set \
                             shrinking (current: {} test(s) failing). The agent is committing to fixes \
                             that don't pan out; switching to interactive so the operator can decide whether \
                             to raise the budget, inject framework context, or commit a fix manually.",
                            step.id, opts.max_auto_iters, declared_fixes_count, cur,
                        )
                    } else {
                        format!(
                            "auto: {} hit investigation-only cap (max_auto_iters={}): {} consecutive turns \
                             ran `cargo test` without modifying any pre-existing step artifact and without \
                             calling `declare_fix`. The agent appears stuck in data collection; switching to \
                             interactive so a human can commit to a fix direction. (If the agent has a real \
                             fix in a new file, teach it to call `declare_fix` before the test run.)",
                            step.id, opts.max_auto_iters, investigation_only_iters,
                        )
                    };
                    host.send(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message,
                    })?;
                    // Fall through (no `continue`) so the auto-iter
                    // / RequestUserInput tail of the loop runs.
                } else {
                    continue; // Tool calls succeeded; LLM continues.
                }
            }

            // 5f. If artifacts were written, run validators for the
            //     current phase. On failure: feed errors back, stay in
            //     this phase. On success: advance phase.
            if !artifacts.is_empty() {
                let current_phase = phases.get(phase_idx).copied().unwrap_or("chat");
                match run_phase_validator(current_phase, &opts.project_dir) {
                    Some(out) => {
                        host.send(&Event::BuildOutput {
                            command: out.command.clone(),
                            stdout_tail: out.stdout_tail.clone(),
                            stderr_tail: out.stderr_tail.clone(),
                            exit_code: out.exit_code,
                        })?;
                        if !out.ok() {
                            phase_iterations += 1;
                            if phase_iterations >= MAX_ITER_PER_PHASE {
                                host.send(&Event::Diagnostic {
                                    level: DiagnosticLevel::Error,
                                    message: format!(
                                        "{current_phase} phase exceeded {MAX_ITER_PER_PHASE} iterations; pausing for user input."
                                    ),
                                })?;
                            } else {
                                let mut content = String::new();
                                if loop_hint_pending {
                                    content.push_str(LOOP_HINT_PREFIX);
                                    loop_hint_pending = false;
                                }
                                content.push_str(&format!(
                                    "{} phase failed (`{}` exited {}). Fix the issues below and re-emit the affected files.\n\n{}\n\n{}",
                                    current_phase,
                                    out.command,
                                    out.exit_code,
                                    out.stderr_tail,
                                    out.stdout_tail
                                ));
                                messages.push(LlmMessage {
                                    role: LlmRole::User,
                                    content,
                                    attachments: Vec::new(),
                                    tool_call_id: None,
                                    tool_calls: Vec::new(),
                                    reasoning: None,
                                });
                                continue;
                            }
                        } else {
                            // Phase succeeded; advance.
                            phase_iterations = 0;
                            phase_idx += 1;
                            if let Some(next) = phases.get(phase_idx) {
                                host.send(&Event::PhaseChanged {
                                    phase: (*next).into(),
                                })?;
                            }
                        }
                    }
                    None => {
                        // No validator for this phase.
                    }
                }
                // Note: post-artifact gate emission was removed; the
                // user explicitly runs `/gate` (or the dashboard's
                // "Run Gate") to see status. Auto-emission was
                // misleading because gate checks like `critique_clean`
                // can only pass after the critique session has run.

                // Auto-mode: evaluate the structural gate (CritiqueClean
                // excluded) and either end the session or feed failures
                // back to the agent. Cap iterations at
                // `opts.max_auto_iters` so a confused agent can't
                // burn turns indefinitely.
                //
                // Critique sessions skip this entirely: a critique
                // session writes an evaluation, not project artifacts,
                // and the structural gate is meaningless at that
                // point (e.g. for a milestone-walk step the gate is
                // dirty by design until every milestone resolves --
                // critique iterations are not the path that resolves
                // them). The OUTER auto-driver evaluates the full
                // gate after each sub-session ends; the critique
                // session itself ends naturally on a no-artifact
                // turn (the wind-down branch below).
                if opts.auto && opts.kind == SessionKind::Work {
                    // Auto-tick milestone task rows whose code has
                    // landed on disk before evaluating the gate.
                    // Agents (qwen3.6 in particular) write the
                    // artifact then forget to flip `- [ ]` to
                    // `- [x]`, the gate stays dirty for what looks
                    // to the agent like already-finished work, and
                    // the auto loop burns its `max_auto_iters`
                    // budget arguing about it. The helper only
                    // flips a row when its first backtick-quoted
                    // token (`<path>::<Symbol>`) maps to a file
                    // that exists AND the symbol grep-matches; it
                    // is intentionally conservative.
                    let _flipped = crate::__internal::steps::tick_resolved_milestone_tasks(
                        &opts.project_dir,
                        &step,
                    );
                    // walk_scope=true: during a milestone walk, only
                    // check the cheap per-milestone gate (when the
                    // step defines one). Falls back to the full step
                    // gate for non-walking steps -- same behavior as
                    // before this split.
                    let report = evaluate_structural_gate(
                        &opts.project_dir,
                        &step,
                        /*walk_scope=*/ true,
                    )?;
                    if report.is_clean() {
                        host.send(&Event::SessionEnd {
                            reason: SessionEndReason::Completed,
                            message: Some(format!("auto: {} structural gate clean", step.id)),
                        })?;
                        return Ok(());
                    }
                    auto_iterations += 1;
                    if auto_iterations >= opts.max_auto_iters {
                        host.send(&Event::Diagnostic {
                            level: DiagnosticLevel::Error,
                            message: format!(
                                "auto: {} exceeded max_auto_iters ({}); switching to interactive. Last gate failures: {}",
                                step.id,
                                opts.max_auto_iters,
                                report
                                    .failures
                                    .iter()
                                    .map(|f| format!("{}: {}", f.description, f.reason))
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            ),
                        })?;
                        // Fall through to RequestUserInput below; the
                        // CLI-side auto driver will see the
                        // RequestUserInput and decide whether to end
                        // the session or hand control to the user.
                    } else {
                        let mut feedback =
                            "Structural gate is not yet clean. Re-emit the affected artifact(s) with these issues fixed:\n\n"
                                .to_string();
                        for f in &report.failures {
                            feedback.push_str(&format!("- {}: {}\n", f.description, f.reason));
                        }
                        messages.push(LlmMessage {
                            role: LlmRole::User,
                            content: feedback,
                            attachments: Vec::new(),
                            tool_call_id: None,
                            tool_calls: Vec::new(),
                            reasoning: None,
                        });
                        continue; // Don't ask the user; agent retries.
                    }
                }
            }
        }

        // Captured from the wind-down branch below so the no-artifact
        // pump can include the orchestrator's gate failures verbatim
        // (Work sessions only). Without this the orchestrator runs the
        // entire `cargo fmt / clippy / build / test` gate every
        // no-artifact turn for the wind-down decision, then throws
        // the failure list away and tells the agent a generic
        // "Produce the artifact file(s) now". The agent then
        // rediscovers everything by calling `run_cargo` and reading
        // files -- duplicating the work the orchestrator already
        // did and burning turns blindly. Threading the structured
        // failures through gives the agent directly actionable
        // feedback identical to the post-artifact gate-dirty branch.
        let mut work_gate_failures_for_pump: Option<String> = None;
        // Auto-mode and no artifact written this turn: the agent is
        // either thinking, asking, stuck, OR genuinely done.
        //
        // Genuinely-done case: the agent has already produced the
        // step's artifact(s) on a prior turn, run cargo verify /
        // test as the prompt requires, and is now winding down with
        // a "ready for gate-check" summary. If the structural gate
        // is clean, ending the session here lets the critique
        // session start. Without this check the wind-down turns
        // (cargo build / cargo test / summary, all of which produce
        // no NEW artifact) burn `max_auto_iters` and the agent's
        // completed work gets cancelled with a runaway-guard error.
        // Nudge cases (asking, stuck) still hit the cap path
        // because the structural gate fails when the artifact isn't
        // on disk yet.
        if opts.auto && effective_artifacts_empty(&assistant_text, opts.kind) && !llm_failed {
            // Work sessions: if the structural gate is already
            // clean, the work is done -- end so the auto driver
            // can launch the critique session. (Critique sessions
            // don't have this exit; they end via the regular
            // empty-artifact path because their gate check is
            // file-existence on the critique markdown, which is
            // already evaluated by the time we reach here.)
            if opts.kind == SessionKind::Work {
                let can_wind_down_clean = can_auto_wind_down_clean_work_session(
                    work_retry_has_prior_blockers,
                    session_persisted_writes,
                );
                // walk_scope=true: see the analogous call above. The
                // failures captured below for the no-artifact pump
                // also come from the walk gate, so the agent only
                // sees actionable per-milestone failures (not the
                // deferred integration checks).
                let report =
                    evaluate_structural_gate(&opts.project_dir, &step, /*walk_scope=*/ true)?;
                if can_wind_down_clean && report.is_clean() {
                    host.send(&Event::SessionEnd {
                        reason: SessionEndReason::Completed,
                        message: Some(format!(
                            "auto: {} structural gate clean (no-artifact wind-down)",
                            step.id
                        )),
                    })?;
                    return Ok(());
                }
                // Milestone-walk steps: the per-step structural
                // gate stays dirty until EVERY milestone has all
                // rows resolved (MilestonesAllResolved), so the
                // gate-clean check above never fires until the
                // last milestone closes. But each individual
                // milestone wind-down is also a legitimate
                // session end -- we want the paired critique to
                // run and the next iteration to scope to the
                // next milestone. Detect "the milestone the
                // agent has been working on is done" by checking
                // whether the current milestone file has any
                // `- [x]` rows AND no `- [ ]` rows. If yes, the
                // agent finished its scoped slice; end the
                // session.
                // Mirror the structural-gate-clean branch's gating:
                // allow wind-down when there are no prior critique
                // blockers OR when this session has persisted writes.
                // Previously this branch hard-required
                // `session_persisted_writes`, which trapped resumed
                // sessions: if a prior run already finished the
                // current milestone and the resumed Work session has
                // nothing left to write, the agent honestly emits a
                // "done; run /advance" reply, the wind-down doesn't
                // fire, and max_auto_iters trips even though the
                // milestone genuinely is complete on disk. The inner
                // `milestone_done` check (a touched-milestone exists
                // that's different from the next pending one) is the
                // real safety: it only fires when at least one
                // milestone file in this walk has all `- [x]` rows.
                if let Some(walk) = step.milestone_walk
                    && can_auto_wind_down_clean_work_session(
                        work_retry_has_prior_blockers,
                        session_persisted_writes,
                    )
                {
                    use crate::__internal::steps::CurrentMilestone;
                    let milestone_done = if let Some(name) = &opts.milestone_name {
                        // Pinned worker (parallel plan-detail walk):
                        // the global walker can't see "this session's
                        // milestone" while other workers are racing
                        // through their own stubs. Ask directly
                        // whether the assigned stub is resolved.
                        match crate::__internal::steps::find_milestone_by_name(
                            &opts.project_dir,
                            &walk,
                            name,
                        ) {
                            CurrentMilestone::File(rel) => {
                                crate::__internal::steps::milestone_is_resolved(
                                    &walk,
                                    &opts.project_dir.join(&rel),
                                )
                            }
                            _ => false,
                        }
                    } else {
                        let current = crate::__internal::steps::find_current_milestone(
                            &opts.project_dir,
                            &walk,
                            false,
                        );
                        match &current {
                            // No more pending milestones: this case
                            // is identical to "structural gate clean"
                            // and is actually unreachable here
                            // (MilestonesAllResolved would have made
                            // the gate clean above), but keep the
                            // arm for safety.
                            CurrentMilestone::AllResolved => true,
                            CurrentMilestone::File(rel) => {
                                // The current milestone is the one
                                // with the FIRST `- [ ]` row -- so
                                // by definition `current` always has
                                // pending rows. To detect "milestone
                                // just finished", check the highest-
                                // numbered milestone with at least
                                // one `- [x]` (i.e. retry-mode pick).
                                // If that's a DIFFERENT file than
                                // the current pending one, the agent
                                // finished a milestone this session.
                                let touched = crate::__internal::steps::find_current_milestone(
                                    &opts.project_dir,
                                    &walk,
                                    true,
                                );
                                matches!(
                                    &touched,
                                    CurrentMilestone::File(t) if t != rel
                                )
                            }
                            CurrentMilestone::NoMilestonesPresent => false,
                        }
                    };
                    if milestone_done {
                        host.send(&Event::SessionEnd {
                            reason: SessionEndReason::Completed,
                            message: Some(format!(
                                "auto: {} milestone complete (no-artifact wind-down); critique will run",
                                step.id
                            )),
                        })?;
                        return Ok(());
                    }
                }
                // Wind-down didn't fire and the gate has real
                // failures. Capture them so the no-artifact pump
                // below can hand the agent specific guidance instead
                // of the generic "produce artifacts" prod.
                if !report.is_clean() {
                    let mut feedback = String::from(
                        "Structural gate has these specific failures; \
                         address them by writing or editing files:\n\n",
                    );
                    for f in &report.failures {
                        feedback.push_str(&format!("- {}: {}\n", f.description, f.reason));
                    }
                    work_gate_failures_for_pump = Some(feedback);
                }
            }
            auto_iterations += 1;
            if auto_iterations >= opts.max_auto_iters {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "auto: {} exceeded max_auto_iters ({}) without producing an artifact; switching to interactive.",
                        step.id, opts.max_auto_iters
                    ),
                })?;
                // Fall through to RequestUserInput.
            } else {
                let pump_content = match work_gate_failures_for_pump {
                    Some(failures) => format!(
                        "You are in automated mode. The structural gate is not yet clean. \
                         {failures}\n\
                         Use `write_file` / `edit_file` to fix the listed failures now; do not \
                         ask questions, decide using the inlined state and document non-trivial \
                         decisions in an `## Auto-decisions` section."
                    ),
                    None => "You are in automated mode. Produce the artifact file(s) now using the artifact-write convention. Do not ask questions; decide using the inlined state and document your decisions in an `## Auto-decisions` section.".to_string(),
                };
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: pump_content,
                    attachments: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    reasoning: None,
                });
                continue;
            }
        }

        // 5g. Wait for the user's next message (or cancellation). When
        //     the previous turn ended in an LlmError we emit a richer
        //     prompt so the operator sees the error inline and knows the
        //     available actions (retry, cancel, course-correct). The
        //     `Followup` quick-actions pair with the prompt for hosts
        //     that render buttons.
        let (request_prompt, request_placeholder) = if llm_failed {
            let k = llm_error_kind.as_deref().unwrap_or("unknown");
            let m = llm_error_message.as_deref().unwrap_or("");
            (
                Some(format!(
                    "LLM dispatch failed ({k}): {m}\n\n\
                     - Type `/retry` to re-issue the same request to the same backend.\n\
                     - Type `/end-session` to abort.\n\
                     - Type any other message to send a course-correction prompt.\n\
                     To switch backends or models, end the session and \
                     restart with a different `--llm-backend` / `--llm-model`."
                )),
                Some("/retry, /end-session, or a course-correction message".into()),
            )
        } else {
            (None, None)
        };
        if llm_failed {
            host.send(&Event::Followup {
                label: "Retry".into(),
                action: "/retry".into(),
            })?;
            host.send(&Event::Followup {
                label: "Cancel".into(),
                action: "/end-session".into(),
            })?;
        }
        host.send(&Event::RequestUserInput {
            prompt: request_prompt,
            placeholder: request_placeholder,
        })?;
        match host.recv()? {
            Some(HostEvent::UserMessage { text }) => {
                // `/retry` after an LlmError re-issues the same request
                // to the same backend without a course-correction turn.
                // We don't push the literal `/retry` onto `messages`
                // (the LLM never sees it) -- just continue the outer
                // loop, which re-sends the unchanged `messages`.
                if llm_failed && text.trim() == "/retry" {
                    continue;
                }
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: text,
                    attachments: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    reasoning: None,
                });
                empty_response_retries = 0;
            }
            Some(HostEvent::Cancel) | None => {
                host.send(&Event::SessionEnd {
                    reason: SessionEndReason::Cancelled,
                    message: None,
                })?;
                return Ok(());
            }
            Some(other) => {
                host.send(&Event::Diagnostic {
                    level: DiagnosticLevel::Warning,
                    message: format!("unexpected host event waiting for input: {other:?}"),
                })?;
                // Stay in the loop; emit RequestUserInput again on next pass.
            }
        }

        // Lightweight escape hatch for tests / cooperative hosts:
        // a literal "/end-session" user message ends the session
        // cleanly. Hosts that want a button can map it to this string
        // until M5 wires Followup events end-to-end.
        if let Some(LlmMessage {
            role: LlmRole::User,
            content,
            ..
        }) = messages.last()
            && content.trim() == "/end-session"
        {
            host.send(&Event::SessionEnd {
                reason: SessionEndReason::Completed,
                message: None,
            })?;
            return Ok(());
        }
    }
}

/// Best-effort RetrievalService construction for the per-session
/// tool dispatcher. The service powers `spec_semantic_search`,
/// `signal_table_query`, and `api_semantic_search`. Returns `None`
/// (with a Diagnostic surfaced through the presenter) when:
///
/// - no `embedder.toml` is reachable (project / env / home priority),
/// - the embedder construction fails (LM Studio not running, etc.),
/// - the runtime construction fails.
///
/// `build_dispatcher_with_runtime` silently drops the retrieval tools
/// when the service is `None`; the per-step prompts already mention
/// them, so without this Diagnostic the operator has no clue why the
/// agent searches `lib:docs` for "spec_semantic_search" instead of
/// calling it.
///
/// Errors here are NEVER fatal to the session: the agent can still
/// drive the work with the non-retrieval tools (read_file, write_file,
/// search, etc.), so we surface the warning and continue.
fn build_retrieval_service<P: Presenter + ?Sized>(
    project_dir: &std::path::Path,
    presenter: &mut P,
) -> Result<Option<Arc<crate::__internal::session::retrieval::RetrievalService>>> {
    use crate::__internal::session::embedder::EmbedderConfig;
    use crate::__internal::session::retrieval::RetrievalService;

    let config = match EmbedderConfig::load() {
        Ok(c) => c,
        Err(e) => {
            presenter.send(&Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message: format!(
                    "retrieval tools disabled (spec_semantic_search, \
                     signal_table_query, api_semantic_search): no embedder \
                     config found. Create `.sim-flow/embedder.toml` (or \
                     `~/.sim-flow/embedder.toml`) pointing at your embedding \
                     server (e.g. LM Studio's nomic-embed model). Underlying \
                     error: {e}"
                ),
            })?;
            return Ok(None);
        }
    };

    let service = match RetrievalService::from_embedder_config(project_dir, config) {
        Ok(s) => s,
        Err(e) => {
            presenter.send(&Event::Diagnostic {
                level: DiagnosticLevel::Warning,
                message: format!(
                    "retrieval tools disabled: failed to construct \
                     RetrievalService for project `{}`: {e}. Verify the \
                     embedder server is reachable and the project's lance \
                     index has been built (`sim-flow build-spec-index`).",
                    project_dir.display(),
                ),
            })?;
            return Ok(None);
        }
    };

    Ok(Some(Arc::new(service)))
}

/// Build the `RequestUserInput.prompt` string the host renders for an
/// `ask_user` suspension. Combines the question with the optional
/// `context` paragraph and `choices` list so the chat panel can show
/// the question in full without re-querying the orchestrator. Mirrors
/// the Architecture §4.5 "Return shape" expectations: agent emits a
/// focused question, optional rationale, and (for choice questions) an
/// inline set of allowed values.
fn build_ask_user_prompt(pending: &crate::__internal::session::ask_user::PendingUserAsk) -> String {
    let mut out = pending.question.clone();
    if !pending.context.trim().is_empty() {
        out.push_str("\n\n");
        out.push_str(pending.context.trim());
    }
    if !pending.choices.is_empty() {
        out.push_str("\n\nChoices: ");
        out.push_str(&pending.choices.join(" / "));
    }
    if let Some(def) = pending.default.as_ref().filter(|d| !d.is_empty()) {
        out.push_str("\n\n(Default: ");
        out.push_str(def);
        out.push(')');
    }
    out
}

/// Map the pending ask's `kind` to a chat-input placeholder hint. The
/// orchestrator passes this to `RequestUserInput.placeholder` so the
/// host can prefill its composer hint or render an inline reply
/// affordance (quick-pick chips for yes-no / choice, free-form
/// text input for the rest). Cancel commands are documented inline
/// for every kind so the user discovers them without having to consult
/// help.
fn ask_user_placeholder(pending: &crate::__internal::session::ask_user::PendingUserAsk) -> String {
    use crate::__internal::session::ask_user::AskUserKind;
    let suffix = " (or /cancel, /cancel-thread)";
    match pending.kind {
        AskUserKind::YesNo => format!("yes / no{suffix}"),
        AskUserKind::Choice => {
            if pending.choices.is_empty() {
                format!("choose one{suffix}")
            } else {
                format!("{}{suffix}", pending.choices.join(" / "))
            }
        }
        AskUserKind::Value => format!("a value{suffix}"),
        AskUserKind::FreeForm => format!("answer{suffix}"),
    }
}
