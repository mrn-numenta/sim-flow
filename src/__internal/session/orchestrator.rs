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

use std::path::{Path, PathBuf};

use crate::client::SessionKind;
use crate::config::Config;
use crate::gate::{self, GateCheck, GateReport};
use crate::prompts;
use crate::session::host::Host;
use crate::session::protocol::{
    DiagnosticLevel, Event, HostEvent, LlmMessage, LlmRole, LlmTool, PROTOCOL_VERSION,
    SessionEndReason, SessionKindOut, SessionTag, StepDescriptorOut,
};
use crate::session::runners;
use crate::session::tools::{self, Tool, ToolResult};
use crate::state::State;
use crate::steps::{StepDescriptor, registry_for};
use crate::{Error, Result};

const FRAMEWORK_DOCS_ROOT_ENV: &str = "SIM_FLOW_FRAMEWORK_DOCS_ROOT";

/// One-strike-warning prefix injected into the next user message
/// the orchestrator builds when the runaway-loop detector sees
/// `cap - 1` structurally-identical responses in a row. The next
/// identical response will trip the abort; this gives the agent
/// one explicit chance to break the cycle by re-reading the prior
/// tool / build error rather than retrying the same call shape.
const LOOP_HINT_PREFIX: &str = "Loop guard warning: your last response was structurally identical to the prior one. \
     If the next response is also identical the orchestrator will abort the session. \
     If a tool call or build is failing repeatedly with the same error, RE-READ the error below \
     before retrying — the call shape may be wrong, the file may not exist, the path may be \
     unwritable, or the operation may simply not be possible in the current state. Try a \
     different approach.\n\n";

/// Inputs the caller (CLI dispatch) passes to `run_session`.
pub struct OrchestratorOptions {
    pub project_dir: PathBuf,
    pub foundation_root: PathBuf,
    pub step_id: String,
    pub kind: SessionKind,
    pub candidate: Option<String>,
    /// Opaque label echoed back inside `RequestLlmResponse` so the
    /// host knows which client to dispatch to (e.g. "vscode",
    /// "anthropic"). The orchestrator never inspects this.
    pub llm_backend: String,
    /// Optional model identifier the host should pass to its client.
    pub llm_model: Option<String>,
    /// Optional base URL override for the local-server backends
    /// (`ollama`, `lmstudio`, `vllm`, `openai-compat`). Forwarded
    /// here for parity with `AutoOptions::llm_base_url`, but the
    /// orchestrator itself doesn't read it -- the JSONL host picks
    /// its endpoint from the dashboard's `sim-flow.llm.servers`
    /// setting, and the in-process `session_cmd` path consumes the
    /// flag directly into `AgentConfig::base_url`. The field is
    /// retained on `OrchestratorOptions` so future host
    /// implementations that want to surface it can do so without
    /// another schema change.
    pub llm_base_url: Option<String>,
    /// Run this session unattended. The agent is told not to ask the
    /// user any questions; on each turn that writes artifacts we
    /// re-evaluate the structural gate (CritiqueClean is excluded
    /// because critique runs in a separate session) and either end
    /// cleanly or feed failures back to the agent. Caller drives
    /// the cross-session work/critique/advance loop.
    pub auto: bool,
    /// Maximum turns the orchestrator will spend re-feeding gate
    /// failures to the agent in `auto` mode before giving up. Ignored
    /// when `auto` is false.
    pub max_auto_iters: u32,
    /// Hard cap on TOTAL LLM requests in this session, regardless of
    /// what loop they came from (gate-failure retries, empty-response
    /// retries, tool-result feedback turns, etc.). Backstop against
    /// runaway loops that the more specific `max_auto_iters` /
    /// `max_critique_iters` caps don't catch -- e.g. a new failure
    /// mode where the agent keeps emitting the same error and the
    /// orchestrator keeps retrying. Hitting this cap aborts the
    /// session cleanly with a diagnostic; no further LLM requests
    /// fire. Default 50; tune via `--max-llm-requests`.
    pub max_llm_requests: u32,
    /// Number of consecutive byte-identical assistant responses that
    /// triggers a "stuck loop" abort. The agent producing the same
    /// text three turns running is a clear signal it's not making
    /// progress, but the structural-gate retry path keeps feeding it
    /// the same failure list -- so the iteration cap alone won't
    /// catch this. Default 3; set to 0 to disable.
    pub max_identical_responses: u32,
    /// True when the agent driving this session has its own native
    /// filesystem tools (Write, Edit, Read, Glob -- e.g. an
    /// interactive `claude` / `codex` / `gh-copilot` PTY) and the
    /// orchestrator is NOT going to extract fenced ` ```<path>`
    /// artifact-write blocks from the agent's response text. In that
    /// mode the artifact-write convention is harmful: the agent
    /// emits the fence expecting an external writer, no writer
    /// exists, so the file lands on disk only after the agent
    /// realises the disconnect and re-issues a Write tool call.
    /// We swap the convention message for instructions that point
    /// at the native tools instead.
    pub agent_has_native_fs_tools: bool,
    /// When true, load the `_conventions/no-preamble.md` convention
    /// into every session's system prompt. Tells the agent to lead
    /// with tool calls, skip recaps / hedging, and defer prose
    /// until after the work lands. Default true: verbose-CoT
    /// models (qwen3.6 etc.) routinely burn the full `max_tokens`
    /// budget on preamble and truncate mid-tool-call, so silencing
    /// the preamble is the safer baseline. Disable
    /// (`--preamble`) when debugging a model's reasoning -- the
    /// extra prose is what you're trying to read in that case.
    pub no_preamble: bool,
}

impl Default for OrchestratorOptions {
    fn default() -> Self {
        Self {
            project_dir: PathBuf::new(),
            foundation_root: PathBuf::new(),
            step_id: String::new(),
            kind: SessionKind::Work,
            candidate: None,
            llm_backend: String::new(),
            llm_model: None,
            llm_base_url: None,
            auto: false,
            max_auto_iters: 3,
            max_llm_requests: 50,
            max_identical_responses: 3,
            agent_has_native_fs_tools: false,
            no_preamble: true,
        }
    }
}

/// Top-level entry point. Drives the session to completion against
/// the supplied host. Returns `Err` on protocol / I/O failures; clean
/// session end (user cancelled or gate clean) returns `Ok(())`.
pub fn run_session<H: Host>(opts: OrchestratorOptions, host: &mut H) -> Result<()> {
    let log = crate::session::debug_log::DebugLog::open(&opts.project_dir);
    let mut wrapped = crate::session::debug_log::LoggingHost::new(host, &log);
    run_session_inner(opts, &mut wrapped)
}

fn run_session_inner<H: Host>(opts: OrchestratorOptions, host: &mut H) -> Result<()> {
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
    let hello = match host.read()? {
        Some(HostEvent::Hello {
            protocol_version, ..
        }) => protocol_version,
        Some(other) => {
            host.write(&Event::SessionEnd {
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
        host.write(&Event::SessionEnd {
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
    host.write(&Event::HelloAck {
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
    let dispatcher = tools::build_dispatcher(crate::steps::UNIVERSAL_TOOLS);
    let library_root = detect_library_root(&opts.project_dir);
    let framework_root = detect_framework_root(&opts.foundation_root);
    let framework_docs_root = detect_framework_docs_root(&opts.foundation_root);
    let write_paths: Vec<String> = crate::steps::allowed_write_paths(&step, opts.kind);

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
        host.write(&Event::PhaseChanged { phase: (*p).into() })?;
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
    // Did THIS session produce any successful artifact / tool call
    // yet? Used by the per-milestone wind-down exit so we don't
    // misfire when the agent's first turn is just reading inputs:
    // without this flag, a milestone-walk step where milestone-N-1
    // is already on disk (from a prior killed run) would see
    // `find_current_milestone(retry=true) != find_current_milestone(retry=false)`
    // on turn 1 and end the session prematurely.
    let mut session_produced_artifact: bool = false;

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

    // 5c. Turn loop.
    let mut turn_index: u32 = 0;
    loop {
        turn_index += 1;
        // Hard cap on total LLM requests. Hitting this aborts the
        // session before another paid call goes out.
        if opts.max_llm_requests > 0 && turn_index > opts.max_llm_requests {
            host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!(
                    "session aborted: hit max_llm_requests cap ({}) -- runaway-loop guard. \
                     Raise `--max-llm-requests` if your flow legitimately needs more turns; \
                     otherwise inspect the recent dispatch history for a stuck retry.",
                    opts.max_llm_requests,
                ),
            })?;
            host.write(&Event::SessionEnd {
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
            host.write(&Event::Diagnostic {
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
            host.write(&Event::SessionEnd {
                reason: SessionEndReason::RunawayGuard,
                message: Some(format!(
                    "{} consecutive failed-tool turns",
                    consecutive_tool_error_turns
                )),
            })?;
            return Ok(());
        }
        let request_id = format!("lr-{turn_index}");
        // Per-turn wall-time tracking. Hosts emit per-call token /
        // wall metrics at the dispatch site (TerminalHost via
        // `agent.dispatch`); the orchestrator measures end-to-end
        // round-trip including chunk streaming and protocol
        // serialization, which can differ from the host-side view.
        let turn_started = std::time::Instant::now();
        host.write(&Event::RequestLlmResponse {
            request_id: request_id.clone(),
            backend: opts.llm_backend.clone(),
            model: opts.llm_model.clone(),
            messages: messages.clone(),
            tools: llm_tools.clone(),
        })?;

        let mut assistant_text = String::new();
        let mut llm_failed = false;
        let mut llm_error_kind: Option<String> = None;
        let mut llm_error_message: Option<String> = None;
        loop {
            match host.read()? {
                Some(HostEvent::LlmChunk {
                    request_id: rid,
                    text,
                }) if rid == request_id => {
                    host.write(&Event::AssistantText {
                        text: text.clone(),
                        final_chunk: false,
                    })?;
                    assistant_text.push_str(&text);
                }
                Some(HostEvent::LlmEnd {
                    request_id: rid, ..
                }) if rid == request_id => {
                    host.write(&Event::AssistantText {
                        text: String::new(),
                        final_chunk: true,
                    })?;
                    let turn_wall_ms = turn_started.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "sim_flow::metrics",
                        event = "turn_end",
                        step = step.id,
                        kind = ?opts.kind,
                        request_id = %request_id,
                        turn_index,
                        assistant_bytes = assistant_text.len(),
                        wall_ms = turn_wall_ms,
                    );
                    break;
                }
                Some(HostEvent::LlmError {
                    request_id: rid,
                    kind,
                    message,
                }) if rid == request_id => {
                    host.write(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message: format!("LLM error ({kind}): {message}"),
                    })?;
                    llm_failed = true;
                    llm_error_kind = Some(kind);
                    llm_error_message = Some(message);
                    break;
                }
                Some(HostEvent::Cancel) => {
                    host.write(&Event::SessionEnd {
                        reason: SessionEndReason::Cancelled,
                        message: None,
                    })?;
                    return Ok(());
                }
                Some(other) => {
                    // Out-of-order events: emit a diagnostic and keep waiting.
                    host.write(&Event::Diagnostic {
                        level: DiagnosticLevel::Warning,
                        message: format!("unexpected host event during LLM call: {other:?}"),
                    })?;
                }
                None => {
                    return Err(Error::HostClosed("mid-turn".into()));
                }
            }
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
                host.write(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "session aborted: agent produced {} structurally-identical responses in a row -- runaway-loop guard. \
                         The structural content (after stripping digits / timestamps) was the same; \
                         feeding it back another identical prompt is unlikely to help. \
                         Inspect `.sim-flow/logs/sim-flow-chat.log` for the recent transcript.",
                        cap,
                    ),
                })?;
                host.write(&Event::SessionEnd {
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
        if !llm_failed && assistant_text.trim().is_empty() {
            if empty_response_retries < MAX_EMPTY_RETRIES {
                host.write(&Event::Diagnostic {
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
                });
                empty_response_retries += 1;
                continue;
            } else {
                host.write(&Event::Diagnostic {
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

        if llm_failed || assistant_text.trim().is_empty() {
            // Skip post-processing on this turn; ask user for input.
        } else {
            messages.push(LlmMessage {
                role: LlmRole::Assistant,
                content: assistant_text.clone(),
                attachments: Vec::new(),
            });

            // 5d. Extract artifacts and write them.
            let mut artifacts = extract_artifacts(&assistant_text);

            // Critique-session fallback: when the agent emits the
            // critique body inline (markdown prose / tables / lists)
            // instead of wrapping it in a fenced ` ```<path>` block as
            // the artifact-write convention requires, the extractor
            // sees nothing and the auto driver loops until the cap
            // fires -- even when the critique itself is fine
            // ("UNRESOLVED items only, no BLOCKERs"). The critique
            // file format is free-form markdown (only the
            // `BLOCKER:` / `UNRESOLVED:` / `RESOLVED:` line prefixes
            // matter to the gate), so the entire response works as
            // valid content. We only apply this when the turn also
            // produced no tool calls -- a turn that's purely
            // `read_file` calls is the agent gathering input, not
            // emitting the critique. Restricting to critique
            // sessions keeps work-session "agent talked, didn't
            // write" cases on the existing nudge-and-cap path
            // (work sessions have multiple expected artifacts; we
            // can't auto-deduce which one prose belongs to).
            let pre_check_tool_calls = tools::extract_tool_calls(&assistant_text);
            if artifacts.is_empty()
                && opts.kind == SessionKind::Critique
                && pre_check_tool_calls.is_empty()
                && !assistant_text.trim().is_empty()
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
                    host.write(&Event::Diagnostic {
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
                } else {
                    // No fenced block AND no findings: refuse to
                    // commit. Push a corrective User turn so the
                    // agent retries with proper structure rather
                    // than the gate clearing on a no-op.
                    host.write(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message: format!(
                            "{}: critique response had no fenced artifact-write block AND \
                             no `BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` finding lines. \
                             Refusing to auto-save -- a critique with no findings would \
                             ghost-pass the gate. Re-prompting the agent.",
                            step.id,
                        ),
                    })?;
                    messages.push(LlmMessage {
                        role: LlmRole::User,
                        content: format!(
                            "Your previous response did not contain a fenced \
                             ```docs/critiques/{}-critique.md`` block AND it did not contain \
                             any `BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` finding lines. \
                             A critique with no findings would silently pass the gate. \
                             Re-emit the critique now: either as a fenced artifact-write \
                             block at the path above, or as free-form markdown that \
                             contains explicit finding-marker lines. At minimum every \
                             evaluation question should produce one `RESOLVED:` or \
                             `UNRESOLVED:` line, and any gap is a `BLOCKER:`.",
                            step.id,
                        ),
                        attachments: Vec::new(),
                    });
                    continue;
                }
            }
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
                let started = std::time::Instant::now();
                match write_artifact(&opts.project_dir, &write_paths, art) {
                    Ok(bytes) => {
                        artifact_write_successes += 1;
                        host.write(&Event::ArtifactWritten {
                            path: art.relative_path.clone(),
                            bytes,
                        })?;
                        host.write(&Event::ToolInvoked {
                            name: "write_file".into(),
                            args_summary: art.relative_path.clone(),
                            status: "ok".into(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        })?;
                    }
                    Err(err) => {
                        let detail = format!("{err}");
                        host.write(&Event::Diagnostic {
                            level: DiagnosticLevel::Error,
                            message: format!("failed to write {}: {detail}", art.relative_path),
                        })?;
                        host.write(&Event::ToolInvoked {
                            name: "write_file".into(),
                            args_summary: art.relative_path.clone(),
                            status: "error".into(),
                            duration_ms: started.elapsed().as_millis() as u64,
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
                session_produced_artifact = true;
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
                });
                continue;
            }

            // 5e. Extract + dispatch fenced tool calls. Tools that
            //     match the dispatcher run; their results feed back as
            //     the next user message and we continue without
            //     prompting the user.
            let tool_calls = tools::extract_tool_calls(&assistant_text);
            if !tool_calls.is_empty() {
                let mut feedback = String::new();
                if loop_hint_pending {
                    feedback.push_str(LOOP_HINT_PREFIX);
                    loop_hint_pending = false;
                }
                feedback.push_str("Tool results:\n\n");
                let mut tool_attachments: Vec<crate::session::protocol::LlmAttachment> = Vec::new();
                // Take the LAST test_failure_count seen this turn.
                // Multiple `run_cargo test` calls in one turn (e.g.
                // the agent re-runs after a fix) collapse to the
                // most recent measurement.
                let mut this_turn_test_count: Option<usize> = None;
                let mut tool_successes: u32 = 0;
                let mut tool_failures: u32 = 0;
                for call in &tool_calls {
                    let started = std::time::Instant::now();
                    let ctx = tools::ToolContext::new(
                        &opts.project_dir,
                        library_root.as_deref(),
                        framework_root.as_deref(),
                        framework_docs_root.as_deref(),
                    )
                    .with_write_paths(&write_paths);
                    let outcome = invoke_tool(&dispatcher, &ctx, call);
                    let status = if outcome.ok { "ok" } else { "error" };
                    if outcome.ok {
                        tool_successes += 1;
                    } else {
                        tool_failures += 1;
                    }
                    host.write(&Event::ToolInvoked {
                        name: call.name.clone(),
                        args_summary: tool_args_summary(call),
                        status: status.into(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    })?;
                    feedback.push_str(&outcome.display);
                    feedback.push_str("\n\n---\n\n");
                    if let Some(c) = outcome.test_failure_count {
                        this_turn_test_count = Some(c);
                    }
                    for att in outcome.attachments {
                        tool_attachments.push(crate::session::protocol::LlmAttachment {
                            mime: att.mime,
                            data: base64_encode(&att.bytes),
                            source: Some(att.source_path),
                        });
                    }
                }
                // Tool-error-streak tracking. All-failed turn -> bump
                // streak; any-succeeded turn -> reset.
                if tool_failures > 0 && tool_successes == 0 {
                    consecutive_tool_error_turns += 1;
                } else if tool_successes > 0 {
                    consecutive_tool_error_turns = 0;
                }
                if tool_successes > 0 {
                    session_produced_artifact = true;
                }
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: feedback,
                    attachments: tool_attachments,
                });

                // No-progress tracker. Strictly-decreasing failure
                // count = real progress; reset to 0. Same / higher
                // count = stuck; increment. No measurement this
                // turn (no run_cargo test call) leaves the state
                // untouched.
                if let Some(cur) = this_turn_test_count {
                    let made_progress = matches!(last_test_failure_count, Some(prev) if cur < prev);
                    if made_progress {
                        no_progress_iters = 0;
                    } else {
                        no_progress_iters += 1;
                    }
                    last_test_failure_count = Some(cur);
                }

                // Bail if we've burned `max_auto_iters` consecutive
                // turns with no test-failure improvement. The
                // diagnostic embeds `max_auto_iters` so the
                // AutoHost wrapper's existing substring matcher
                // cancels the in-flight sub-session and the auto
                // driver flips to manual mode -- no separate
                // signal-path to keep in sync.
                if opts.auto
                    && this_turn_test_count.is_some()
                    && no_progress_iters >= opts.max_auto_iters
                {
                    let cur = last_test_failure_count.unwrap_or(0);
                    host.write(&Event::Diagnostic {
                        level: DiagnosticLevel::Error,
                        message: format!(
                            "auto: {} hit no-progress cap (max_auto_iters={}): {} consecutive \
                             cargo-test runs with the failure count not strictly decreasing \
                             (current: {} test(s) failing). The agent is iterating without \
                             measurable improvement; switching to interactive.",
                            step.id, opts.max_auto_iters, no_progress_iters, cur,
                        ),
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
                        host.write(&Event::BuildOutput {
                            command: out.command.clone(),
                            stdout_tail: out.stdout_tail.clone(),
                            stderr_tail: out.stderr_tail.clone(),
                            exit_code: out.exit_code,
                        })?;
                        if !out.ok() {
                            phase_iterations += 1;
                            if phase_iterations >= MAX_ITER_PER_PHASE {
                                host.write(&Event::Diagnostic {
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
                                });
                                continue;
                            }
                        } else {
                            // Phase succeeded; advance.
                            phase_iterations = 0;
                            phase_idx += 1;
                            if let Some(next) = phases.get(phase_idx) {
                                host.write(&Event::PhaseChanged {
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
                    let report = evaluate_structural_gate(&opts.project_dir, &step)?;
                    if report.is_clean() {
                        host.write(&Event::SessionEnd {
                            reason: SessionEndReason::Completed,
                            message: Some(format!("auto: {} structural gate clean", step.id)),
                        })?;
                        return Ok(());
                    }
                    auto_iterations += 1;
                    if auto_iterations >= opts.max_auto_iters {
                        host.write(&Event::Diagnostic {
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
                        });
                        continue; // Don't ask the user; agent retries.
                    }
                }
            }
        }

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
                let report = evaluate_structural_gate(&opts.project_dir, &step)?;
                if report.is_clean() {
                    host.write(&Event::SessionEnd {
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
                if let Some(walk) = step.milestone_walk
                    && session_produced_artifact
                {
                    let current = crate::__internal::steps::find_current_milestone(
                        &opts.project_dir,
                        &walk,
                        false,
                    );
                    use crate::__internal::steps::CurrentMilestone;
                    let milestone_done = match &current {
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
                    };
                    if milestone_done {
                        host.write(&Event::SessionEnd {
                            reason: SessionEndReason::Completed,
                            message: Some(format!(
                                "auto: {} milestone complete (no-artifact wind-down); critique will run",
                                step.id
                            )),
                        })?;
                        return Ok(());
                    }
                }
            }
            auto_iterations += 1;
            if auto_iterations >= opts.max_auto_iters {
                host.write(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "auto: {} exceeded max_auto_iters ({}) without producing an artifact; switching to interactive.",
                        step.id, opts.max_auto_iters
                    ),
                })?;
                // Fall through to RequestUserInput.
            } else {
                messages.push(LlmMessage {
                    role: LlmRole::User,
                    content: "You are in automated mode. Produce the artifact file(s) now using the artifact-write convention. Do not ask questions; decide using the inlined state and document your decisions in an `## Auto-decisions` section.".into(),
                    attachments: Vec::new(),
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
            host.write(&Event::Followup {
                label: "Retry".into(),
                action: "/retry".into(),
            })?;
            host.write(&Event::Followup {
                label: "Cancel".into(),
                action: "/end-session".into(),
            })?;
        }
        host.write(&Event::RequestUserInput {
            prompt: request_prompt,
            placeholder: request_placeholder,
        })?;
        match host.read()? {
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
                });
                empty_response_retries = 0;
            }
            Some(HostEvent::Cancel) | None => {
                host.write(&Event::SessionEnd {
                    reason: SessionEndReason::Cancelled,
                    message: None,
                })?;
                return Ok(());
            }
            Some(other) => {
                host.write(&Event::Diagnostic {
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
            host.write(&Event::SessionEnd {
                reason: SessionEndReason::Completed,
                message: None,
            })?;
            return Ok(());
        }
    }
}

/// Render a system message describing the fenced-block tool-call
/// fallback that backends without native tool-use can emit. Native
/// tool-use clients still see the same tools via the protocol's
/// `RequestLlmResponse.tools` field.
fn build_tool_notice(
    dispatcher: &[Box<dyn Tool>],
    library_root: Option<&Path>,
    framework_root: Option<&Path>,
    framework_docs_root: Option<&Path>,
    write_paths: &[String],
) -> String {
    let mut out = String::from("Tool catalog (orchestrator-mediated):\n\n");
    for t in dispatcher {
        out.push_str(&format!("- `{}` - {}\n", t.name(), t.description()));
    }
    if write_paths.is_empty() {
        out.push_str(
            "\nWrites are disabled in this session. `write_file`, `edit_file`, and the fenced artifact-write convention will all reject any path. Use the read-only tools to inspect state and report findings as text.\n",
        );
    } else {
        out.push_str(
            "\nWrite scope (per step + kind): the orchestrator only persists writes that match one of these project-relative prefixes (entries ending in `/` match any path under that directory; others must match exactly):\n",
        );
        for p in write_paths {
            out.push_str(&format!("- `{p}`\n"));
        }
        out.push_str(
            "Paths outside this list are rejected by `write_file`, `edit_file`, AND the fenced ` ```<path> ` artifact-write convention. If you have a strong reason to land work elsewhere, surface it in your reply rather than retrying with a different out-of-scope path.\n",
        );
    }
    if let Some(root) = library_root {
        out.push_str(&format!(
            "\nLibrary root (read-only, auto-detected): `{}`. Reads can target it by prefixing the path with `lib:`; for example `lib:docs/modeling-guide/01-quickstart.md` or `lib:examples/00-simple-pipeline/`. `list_dir` accepts a bare `lib:` to list the library root itself. `write_file` rejects `lib:` paths -- writes always land under the project directory.\n",
            root.display()
        ));
    } else {
        out.push_str(
            "\nNo library root detected. `lib:` reads will fail until a sim-models layout is found above the project dir.\n",
        );
    }
    if let Some(root) = framework_root {
        out.push_str(&format!(
            "\nFramework source root (read-only): `{}`. Reads can target it via the `fw:` prefix for source-level signatures and crate layout. Prefer the curated rustdoc under `fw:api/...` for API discovery; use `fw:src/prelude.rs` or individual `fw:src/...` files only when you need exact signatures or source examples. Treat the framework as a stable API -- do NOT browse internal helpers; if a behavior isn't in the prelude or a directly-re-exported module, ask rather than reverse-engineering it.\n",
            root.display()
        ));
    }
    if let Some(root) = framework_docs_root {
        out.push_str(&format!(
            "\nFramework API docs root (read-only): `{}`. A curated framework API TOC is provided separately in this prompt. Use that TOC to choose specific `fw:api/pages/...md` files, then read only those pages on demand.\n",
            root.display()
        ));
    }
    out.push_str(
        "\nNative tool-use is preferred; clients without it can emit a fenced block whose info-string is `tool:<name>` and whose body is the argument payload. Examples:\n\n```tool:read_file\nsrc/lib.rs\n```\n\n```tool:list_dir\nfw:\n```\n\n```tool:read_file\nfw:api/toc.md\n```\n\n```tool:read_file\nfw:api/pages/foundation_framework/prelude/index.md\n```\n\n```tool:read_file\nfw:src/prelude.rs\n```\n\n```tool:search\n{\"pattern\":\"ConnectivityPlan\",\"path\":\"fw:api/pages\"}\n```\n\nThe `edit_file` tool's fenced-block body is a JSON object (its three args -- `path`, `old_string`, `new_string` -- can be multi-line, so a JSON body is the only unambiguous form):\n\n```tool:edit_file\n{\"path\": \"spec.md\", \"old_string\": \"## Pipelining\", \"new_string\": \"## Pipelining and Hierarchy\"}\n```\n\n## Choosing between edit_file and the artifact-write convention\n\nPrefer `edit_file` for SMALL, TARGETED CHANGES against a file already on disk: rename a header, fix a typo, change a single value, add or delete a paragraph. `old_string` must appear EXACTLY ONCE in the current file -- include enough surrounding context to make the substring unique, and read the file first if you don't already have its current text in this turn. Use the artifact-write convention (full-file fenced block whose info-string is the path) only when creating a new file or when the change touches most of the file.\n\nThe orchestrator runs the tool, emits a `ToolInvoked` event for the host, and feeds the tool's output back as the next user message.",
    );
    out
}

/// Resolve the foundation framework crate root from
/// `<foundation_root>/crates/framework/`. Returns `None` if the
/// expected layout isn't present (e.g. the foundation_root override
/// points somewhere other than the canonical sim-foundation tree).
fn detect_framework_root(foundation_root: &Path) -> Option<std::path::PathBuf> {
    let candidate = foundation_root.join("crates").join("framework");
    if candidate.join("src").is_dir() {
        Some(candidate)
    } else {
        None
    }
}

fn detect_framework_docs_root(foundation_root: &Path) -> Option<std::path::PathBuf> {
    if let Some(candidate) = std::env::var_os(FRAMEWORK_DOCS_ROOT_ENV).map(PathBuf::from)
        && is_framework_docs_root(&candidate)
    {
        return Some(candidate);
    }
    let candidate = foundation_root
        .join("target")
        .join("sim-flow-vscode-api-docs");
    if is_framework_docs_root(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn is_framework_docs_root(candidate: &Path) -> bool {
    candidate.join("toc.md").is_file() && candidate.join("pages").is_dir()
}

/// Walk up from `project_dir` looking for a directory that contains
/// both `docs/modeling-guide/` and `examples/`. That layout matches
/// the sim-models repo we want the agent to reference. Returns the
/// first such ancestor (highest in the tree); `None` if nothing in the
/// chain matches.
fn detect_library_root(project_dir: &Path) -> Option<std::path::PathBuf> {
    let mut cursor = project_dir.to_path_buf();
    // Cap at 16 levels to avoid pathological infinite loops if the
    // canonical path resolution does anything weird.
    for _ in 0..16 {
        let docs = cursor.join("docs").join("modeling-guide");
        let examples = cursor.join("examples");
        if docs.is_dir() && examples.is_dir() {
            return Some(cursor);
        }
        if !cursor.pop() {
            break;
        }
    }
    None
}

fn invoke_tool(
    dispatcher: &[Box<dyn Tool>],
    ctx: &tools::ToolContext,
    call: &tools::ParsedToolCall,
) -> ToolResult {
    let tool = match dispatcher.iter().find(|t| t.name() == call.name) {
        Some(t) => t,
        None => {
            return ToolResult::err(format!(
                "tool `{}` is not available for this step",
                call.name
            ));
        }
    };
    let args = match tool_args_from_body(&call.name, &call.body) {
        Ok(v) => v,
        Err(msg) => return ToolResult::err(msg),
    };
    match tool.invoke(ctx, &args) {
        Ok(out) => out,
        Err(err) => ToolResult::err(format!("tool `{}` failed: {err}", call.name)),
    }
}

fn tool_args_from_body(name: &str, body: &str) -> std::result::Result<serde_json::Value, String> {
    // JSON body is the universal shape: backends with native tool-use
    // (LM Studio function calling, OpenAI tool_calls, Anthropic
    // tool_use) synthesize fenced blocks whose body is the call's
    // arguments JSON, and `edit_file`'s multi-line strings already
    // require it. If the body parses as a JSON object we use it
    // directly; otherwise we fall back to the per-tool line-based
    // form documented in the system-prompt examples.
    //
    // `write_file` accepts JSON args here too: the system prompt
    // still recommends the artifact-write convention (fenced block
    // whose info-string is the file path) because it round-trips
    // cleanly through fenced-block-only backends, but rejecting
    // `tool:write_file` outright deadlocks native-tool-calling
    // backends — they synthesize `tool:<name>` fences for every
    // function-call response, and an unrecoverable rejection sends
    // them into a runaway retry loop until `max_identical_responses`
    // fires.
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => Ok(value),
            Err(e) => Err(format!("{name}: failed to parse JSON args: {e}")),
        };
    }

    match name {
        "read_file" | "list_dir" => {
            let path = body
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());
            match path {
                Some(p) => Ok(serde_json::json!({ "path": p })),
                None => Err(format!(
                    "{name}: empty body; expected a path on the first line"
                )),
            }
        }
        "search" => {
            let mut iter = body.lines().filter(|l| !l.trim().is_empty());
            let pattern = iter.next().map(|l| l.trim().to_string());
            let path = iter.next().map(|l| l.trim().to_string());
            match pattern {
                Some(p) => match path {
                    Some(scope) => Ok(serde_json::json!({ "pattern": p, "path": scope })),
                    None => Ok(serde_json::json!({ "pattern": p })),
                },
                None => Err("search: empty body; expected a regex pattern".into()),
            }
        }
        "edit_file" => Err(
            "edit_file fenced fallback requires a JSON object body, e.g. \
             `{\"path\": \"foo.md\", \"old_string\": \"...\", \"new_string\": \"...\"}`. \
             Prefer native tool-use when the backend supports it."
                .into(),
        ),
        "write_file" => {
            // Permissive fallback: treat the fenced body as
            // "path on the first non-empty line, content as the
            // rest" so an agent that reaches for `tool:write_file`
            // (the natural function-call shape) doesn't get
            // bounced when the body isn't JSON-wrapped. The JSON
            // path above still works; this branch covers backends
            // that emit bare path + content lines.
            let mut lines = body.lines();
            let path = loop {
                match lines.next() {
                    Some(l) if !l.trim().is_empty() => break Some(l.trim().to_string()),
                    Some(_) => continue,
                    None => break None,
                }
            };
            let Some(path) = path else {
                return Err(write_file_help("empty body"));
            };
            // Drop the leading blank line(s) commonly written
            // between the path and the content block.
            let mut content_lines: Vec<&str> = lines.collect();
            while content_lines.first().is_some_and(|l| l.trim().is_empty()) {
                content_lines.remove(0);
            }
            if content_lines.is_empty() {
                return Err(write_file_help(&format!(
                    "missing file content for `{path}`"
                )));
            }
            let content = content_lines.join("\n");
            Ok(serde_json::json!({ "path": path, "content": content }))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Helper text rendered when a `tool:write_file` fenced call lacks
/// the path-then-content body. Includes a concrete artifact-write
/// example so the agent can recover in one read instead of
/// trial-and-error.
fn write_file_help(reason: &str) -> String {
    format!(
        "write_file: {reason}. The fenced-tool body must be \"path on \
         line 1, blank line, content below\". For multi-line content \
         prefer the artifact-write convention -- the fence info-string \
         is the file path and the body is the file content:\n\n\
         ```src/model/mod.rs\npub mod foo;\npub mod bar;\n```\n\n\
         Or pass JSON args directly: \
         `{{\"path\": \"<rel>\", \"content\": \"<text>\"}}`."
    )
}

fn tool_args_summary(call: &tools::ParsedToolCall) -> String {
    let line = call.body.lines().next().unwrap_or("").trim();
    if line.len() > 80 {
        format!("{}...", &line[..80])
    } else {
        line.to_string()
    }
}

/// Standard base64 (RFC 4648) encoder. Inlined to avoid pulling in
/// the `base64` crate just for the tool-attachment hand-off; we have
/// at most one or two image encodings per session.
fn base64_encode(input: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n =
            (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8) | u32::from(input[i + 2]);
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHA[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = u32::from(input[i]) << 16;
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8);
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

fn run_phase_validator(phase: &str, project_dir: &Path) -> Option<runners::RunnerOutput> {
    match phase {
        "build" => runners::cargo_check(project_dir).ok(),
        "test" => runners::cargo_test(project_dir, None).ok(),
        _ => None,
    }
}

pub(crate) fn step_descriptor_for_protocol(
    step: &StepDescriptor,
    kind: SessionKindOut,
    foundation_root: &Path,
) -> StepDescriptorOut {
    let suffix = match kind {
        SessionKindOut::Work => "",
        SessionKindOut::Critique => "-critique",
    };
    let path = foundation_root
        .join(crate::prompts::PROMPTS_DIR)
        .join(format!("{}{}.md", step.instruction_slug, suffix));
    let (phases, tool_names) = match kind {
        SessionKindOut::Work => (step.work_phases, crate::steps::UNIVERSAL_TOOLS),
        SessionKindOut::Critique => (step.critique_phases, crate::steps::UNIVERSAL_TOOLS),
    };
    StepDescriptorOut {
        step: step.id.into(),
        kind,
        flow: step.flow.as_str().into(),
        prerequisite: step.prerequisite.map(String::from),
        instruction_path: path.display().to_string(),
        work_artifacts: step.work_artifacts.iter().map(|s| (*s).into()).collect(),
        predecessor_inputs: step
            .predecessor_inputs
            .iter()
            .map(|s| (*s).into())
            .collect(),
        per_candidate: step.per_candidate,
        phases: phases.iter().map(|s| (*s).into()).collect(),
        tools: tool_names.iter().map(|s| (*s).into()).collect(),
    }
}

// ---------------------------------------------------------------------
// Public message-building entry point used by both the JSONL turn loop
// and the interactive PTY driver. Produces the exact stack of system /
// user messages the orchestrator would otherwise assemble inline at
// the start of `run_session_inner`, plus the advertised tool catalog.
// ---------------------------------------------------------------------

/// What `build_initial_messages` returns: the full message stack ready
/// to ship to an LLM (or render into a single prompt for an
/// interactive session) plus the tool catalog for backends with
/// native tool-use.
pub struct MessageBundle {
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<LlmTool>,
}

pub fn build_initial_messages(
    opts: &OrchestratorOptions,
    step: &StepDescriptor,
) -> Result<MessageBundle> {
    let tool_names: &[&'static str] = crate::steps::UNIVERSAL_TOOLS;
    let dispatcher = tools::build_dispatcher(tool_names);
    let library_root = detect_library_root(&opts.project_dir);
    let framework_root = detect_framework_root(&opts.foundation_root);
    let framework_docs_root = detect_framework_docs_root(&opts.foundation_root);
    let llm_tools: Vec<LlmTool> = dispatcher
        .iter()
        .map(|t| LlmTool {
            name: t.name().into(),
            description: t.description().into(),
            args_schema: t.args_schema(),
        })
        .collect();

    let instruction_body = prompts::load_for_project(
        &opts.foundation_root,
        &opts.project_dir,
        step.instruction_slug,
        opts.kind,
    )?;
    let mut messages: Vec<LlmMessage> = Vec::new();
    // Boilerplate "conventions" (artifact-write rules, automated-mode
    // notes) live as files under `<foundation>/<PROMPTS_DIR>/_conventions/`.
    // Two delivery shapes:
    //   - Native-tools agents (interactive `claude` / `codex` /
    //     `gh-copilot`) get a thin bootstrap directive that names the
    //     absolute path; the agent's own Read tool fetches the body.
    //     Skipping the inline keeps a multi-thousand-character paste
    //     out of the PTY (paste-detection / ECHO / double-newline
    //     pain). Step-specific instructions stay inlined since
    //     they're small and we want them guaranteed in context.
    //   - JSONL hosts (no native Read) keep inlining; the orchestrator
    //     loads the same file from disk so the wording is single-
    //     source-of-truth.
    let convention_name = if opts.agent_has_native_fs_tools {
        "native-tools"
    } else {
        "fenced-blocks"
    };
    // Mode-notes: always inject a positive signal for the current
    // mode (not the absence of the other one). Earlier we relied on
    // "auto-mode notes get loaded only when auto, the step prompt
    // does a self-check on the literal string." Weaker models
    // (qwen3-coder etc.) couldn't tell that a backtick-quoted
    // pattern reference was different from an active assertion, and
    // happily proceeded as if auto mode were on. Loading both
    // conventions side-by-side -- one per branch -- gives every
    // model an unambiguous "MANUAL mode is ACTIVE" / "AUTOMATED mode
    // is ACTIVE" anchor.
    let mode_notes_name = if opts.auto {
        "auto-mode"
    } else {
        "manual-mode"
    };
    let mode_notes_label = if opts.auto {
        "automated-mode notes"
    } else {
        "manual-mode notes"
    };
    let combined_system = if opts.agent_has_native_fs_tools {
        let mut directives = format!(
            "Before responding, read the conventions file at:\n\n  {}\n\n\
             Treat its content as a system instruction that applies for\n\
             the rest of this session. The file is short (read it in full).\
             \n\nAlso read the {} at:\n\n  {}\n\nFollow them on every turn.\
             \n\nAlso read the no-emojis convention at:\n\n  {}\n\n\
             ASCII only -- no decorative glyphs in files, tool args, or chat replies.",
            prompts::convention_path(&opts.foundation_root, convention_name).display(),
            mode_notes_label,
            prompts::convention_path(&opts.foundation_root, mode_notes_name).display(),
            prompts::convention_path(&opts.foundation_root, "no-emojis").display(),
        );
        if opts.no_preamble {
            directives.push_str(&format!(
                "\n\nAlso read the response-shape convention at:\n\n  {}\n\n\
                 Tool calls first, prose last. No recap, no hedging, no preamble.",
                prompts::convention_path(&opts.foundation_root, "no-preamble").display(),
            ));
        }
        format!("{}\n\n---\n\n{}", directives, instruction_body)
    } else {
        let convention = prompts::load_convention(&opts.foundation_root, convention_name)?;
        let mode_notes = prompts::load_convention(&opts.foundation_root, mode_notes_name)?;
        let no_emojis = prompts::load_convention(&opts.foundation_root, "no-emojis")?;
        let mut combined = format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n",
            convention, mode_notes, no_emojis,
        );
        if opts.no_preamble {
            let no_preamble = prompts::load_convention(&opts.foundation_root, "no-preamble")?;
            combined.push_str(&no_preamble);
            combined.push_str("\n\n---\n\n");
        }
        combined.push_str(&instruction_body);
        combined
    };
    messages.push(LlmMessage {
        role: LlmRole::System,
        content: combined_system,
        attachments: Vec::new(),
    });
    if !llm_tools.is_empty() {
        let write_paths = crate::steps::allowed_write_paths(step, opts.kind);
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: build_tool_notice(
                &dispatcher,
                library_root.as_deref(),
                framework_root.as_deref(),
                framework_docs_root.as_deref(),
                &write_paths,
            ),
            attachments: Vec::new(),
        });
    }
    // Stable-first ordering: project-stable TOCs (spec, framework
    // API), then per-step stable inputs, then per-milestone /
    // per-retry volatile inputs. vLLM's KV prefix cache reuses every
    // token-identical message at the head of the request, so anything
    // that changes between dispatches (current milestone, prior
    // critique body) goes LAST so the long stable head stays cached
    // across milestone advances and critique retries within a step.
    if let Some(toc) = build_spec_toc_message(&opts.project_dir) {
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: toc,
            attachments: Vec::new(),
        });
    }
    if let Some(root) = framework_docs_root.as_deref()
        && let Some(toc) = build_framework_api_toc_message(root)
    {
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: toc,
            attachments: Vec::new(),
        });
    }
    if let Some(inputs) = build_session_inputs(&opts.project_dir, step, opts.kind) {
        messages.push(LlmMessage {
            role: LlmRole::System,
            content: inputs.stable,
            attachments: Vec::new(),
        });
        if let Some(volatile) = inputs.volatile {
            messages.push(LlmMessage {
                role: LlmRole::System,
                content: volatile,
                attachments: Vec::new(),
            });
        }
    }
    let opening = initial_user_prompt(step.id, opts.kind, &expected_output_paths(step, opts.kind));
    messages.push(LlmMessage {
        role: LlmRole::User,
        content: opening,
        attachments: Vec::new(),
    });

    Ok(MessageBundle {
        messages,
        tools: llm_tools,
    })
}

// ---------------------------------------------------------------------
// Helpers shared with the (now-deleted) TS implementation. Behavioral
// parity with `extensions/sim-flow-vscode/src/participant/artifacts.ts`
// and `handlers.ts::initialUserPrompt` / `buildCritiqueInputs`.
// ---------------------------------------------------------------------

/// Evaluate the step's gate but skip the `CritiqueClean` checks.
/// Used by auto-mode work sessions to decide whether the structural
/// part of the gate is clean -- the critique-clean part can only
/// pass after the separate critique session runs.
fn evaluate_structural_gate(project_dir: &Path, step: &StepDescriptor) -> Result<GateReport> {
    let filtered: Vec<GateCheck> = step
        .gate_checks
        .iter()
        .filter(|c| !matches!(c, GateCheck::CritiqueClean { .. }))
        .cloned()
        .collect();
    gate::evaluate(project_dir, &filtered)
}

/// Heuristic: did this turn's response contain any artifact-write
/// fenced block? Used to detect "agent is stalling without producing
/// output" turns in auto mode.
/// Mirror of the critique-session fallback in `run_session`: returns
/// false (i.e. "an artifact was produced") whenever a fenced
/// artifact-write block extracted OR the session is a critique with
/// substantive body content and no tool calls. Used at the
/// auto-iteration cap check so a turn that wrote the critique file
/// via the fallback doesn't get counted as "no artifact" and re-
/// trigger the cap.
fn effective_artifacts_empty(response_text: &str, kind: SessionKind) -> bool {
    if !extract_artifacts(response_text).is_empty() {
        return false;
    }
    if kind == SessionKind::Critique
        && tools::extract_tool_calls(response_text).is_empty()
        && !response_text.trim().is_empty()
    {
        return false;
    }
    true
}

// AUTO_MODE_SYSTEM, ARTIFACT_CONVENTION_SYSTEM, and NATIVE_FS_TOOLS_SYSTEM
// used to live here as `concat!` strings. They were extracted to
// `<foundation>/tools/sim-flow/prompts/_conventions/{auto-mode,
// manual-mode,fenced-blocks,native-tools,no-emojis,no-preamble}.md` so:
//   - PTY agents that have a Read tool can fetch them on demand
//     instead of having a multi-thousand-character paste shoved into
//     stdin (avoiding paste-detection / ECHO / newline doubling).
//   - JSONL hosts still inline them, but via runtime read so there's
//     a single source of truth for the wording.
// `prompts::load_convention(foundation_root, name)` is the loader;
// `build_initial_messages` chooses inline vs reference-by-path based
// on `OrchestratorOptions::agent_has_native_fs_tools`.

fn expected_output_paths(step: &StepDescriptor, kind: SessionKind) -> Vec<String> {
    match kind {
        SessionKind::Work => step.work_artifacts.iter().map(|s| (*s).into()).collect(),
        SessionKind::Critique => vec![format!("docs/critiques/{}-critique.md", step.id)],
    }
}

fn initial_user_prompt(step_id: &str, kind: SessionKind, paths: &[String]) -> String {
    let mut out = String::new();
    match kind {
        SessionKind::Work => {
            out.push_str(&format!(
                "Begin the {step_id} work session now. The TOC above lists this step's predecessor inputs and target artifacts (path + size only); fetch any of them with `read_file` before you make claims about their content. Your VERY FIRST RESPONSE must contain:\n\n\
                 1. The `read_file` tool calls you need to inspect target artifacts that are already on disk and any predecessor inputs that aren't yet covered by the inlined critique below; OR, if you've already read everything you need (e.g. a small step with only a critique inlined), one short sentence stating what each target artifact currently contains.\n\
                 2. Either:\n\
                    a. A bulleted list of what is still missing relative to the instructions / gate checks, followed by ONE concrete question for me about the most important missing item; OR\n\
                    b. The single line `All required content appears present - run /advance to gate-check.` if every item the instructions require is already covered.\n\n\
                 Do not return an empty response. Do not wait for further prompting. Once you've read what you need, emit the artifact file(s) using the artifact-write convention -- or `edit_file` for targeted fixes -- as soon as you have enough content to save.",
            ));
        }
        SessionKind::Critique => {
            out.push_str(&format!(
                "Begin the {step_id} critique now. The TOC above lists this step's predecessor inputs and target artifacts (path + size only); fetch them with `read_file` before critiquing -- the content is NOT inlined. Your VERY FIRST RESPONSE must contain all three of:\n\n\
                 1. The `read_file` tool calls you need to inspect each target artifact and any predecessor input you'll cite; OR, once you've already read what you need this turn, a one-sentence summary of what the step's artifacts cover.\n\
                 2. A bulleted list of concrete issues you would flag relative to the step instructions and gate checks.\n\
                 3. The artifact-write block for the critique file as specified by the instructions.\n\n\
                 Do not wait for further prompting; read what you need then emit the critique.",
            ));
        }
    }
    if !paths.is_empty() {
        out.push_str(
            "\n\nWrite these files using the artifact-write convention (fenced block with the path as the info-string):\n\n",
        );
        for p in paths {
            out.push_str(&format!("- `{p}`\n"));
        }
        out.push_str("\nUse those exact paths - do NOT invent new filenames.");
    }
    out
}

/// If a spec was ingested into this project (`.sim-flow/source-spec-toc.md`
/// exists), return its TOC inlined as a system message. The agent
/// uses the TOC to decide which `spec-pages/<NNN>.md` files to fetch
/// via `read_file` / `search`. Per Phase 4 design we never inline
/// the full spec body -- specs can be hundreds of pages.
fn build_spec_toc_message(project_dir: &Path) -> Option<String> {
    let toc_path = project_dir.join(".sim-flow/source-spec-toc.md");
    let body = std::fs::read_to_string(&toc_path).ok()?;
    Some(format!(
        "Source spec is available. Use `read_file` / `search` against \
         `.sim-flow/spec-pages/<NNN>.md` to read individual pages on demand. \
         Do NOT request the full spec at once; consult the TOC below and \
         fetch only what you need.\n\n{body}"
    ))
}

/// If normalized framework API docs are available, return the bundled
/// TOC as a system message. The TOC points at `fw:api/pages/...` files
/// so the agent fetches only the specific API pages it needs.
fn build_framework_api_toc_message(framework_docs_root: &Path) -> Option<String> {
    let body = std::fs::read_to_string(framework_docs_root.join("toc.md")).ok()?;
    Some(format!(
        "Framework API docs are available under the `fw:api/` prefix. \
         Do NOT read the full API surface at once. Read the TOC below, then fetch only the \
         specific `fw:api/pages/...` files you need.\n\n{body}"
    ))
}

/// Split form of the per-session inputs message. `stable` is the
/// preamble + predecessor / work-artifact / plan-index TOC that does
/// NOT change across milestones or critique retries within a step.
/// `volatile` is the milestone-scope preamble + current-milestone TOC
/// entry + inlined critique body -- everything that DOES change. The
/// caller emits them as TWO separate System messages so vLLM's prefix
/// cache can reuse the long stable prefix across dispatches; without
/// the split the volatile tail invalidates the cache from the first
/// turn onward and the model re-encodes the entire input each time.
struct SessionInputs {
    stable: String,
    volatile: Option<String>,
}

fn build_session_inputs(
    project_dir: &Path,
    step: &StepDescriptor,
    kind: SessionKind,
) -> Option<SessionInputs> {
    // Predecessors and this step's existing artifacts are listed as
    // a TOC (path + size) -- the agent fetches their content via
    // `read_file` on demand. This avoids burning tokens re-inlining
    // every predecessor on every turn of a long iteration loop. Two
    // exceptions that ARE inlined verbatim because they're the
    // immediate context the agent must act on:
    //
    //   - the active <step>-critique.md file on a work re-run
    //     (the findings the agent must address this turn);
    //   - the same file on a CRITIQUE re-run, to scope the second
    //     pass to "are the prior BLOCKERs resolved?" instead of
    //     repeating the full structural-question evaluation. Without
    //     this the second-pass critique re-derives every question
    //     from scratch and weaker models routinely flag new blockers
    //     that didn't exist in the first pass, blowing the
    //     critique-iteration budget.
    let critique_rel = format!("docs/critiques/{}-critique.md", step.id);
    let critique_abs = project_dir.join(&critique_rel);
    // Read the rendered markdown body for legacy fallback / first-
    // pass critique inlining; the JSON sibling (when present) is
    // the source of truth for the BLOCKER list. `critique_body` is
    // None when neither artifact is on disk yet.
    let critique_body = std::fs::read_to_string(&critique_abs).ok();
    // Critique-retry detection: the file exists AND it's a critique
    // session AND we're not on the first pass. The first-pass test
    // is "no BLOCKER findings at all" -- a fresh critique file from
    // a prior run that already evaluated cleanly wouldn't have any.
    // We guard on BLOCKER presence so a previously-clean critique
    // doesn't suppress the full evaluation when the agent
    // legitimately needs it (e.g. the work session was edited
    // externally between runs).
    let prior_critique_blockers = retry_blocker_blocks(project_dir, step.id);
    let is_critique_retry = kind == SessionKind::Critique && !prior_critique_blockers.is_empty();
    let inline_critique = kind == SessionKind::Work || is_critique_retry;

    // Milestone-walk scoping. When a step's descriptor binds it to
    // a milestone-walk (DM2d, DM3b, DM3c, DM4b), the orchestrator
    // shows the agent ONLY the current milestone file plus the
    // plan's index, not the whole milestone directory. The
    // auto-driver iterates work-then-critique sessions; each
    // iteration the orchestrator picks the right milestone (same
    // one for retry, next pending one for advance). Without this
    // scoping the agent sees every milestone file at once and
    // chains them in a single work session, defeating the
    // per-milestone critique pattern.
    //
    // The current-milestone choice depends on session kind AND
    // retry state:
    //
    // - **Work, fresh advance** (no prior BLOCKERs): scope to the
    //   FIRST pending milestone -- the next slice of work.
    // - **Work, retry** (prior BLOCKERs): scope to the
    //   HIGHEST-numbered already-touched milestone -- the same
    //   milestone the Work session was on when the critique
    //   raised the BLOCKERs.
    // - **Critique** (any state): scope to the HIGHEST-numbered
    //   already-touched milestone -- the one the Work session
    //   JUST finished. Without this, a fresh-advance critique
    //   after milestone-N's Work would scope to milestone-(N+1)
    //   (the new "first pending") and the agent would critique
    //   the wrong milestone -- exactly the bug observed where
    //   DM3b's first critique reviewed an empty milestone-02
    //   instead of the milestone-01 work it should have evaluated.
    let milestone_scope: Option<String> = match step.milestone_walk {
        Some(walk) => {
            let pick_touched = match kind {
                SessionKind::Critique => true,
                SessionKind::Work => !prior_critique_blockers.is_empty(),
            };
            match crate::__internal::steps::find_current_milestone(project_dir, &walk, pick_touched)
            {
                crate::__internal::steps::CurrentMilestone::File(rel) => Some(rel),
                // AllResolved / NoMilestonesPresent: don't inject a
                // milestone scope. The structural gate
                // (MilestonesAllResolved) decides whether the step
                // can advance.
                _ => None,
            }
        }
        None => None,
    };

    // Two TOC buckets so volatile entries (the current milestone file)
    // can be emitted in a separate System message after the stable
    // ones, lengthening the prefix vLLM's KV cache can reuse across
    // milestone advances and critique retries within a step.
    let mut stable_toc: Vec<TocEntry> = Vec::new();
    let mut volatile_toc: Vec<TocEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_stable = |rel: &str, seen: &mut std::collections::HashSet<String>| {
        if seen.insert(rel.to_string()) {
            stable_toc.push(toc_entry_for(project_dir, rel));
        }
    };
    if let Some(walk) = step.milestone_walk {
        // Keep predecessor inputs / work_artifacts that are NOT
        // inside the milestone directory (e.g. docs/spec.md,
        // docs/testbench.md, src/, tests/), drop everything that
        // points at or into walk.dir, then explicitly add the
        // plan index in the stable bucket. The current milestone
        // file goes into the volatile bucket below. The agent never
        // sees other milestone files exist (beyond any TOC inside
        // the index).
        let walk_dir = walk.dir.trim_end_matches('/');
        let inside_walk = |rel: &str| {
            let r = rel.trim_end_matches('/');
            r == walk_dir || r.starts_with(&format!("{walk_dir}/"))
        };
        for rel in step.predecessor_inputs {
            if !inside_walk(rel) {
                push_stable(rel, &mut seen);
            }
        }
        for rel in step.work_artifacts {
            if !inside_walk(rel) {
                push_stable(rel, &mut seen);
            }
        }
        push_stable(walk.index_file, &mut seen);
    } else {
        for rel in step.predecessor_inputs {
            push_stable(rel, &mut seen);
        }
        for rel in step.work_artifacts {
            push_stable(rel, &mut seen);
        }
    }
    if let Some(milestone_rel) = &milestone_scope
        && seen.insert(milestone_rel.clone())
    {
        volatile_toc.push(toc_entry_for(project_dir, milestone_rel));
    }
    // Only TOC the critique file when its body exists. The OLD
    // build always added a "(not yet on disk)" entry for the
    // critique file even on a fresh Work session, which both
    // misled the Work agent into thinking it should write the
    // critique file AND added a synthetic volatile message that
    // broke prefix caching across the very first dispatch.
    if inline_critique && critique_body.is_some() && seen.insert(critique_rel.clone()) {
        volatile_toc.push(toc_entry_for(project_dir, &critique_rel));
    }

    if stable_toc.is_empty() && volatile_toc.is_empty() && !inline_critique {
        return None;
    }

    let mut stable = format!(
        "Step `{}` inputs and target artifacts. File entries show path + size; \
         directory entries are expanded one level so you can see what's actually \
         on disk WITHOUT calling `list_dir`. Use `read_file` to fetch file content \
         on demand; do NOT assume a file's content is inlined here, and do NOT \
         claim a directory is empty unless its expansion below is empty.\n\n",
        step.id
    );
    for entry in &stable_toc {
        stable.push_str(&entry.render_block(project_dir));
    }

    let mut volatile = String::new();
    // Milestone-scope preamble. The agent's prompt already mentions
    // milestone walking, but the orchestrator-injected preamble
    // makes the CURRENT milestone unambiguous and tells the agent
    // not to read or write any sibling milestone file -- the
    // structural enforcement that the prompt-only "STOP after each
    // milestone" instruction failed to deliver in earlier runs.
    if let (Some(walk), Some(milestone_rel)) = (step.milestone_walk, &milestone_scope) {
        let session_label = match kind {
            SessionKind::Work => "work",
            SessionKind::Critique => "critique",
        };
        // Resolution criterion + sibling-protection wording differ
        // between execution-mode walks (DM2d / DM3b / DM3c / DM4b)
        // and planning-detail walks (DM2cd / DM3ad / DM4ad). The
        // execution-mode prompt talks about `- [ ]` rows resolving;
        // the detail-mode prompt talks about replacing the
        // outline's stub with a full task list.
        let prefix_pattern = walk.file_prefixes.to_vec().join("` / `");
        let resolution_clause = if let Some(marker) = walk.placeholder_marker {
            format!(
                "When the placeholder marker (`{marker}`) is gone from the current \
                 milestone -- meaning you've replaced the stub with a full task \
                 list per the format specified by your prompt"
            )
        } else {
            "When all `- [ ]` rows in the current milestone are resolved \
             (`- [x]` done OR `- [-]` deferred with a `defer reason:` sub-bullet)"
                .to_string()
        };
        volatile.push_str(&format!(
            "**Milestone scope (orchestrator-enforced)**: this {session_label} \
             session targets EXACTLY ONE milestone -- `{milestone_rel}`. The plan \
             index `{}` is also inlined for context. You MUST NOT read or modify \
             any other `{prefix_pattern}<NN>-*.md` file in this session; sibling \
             milestones are intentionally hidden so each gets its own focused \
             critique. {resolution_clause}, stop and surface the canonical \
             `<milestone-name> complete; ready for critique.` notice. The \
             auto-driver will run the paired critique, then re-launch a \
             fresh session for the next milestone.\n\n",
            walk.index_file,
        ));
    }
    for entry in &volatile_toc {
        volatile.push_str(&entry.render_block(project_dir));
    }

    if inline_critique && (critique_body.is_some() || is_critique_retry) {
        volatile.push_str("\n---\n\n");
        if is_critique_retry {
            // `prior_critique_blockers` was already JSON-first
            // resolved at the top of the function; reuse it so the
            // inline blocks match the count the gate / auto driver
            // see.
            let blocks = &prior_critique_blockers;
            volatile.push_str(&format!(
                "Critique-retry mode. The prior pass flagged the BLOCKER(s) below; \
                 the work session has since re-run. Your task on THIS pass is FOCUSED:\n\n\
                 - For each prior BLOCKER, decide whether the work session's updated \
                   artifact resolves it. Quote the gap from the prior block if it is \
                   still applicable so the resolution is traceable.\n\
                 - Write the new critique fresh: emit `RESOLVED:` / `BLOCKER:` / \
                   `UNRESOLVED:` lines for the items below. Do NOT carry over the \
                   prior pass's RESOLVED / UNRESOLVED items verbatim -- those have \
                   been intentionally OMITTED from this context to keep your scope \
                   tight. They were closed in the prior pass; only re-flag if the \
                   new work introduced a regression.\n\
                 - Do NOT re-derive the full structural evaluation. Do NOT raise NEW \
                   `BLOCKER:` items unless the work session introduced a fresh \
                   problem (e.g. broke a previously-clean section). New `UNRESOLVED:` \
                   items surfaced by this turn's changes are fine.\n\n\
                 Prior BLOCKER(s) ({}) to re-evaluate:\n\n",
                blocks.len(),
            ));
            const RETRY_BLOCK_CAP: usize = 4_000;
            for (i, block) in blocks.iter().enumerate() {
                volatile.push_str(&format!(
                    "#### Prior BLOCKER {} of {}\n\n",
                    i + 1,
                    blocks.len()
                ));
                if block.len() <= RETRY_BLOCK_CAP {
                    volatile.push_str(block);
                } else {
                    // Surface truncation explicitly so the agent
                    // doesn't silently fix the wrong part of the
                    // BLOCKER, and log a metric so the cap can be
                    // raised if it bites recurrently.
                    tracing::warn!(
                        target: "sim_flow::metrics",
                        event = "critique_retry_block_truncated",
                        step = step.id,
                        block_index = i,
                        block_bytes = block.len(),
                        cap_bytes = RETRY_BLOCK_CAP,
                    );
                    volatile.push_str(&block[..RETRY_BLOCK_CAP]);
                    volatile.push_str(&format!(
                        "\n\n... [truncated to {RETRY_BLOCK_CAP} chars; original was {} chars. \
                         The full BLOCKER body is in the prior critique file -- \
                         re-read `{critique_rel}` if you need the tail.]",
                        block.len(),
                    ));
                }
                volatile.push_str("\n\n");
            }
        } else if let Some(body) = &critique_body {
            volatile.push_str(
                "Latest critique for this step (inlined because addressing these findings is your immediate task):\n\n",
            );
            volatile.push_str(&format!(
                "### `{critique_rel}`\n\n{}",
                truncate(body, 16_000),
            ));
        }
    }

    // Inline the orchestrator's most recent post-Work cargo report
    // (fmt-check + clippy) into the Critique session input. Lives at
    // `.sim-flow/cargo-checks-{step}.md` and gets overwritten each
    // milestone advance; the Critique now sees authoritative cargo
    // state instead of guessing from the Work transcript. Skip on
    // Work sessions -- Work writes the code, then the orchestrator
    // runs the checks AFTER, so Work has nothing fresh to read.
    if kind == SessionKind::Critique {
        let cargo_report_rel = format!(".sim-flow/cargo-checks-{}.md", step.id);
        let cargo_report_abs = project_dir.join(&cargo_report_rel);
        if let Ok(report_body) = std::fs::read_to_string(&cargo_report_abs) {
            volatile.push_str("\n---\n\n");
            volatile.push_str(&report_body);
        }
    }

    let volatile = if volatile.is_empty() {
        None
    } else {
        Some(volatile)
    };
    Some(SessionInputs { stable, volatile })
}

/// JSON-first blocker extractor for the retry-inline path. When
/// `<step>-critique.json` exists, parse it and return one
/// formatted block per `kind == blocker` finding (header line +
/// body, mirroring the markdown shape so the agent's retry context
/// reads naturally). Falls back to the legacy markdown regex
/// (`extract_blocker_blocks`) when no JSON sibling is on disk so
/// projects mid-flight before the migration keep working.
fn retry_blocker_blocks(project_dir: &Path, step_id: &str) -> Vec<String> {
    let json_rel = format!("docs/critiques/{step_id}-critique.json");
    let json_abs = project_dir.join(&json_rel);
    if let Ok(text) = std::fs::read_to_string(&json_abs)
        && let Ok(parsed) = serde_json::from_str::<crate::critique::CritiqueJson>(&text)
    {
        return parsed
            .findings
            .iter()
            .filter(|f| f.kind == crate::critique::FindingKind::Blocker)
            .map(|f| {
                if f.body.trim().is_empty() {
                    format!("**BLOCKER: {}**", f.title.trim())
                } else {
                    format!("**BLOCKER: {}**\n\n{}", f.title.trim(), f.body.trim())
                }
            })
            .collect();
    }
    let md_abs = project_dir.join(format!("docs/critiques/{step_id}-critique.md"));
    let body = std::fs::read_to_string(&md_abs).unwrap_or_default();
    extract_blocker_blocks(&body)
}

/// Pull each `BLOCKER:` block out of a critique markdown file as a
/// MULTI-LINE string covering the line that opens with `BLOCKER:`
/// (after stripping list-markers / bold) plus every following line
/// until the next finding marker (`BLOCKER:` / `UNRESOLVED:` /
/// `RESOLVED:`), a markdown heading, a horizontal rule, or EOF. The
/// header line is included so the agent sees the prefix verbatim;
/// sub-bullets and explanatory prose that follow stay attached.
///
/// `extract_blocker_blocks().len()` is the gate-relevant count of
/// blockers and replaces the older single-line `parse_blocker_lines`
/// helper. Whole blocks (rather than just header lines) are what we
/// inline into a focused critique-retry: a multi-bullet BLOCKER
/// describing three sub-gaps loses all the actionable detail if
/// only the first line survives.
fn extract_blocker_blocks(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if line_kind(lines[i]) == Some(FindingKind::Blocker) {
            let start = i;
            let mut j = i + 1;
            while j < lines.len() && !is_block_terminator(lines[j]) {
                j += 1;
            }
            // Trim trailing blank lines so blocks read cleanly when
            // joined back together.
            let mut end = j;
            while end > start + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            out.push(lines[start..end].join("\n"));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FindingKind {
    Blocker,
    Unresolved,
    Resolved,
}

/// Match a finding marker at the start of a line. The recognized
/// shapes (after lenient prefix-stripping) are:
///
/// - `BLOCKER:` / `BLOCKERS:` / case-variants
/// - `UNRESOLVED:` / `UNRESOLVEDS:` / case-variants
/// - `RESOLVED:` / `RESOLVEDS:` / case-variants
///
/// The leading prefix-strip allows: list markers (`-`, `*`, `+`),
/// markdown headings (`#`+), whitespace, bold/underline (`**` /
/// `__`), and one stray non-alphanumeric "decoration" character
/// (emoji like `❌`, dingbats, checkmarks). Today's qwen run emitted
/// `### ❌ BLOCKER: ...` as a heading-with-emoji and the prior
/// strict-list-only matcher silently passed the gate; this is the
/// fix.
static FINDING_MARKER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        // MUST stay in sync with `__internal/critique.rs::FINDING_MARKER_RE`.
        // See that comment for the prefix ordering rationale.
        r"^[\s\-\*\+#>]*(?:\d+\.\s+)?(?:\*\*|__)?\s*[^\w\s]*\s*(?P<kind>(?i)blockers?|unresolveds?|resolveds?):"
    )
    .expect("finding-marker regex compiles")
});

fn line_kind(line: &str) -> Option<FindingKind> {
    let m = FINDING_MARKER_RE.captures(line)?;
    let kind = m.name("kind")?.as_str().to_ascii_lowercase();
    if kind.starts_with("blocker") {
        Some(FindingKind::Blocker)
    } else if kind.starts_with("unresolved") {
        Some(FindingKind::Unresolved)
    } else if kind.starts_with("resolved") {
        Some(FindingKind::Resolved)
    } else {
        None
    }
}

fn is_block_terminator(line: &str) -> bool {
    if line_kind(line).is_some() {
        return true;
    }
    // A heading is a terminator unless `line_kind` already claimed
    // it as a finding (handled above): `### Section header` ends a
    // prior block; `### ❌ BLOCKER: ...` IS a block-start, not a
    // terminator.
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return true;
    }
    let only_dashes = line.trim();
    if (only_dashes.starts_with("---") || only_dashes.starts_with("***"))
        && only_dashes
            .chars()
            .all(|c| c == '-' || c == '*' || c == ' ')
    {
        return true;
    }
    false
}

/// Backwards-compatible single-line view: each entry is the
/// `BLOCKER:` header line (without its body) for callers that just
/// want a count or a one-line summary. Internally implemented on
/// top of `extract_blocker_blocks` so both helpers agree.
fn parse_blocker_lines(body: &str) -> Vec<String> {
    extract_blocker_blocks(body)
        .iter()
        .filter_map(|block| block.lines().next().map(String::from))
        .collect()
}

struct TocEntry {
    rel: String,
    state: TocState,
}

enum TocState {
    Directory,
    File {
        bytes: u64,
    },
    /// Small file whose contents are inlined directly into the
    /// session inputs message so the agent doesn't have to spend a
    /// `read_file` tool turn fetching it. Used for predecessor
    /// inputs whose body fits under
    /// `SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES` (default 4096).
    /// Eliminates 5-10 turns of overhead per Critique on a typical
    /// step that reads spec.md / decomposition.md / data-movement.md
    /// / etc.
    InlinedFile {
        bytes: u64,
        body: String,
    },
    Missing,
}

impl TocEntry {
    /// Render this entry as one or more bullet lines. Directories
    /// expand one level deep so the model can SEE the file list and
    /// can't hallucinate "empty"; nested directories are still
    /// summarized as `(directory, N entries)` so the prompt doesn't
    /// recurse without bound. Small files are inlined as fenced code
    /// blocks so the agent can read them without a tool turn.
    fn render_block(&self, project_dir: &Path) -> String {
        match &self.state {
            TocState::Directory => render_directory_block(project_dir, &self.rel),
            TocState::File { bytes } => format!("- `{}` ({} bytes)\n", self.rel, bytes),
            TocState::InlinedFile { bytes, body } => {
                let lang = inline_lang_hint(&self.rel);
                format!(
                    "- `{}` ({} bytes, inlined below):\n\n```{}\n{}\n```\n\n",
                    self.rel,
                    bytes,
                    lang,
                    body.trim_end()
                )
            }
            TocState::Missing => format!("- `{}` (not yet on disk)\n", self.rel),
        }
    }
}

/// Pick a fenced-block language hint from a path. Markdown stays
/// markdown so nested fences don't break the agent's parser; Rust
/// gets `rust`; everything else falls back to a generic `text`
/// fence which is safe for arbitrary content.
fn inline_lang_hint(rel: &str) -> &'static str {
    match std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some(ext) if ext.eq_ignore_ascii_case("md") => "markdown",
        Some(ext) if ext.eq_ignore_ascii_case("rs") => "rust",
        Some(ext) if ext.eq_ignore_ascii_case("toml") => "toml",
        Some(ext) if ext.eq_ignore_ascii_case("json") => "json",
        _ => "text",
    }
}

/// Per-file threshold below which `toc_entry_for` inlines the body.
/// 4 KB ~= 1K tokens. Across 8 predecessor inputs adds ~8K tokens
/// to the stable prefix but saves 5-10 `read_file` tool turns per
/// Critique session, each of which costs 3K+ tokens of reasoning.
/// Set `SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES=0` to disable.
fn inline_input_threshold_bytes() -> u64 {
    std::env::var("SIM_FLOW_INLINE_INPUT_THRESHOLD_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(4096)
}

fn render_directory_block(project_dir: &Path, rel: &str) -> String {
    let abs = project_dir.join(rel);
    let entries = match std::fs::read_dir(&abs) {
        Ok(it) => it.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(_) => {
            return format!("- `{rel}` (directory; could not be read)\n");
        }
    };
    if entries.is_empty() {
        return format!("- `{rel}` (directory, EMPTY)\n");
    }
    let mut listings: Vec<(String, String)> = Vec::with_capacity(entries.len());
    for ent in entries {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hide dotfiles (.gitkeep, .DS_Store, etc.)
        }
        let suffix = match ent.file_type() {
            Ok(ft) if ft.is_dir() => {
                let n = std::fs::read_dir(ent.path())
                    .map(|it| {
                        it.filter_map(|e| e.ok())
                            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                            .count()
                    })
                    .unwrap_or(0);
                format!(
                    "/ (directory, {n} entr{})",
                    if n == 1 { "y" } else { "ies" }
                )
            }
            Ok(_) => match ent.metadata() {
                Ok(m) => format!(" ({} bytes)", m.len()),
                Err(_) => String::from(" (size unknown)"),
            },
            Err(_) => String::new(),
        };
        listings.push((name.clone(), suffix));
    }
    if listings.is_empty() {
        return format!("- `{rel}` (directory, EMPTY)\n");
    }
    listings.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = format!(
        "- `{rel}` (directory, {} entr{}):\n",
        listings.len(),
        if listings.len() == 1 { "y" } else { "ies" }
    );
    for (name, suffix) in listings {
        out.push_str(&format!("  - {name}{suffix}\n"));
    }
    out
}

fn toc_entry_for(project_dir: &Path, rel: &str) -> TocEntry {
    if rel.ends_with('/') {
        return TocEntry {
            rel: rel.to_string(),
            state: TocState::Directory,
        };
    }
    let abs = project_dir.join(rel);
    match std::fs::metadata(&abs) {
        Ok(meta) if meta.is_dir() => TocEntry {
            rel: rel.to_string(),
            state: TocState::Directory,
        },
        Ok(meta) => {
            let bytes = meta.len();
            let threshold = inline_input_threshold_bytes();
            // Try to inline small text-shaped files. Binary files
            // (.png, .jpg, .pdf, .db) stay as plain TOC entries
            // even when small -- they aren't useful to the agent
            // as fenced text and would corrupt the markdown.
            if threshold > 0
                && bytes <= threshold
                && is_inlinable_extension(rel)
                && let Ok(body) = std::fs::read_to_string(&abs)
            {
                return TocEntry {
                    rel: rel.to_string(),
                    state: TocState::InlinedFile { bytes, body },
                };
            }
            TocEntry {
                rel: rel.to_string(),
                state: TocState::File { bytes },
            }
        }
        Err(_) => TocEntry {
            rel: rel.to_string(),
            state: TocState::Missing,
        },
    }
}

/// Whitelist of extensions safe to inline as fenced text. Markdown,
/// Rust source, TOML configs, JSON (e.g. critique.json), shell,
/// plain text, and the docs we know are always small. Skip
/// binaries and large generated artifacts even if they happen to
/// fall under the byte threshold.
fn is_inlinable_extension(rel: &str) -> bool {
    let ext = std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "rs" | "toml" | "json" | "txt" | "sh" | "yml" | "yaml" | ""
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n... (truncated)", &s[..max])
    }
}

/// Compute a hash of `text` after normalizing away churn that varies
/// turn-to-turn while the structural content stays the same:
///
/// - Runs of ASCII digits collapse to `<N>` (eats timestamps, byte
///   counts, line numbers, retry indices, durations, exit codes).
/// - Runs of whitespace collapse to a single space (different
///   indentation / line wrapping doesn't defeat the comparison).
///
/// Used by the stuck-loop guard. Two responses that differ only in
/// numbers and whitespace map to the same hash and trip the guard.
fn normalized_response_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let normalized = normalize_for_loop_detection(text);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    hasher.finish()
}

fn normalize_for_loop_detection(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digit_run = false;
    let mut in_ws_run = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digit_run {
                out.push_str("<N>");
                in_digit_run = true;
            }
            in_ws_run = false;
            continue;
        }
        in_digit_run = false;
        if ch.is_whitespace() {
            if !in_ws_run {
                out.push(' ');
                in_ws_run = true;
            }
            continue;
        }
        in_ws_run = false;
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------
// Artifact extraction: parse fenced ``` <path> blocks out of the
// LLM response. Mirrors the TS regex in artifacts.ts.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedArtifact {
    pub relative_path: String,
    pub content: String,
}

fn extract_artifacts(response_text: &str) -> Vec<ExtractedArtifact> {
    use std::collections::HashMap;
    // Multi-line search for `^``` <path>\n...\n``` $`. We do this by
    // hand to avoid a regex with multiline + dotall combos that fight
    // the `regex` crate's defaults.
    let mut out: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut lines = response_text.split('\n').enumerate().peekable();
    let mut in_block: Option<(String, Vec<String>)> = None;
    for (_idx, line) in &mut lines {
        let trimmed_line = line;
        if let Some((path, body)) = in_block.as_mut() {
            if trimmed_line.trim_start().starts_with("```") && trimmed_line.trim().len() == 3 {
                // Closing fence.
                let content = body.join("\n");
                if !out.contains_key(path) {
                    order.push(path.clone());
                }
                out.insert(path.clone(), content);
                in_block = None;
            } else {
                body.push(line.to_string());
            }
        } else if let Some(rest) = trimmed_line.strip_prefix("```") {
            // Opening fence: info-string follows. If it looks like a
            // path (has a `.` and no whitespace), treat as artifact.
            let info = rest.trim();
            if !info.is_empty() && info.contains('.') && is_safe_relative_path(info) {
                in_block = Some((info.to_string(), Vec::new()));
            }
            // else: a normal language fence (e.g. ```rust); ignore.
        }
    }

    order
        .into_iter()
        .map(|path| {
            let content = out.remove(&path).unwrap_or_default();
            ExtractedArtifact {
                relative_path: path,
                content: strip_trailing_newline(&content).to_string(),
            }
        })
        .collect()
}

fn strip_trailing_newline(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

fn is_safe_relative_path(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    if p.starts_with('/') || p.starts_with('\\') {
        return false;
    }
    if p.contains("..") {
        return false;
    }
    if p.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return false;
    }
    if p.chars().any(|c| (c as u32) < 0x20) {
        return false;
    }
    p.contains('.')
}

/// True when `p` lands inside `.sim-flow/` (the orchestrator's own
/// state tree). Agents must never write here -- not `state.toml` (a
/// past run had the agent "fix" its own gate status by editing it),
/// not `config.toml`, not the prompt overrides, not the control
/// socket. We enforce this on the JSONL artifact-writer side; in PTY
/// mode the system prompt carries the same prohibition since the
/// agent's native Write tool is out of our reach.
fn writes_to_sim_flow_state(p: &str) -> bool {
    let normalized = p.replace('\\', "/");
    normalized == ".sim-flow" || normalized.starts_with(".sim-flow/")
}

fn write_artifact(
    project_dir: &Path,
    write_paths: &[String],
    art: &ExtractedArtifact,
) -> Result<u64> {
    if !is_safe_relative_path(&art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting unsafe artifact path: {}",
            art.relative_path
        )));
    }
    if writes_to_sim_flow_state(&art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting agent write to orchestrator state tree: {} (the `.sim-flow/` directory is read-only for the agent; write generated documents under `docs/`, project source under `src/`, etc.)",
            art.relative_path
        )));
    }
    if !crate::steps::is_path_allowed_for_writes(write_paths, &art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting agent write to `{}`: outside the per-step write allowlist ({}). Update the artifact path to land under one of the allowed prefixes, or extend the step's `work_write_paths` if the new location is a deliberate widening.",
            art.relative_path,
            if write_paths.is_empty() {
                "(none)".to_string()
            } else {
                write_paths.join(", ")
            },
        )));
    }
    // is_safe_relative_path rejects absolute paths and any segment
    // containing "..", so `project_dir.join(<safe-relative>)` is
    // guaranteed to stay inside `project_dir` without needing a
    // canonicalize round-trip on a not-yet-existing file.
    let abs = project_dir.join(&art.relative_path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&abs, art.content.as_bytes()).map_err(|source| Error::Io {
        path: abs.clone(),
        source,
    })?;
    // When the agent writes a critique JSON, render the markdown
    // sibling immediately. The agent only emits the canonical JSON;
    // humans (and the gate's grep-the-md fallback for legacy
    // projects) read the rendered markdown. Render errors surface
    // as protocol errors so a malformed critique fails loud rather
    // than silently leaving a stale `.md` on disk.
    if crate::critique::is_critique_json_path(&art.relative_path) {
        crate::critique::render_critique_markdown_to_disk(project_dir, &art.relative_path)?;
    }
    Ok(art.content.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_args_from_body_accepts_write_file_json() {
        // Regression: native-tool-calling backends (LM Studio, OpenAI,
        // Anthropic) translate function-call responses into
        // `tool:<name>` fenced blocks with JSON args. Rejecting
        // `write_file` outright sent those agents into a runaway
        // retry loop because they had no other shape to emit.
        let body = "{\"path\":\"docs/targets.md\",\"content\":\"# Targets\\n\"}";
        let value = tool_args_from_body("write_file", body)
            .expect("write_file with JSON args must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("docs/targets.md")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("# Targets\n")
        );
    }

    #[test]
    fn tool_args_from_body_rejects_write_file_without_content() {
        // A bare path line with nothing after it has no content to
        // write. The fallback must surface a help message including
        // a concrete artifact-write example so the agent can recover
        // in one turn instead of guessing.
        let err = tool_args_from_body("write_file", "docs/targets.md\n").unwrap_err();
        assert!(
            err.contains("missing file content"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("```src/model/mod.rs"),
            "expected example: {err}"
        );
        assert!(err.contains("artifact-write"), "expected guidance: {err}");
    }

    #[test]
    fn tool_args_from_body_accepts_write_file_path_then_content() {
        // Permissive fallback: agents that emit `tool:write_file`
        // with a bare path + blank line + content body get treated
        // as if they had passed JSON args. Native-tool-use backends
        // synthesize this shape constantly; rejecting it cost
        // auto-iters in the e2e flow. `body.lines()` strips trailing
        // newlines, so the joined content matches that view.
        let body = "src/model/mod.rs\n\npub mod payloads;\npub mod stages;\n";
        let value =
            tool_args_from_body("write_file", body).expect("path + content body must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("src/model/mod.rs")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("pub mod payloads;\npub mod stages;")
        );
    }

    #[test]
    fn tool_args_from_body_accepts_write_file_path_with_no_blank_separator() {
        // Some agents skip the blank line between path and content.
        // The fallback should still recover -- treat the rest of the
        // body as content directly.
        let body = "src/lib.rs\nfn main() {}\n";
        let value = tool_args_from_body("write_file", body)
            .expect("path + immediate content must be accepted");
        assert_eq!(
            value.get("path").and_then(|v| v.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(
            value.get("content").and_then(|v| v.as_str()),
            Some("fn main() {}")
        );
    }

    #[test]
    fn tool_args_from_body_rejects_write_file_with_only_blank_lines() {
        // Path line followed only by blank lines = no actual content
        // to write. Surface the same help message as an empty body.
        let err = tool_args_from_body("write_file", "src/foo.rs\n\n\n").unwrap_err();
        assert!(
            err.contains("missing file content"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn normalize_strips_runs_of_digits() {
        // Catches timestamps, byte counts, retry indices, exit codes.
        // Two messages that differ only in numbers should normalize
        // identically.
        let a = "compile failed at 12:34:56 (exit 1, 4096 bytes)";
        let b = "compile failed at 18:02:11 (exit 7, 12 bytes)";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
        assert!(normalize_for_loop_detection(a).contains("<N>"));
    }

    #[test]
    fn normalize_collapses_whitespace_runs() {
        let a = "error:  cannot find  thing";
        let b = "error: cannot find\n\tthing";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
    }

    #[test]
    fn normalize_distinguishes_structurally_different_text() {
        let a = "compile error: missing import";
        let b = "compile error: type mismatch";
        assert_ne!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
    }

    #[test]
    fn normalized_hash_matches_for_timestamp_only_diffs() {
        // The exact case the user warned about: spewing the same
        // error with shifting timestamps every retry. Hash should
        // match across turns even though no two messages are
        // byte-identical.
        let h1 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:11:51Z (run 1)");
        let h2 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:12:42Z (run 2)");
        let h3 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:13:33Z (run 3)");
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn extract_artifacts_picks_fenced_blocks_with_paths() {
        let body = "Here is the spec.\n\n```spec.md\n# Spec\nClock: 2 GHz\n```\n\nDone.";
        let arts = extract_artifacts(body);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].relative_path, "spec.md");
        assert_eq!(arts[0].content, "# Spec\nClock: 2 GHz");
    }

    #[test]
    fn extract_artifacts_ignores_language_only_fences() {
        let body = "```rust\nfn main() {}\n```\n";
        assert!(extract_artifacts(body).is_empty());
    }

    #[test]
    fn extract_artifacts_rejects_traversal_paths() {
        let body = "```../etc/passwd\nx\n```\n```/abs.md\nx\n```\n";
        assert!(extract_artifacts(body).is_empty());
    }

    #[test]
    fn extract_artifacts_keeps_latest_when_path_repeats() {
        let body = "```spec.md\nv1\n```\n```spec.md\nv2\n```\n";
        let arts = extract_artifacts(body);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].content, "v2");
    }

    #[test]
    fn write_artifact_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let bytes = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "docs/notes.md".into(),
                content: "hi".into(),
            },
        )
        .unwrap();
        assert_eq!(bytes, 2);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("docs/notes.md")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn write_artifact_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let err = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "../escape.md".into(),
                content: "x".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[test]
    fn write_artifact_rejects_orchestrator_state_writes() {
        // Agent must not touch anything under `.sim-flow/` -- past
        // runs have had the agent try to "fix" its own gate status
        // by editing state.toml. Cover the obvious targets and a
        // backslash-disguised variant.
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        for bad in [
            ".sim-flow/state.toml",
            ".sim-flow/config.toml",
            ".sim-flow/critiques/DM0-critique.md",
            ".sim-flow/prompts/dm0-specification.work.md",
            ".sim-flow\\state.toml",
        ] {
            let err = write_artifact(
                tmp.path(),
                &allowed,
                &ExtractedArtifact {
                    relative_path: bad.into(),
                    content: "tampered".into(),
                },
            )
            .unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains("orchestrator state tree"),
                "expected state-tree rejection for {bad:?}, got: {msg}",
            );
        }
    }

    #[test]
    fn write_artifact_rejects_paths_outside_write_allowlist() {
        // The per-step write allowlist gates fenced artifact-write
        // blocks, not just `write_file` tool calls. A step whose
        // allowlist is `["docs/"]` must reject a fenced ` ```src/lib.rs `
        // block — otherwise the allowlist would only constrain the
        // tool-use API, leaving the artifact-write convention as a
        // bypass.
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let err = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "src/lib.rs".into(),
                content: "fn main() {}".into(),
            },
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("write allowlist"),
            "expected allowlist rejection, got: {msg}",
        );
    }

    #[test]
    fn parse_blocker_lines_handles_common_shapes() {
        // The critique format is free-form markdown; agents have
        // emitted blockers as `- BLOCKER: ...`, `* **BLOCKER:** ...`,
        // and `BLOCKER: ...` (bare) across runs. The retry-detection
        // path must recognize all of them so a critique-retry doesn't
        // silently fall back to the full evaluation just because a
        // model preferred bold-font BLOCKER markers.
        let body = "\
# DM0 Critique\n\
\n\
- BLOCKER: missing gate budget\n\
* **BLOCKER:** ambiguous reset semantics\n\
BLOCKER: no examples for stage 2\n\
- UNRESOLVED: layout details\n\
- RESOLVED: clock domain decision\n\
random text BLOCKER: not a heading\n\
";
        let blockers = parse_blocker_lines(body);
        assert_eq!(blockers.len(), 3, "got {blockers:?}");
        assert!(blockers[0].contains("missing gate budget"));
        assert!(blockers[1].contains("ambiguous reset semantics"));
        assert!(blockers[2].contains("no examples for stage 2"));
    }

    #[test]
    fn parse_blocker_lines_returns_empty_when_clean() {
        // A critique that resolved cleanly emits only RESOLVED /
        // UNRESOLVED lines. The retry-detection path keys off "any
        // BLOCKER present" -- empty here means the next critique
        // pass should run the full evaluation, not the focused-retry
        // shortcut.
        let body = "- RESOLVED: clock domain.\n- UNRESOLVED: stage 2 timing.\n";
        assert!(parse_blocker_lines(body).is_empty());
    }

    #[test]
    fn parse_blocker_lines_ignores_inline_mentions() {
        // Don't trigger on prose that mentions the word BLOCKER
        // mid-sentence; we only care about heading-shaped lines.
        let body = "We discussed the BLOCKER: marker convention earlier.\n";
        assert!(parse_blocker_lines(body).is_empty());
    }

    #[test]
    fn extract_blocker_blocks_captures_multi_line_body() {
        // Real DM3a critiques emit a single BLOCKER followed by
        // sub-bullets and a fix recipe. The whole block must come
        // through so the focused-retry context still contains the
        // actionable detail.
        let body = "\
### BLOCKER 2 - coverage.md incomplete\n\
\n\
BLOCKER: `coverage.md` was partially updated, but gaps persist:\n\
\n\
- **Numeric threshold** - still absent.\n\
- **Exclusions with reasons** - command-line flags are not prose.\n\
- **Report path** - only the directory is named.\n\
\n\
The fix is to update `coverage.md` to add (a) ... (b) ... (c) ...\n\
\n\
### BLOCKER 3 - traceability table\n\
\n\
RESOLVED: traceability section satisfies check 11.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        let block = &blocks[0];
        assert!(block.starts_with("BLOCKER: `coverage.md` was partially updated"));
        assert!(block.contains("Numeric threshold"));
        assert!(block.contains("Exclusions with reasons"));
        assert!(block.contains("Report path"));
        assert!(block.contains("The fix is to update"));
        // Must stop before the next heading.
        assert!(!block.contains("### BLOCKER 3"));
        assert!(!block.contains("RESOLVED: traceability"));
    }

    #[test]
    fn extract_blocker_blocks_terminates_on_finding_marker() {
        // A bare BLOCKER followed by a sibling RESOLVED on the next
        // line should yield exactly the BLOCKER body up to (not
        // including) the RESOLVED line.
        let body = "\
- BLOCKER: missing gate budget. The spec lacks a hard cycle\n\
  bound for the worst-case path through stage 2.\n\
- RESOLVED: clock domain decision recorded.\n\
- UNRESOLVED: stage 2 timing still pending.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("missing gate budget"));
        assert!(blocks[0].contains("worst-case path"));
        assert!(!blocks[0].contains("RESOLVED"));
        assert!(!blocks[0].contains("UNRESOLVED"));
    }

    #[test]
    fn extract_blocker_blocks_terminates_on_horizontal_rule() {
        // Markdown horizontal rules (`---`, `***`) commonly delimit
        // sections in our critique template; they end a BLOCKER body.
        let body = "\
BLOCKER: foo is broken because of bar.\n\
\n\
Fix it by doing X.\n\
\n\
---\n\
\n\
## Carried-Forward Items\n\
\n\
UNRESOLVED: shorthand references.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("foo is broken"));
        assert!(blocks[0].contains("Fix it by doing X"));
        assert!(!blocks[0].contains("---"));
        assert!(!blocks[0].contains("Carried-Forward"));
    }

    #[test]
    fn extract_blocker_blocks_handles_header_shaped_body_lines() {
        // `### BLOCKER 1` is a heading describing a finding, not a
        // finding line itself. The actual `BLOCKER:` marker lives on
        // a later line. The header should NOT be captured as a
        // separate finding, and it terminates the prior block.
        let body = "\
### BLOCKER 1 - stress.md target coverage\n\
\n\
RESOLVED: stress.md exercises every target.\n\
\n\
### BLOCKER 2 - coverage.md\n\
\n\
BLOCKER: numeric threshold missing.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert!(blocks[0].starts_with("BLOCKER: numeric threshold"));
    }

    #[test]
    fn extract_blocker_blocks_trims_trailing_blank_lines() {
        // Blocks are joined back together when inlined into the
        // retry prompt; trailing blank lines would compound into
        // visual noise. Trim them.
        let body = "\
BLOCKER: thing is wrong.\n\
\n\
Some explanation.\n\
\n\
\n\
\n\
## Next Section\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].ends_with('\n'));
        assert!(blocks[0].ends_with("Some explanation."));
    }

    #[test]
    fn line_kind_matches_heading_with_emoji() {
        // The actual qwen run today emitted `### ❌ BLOCKER:` --
        // markdown H3 + dingbat + finding marker. The strict matcher
        // returned None, the gate saw zero blockers, and the step
        // advanced clean. Heading-style findings MUST match now.
        assert_eq!(
            line_kind("### ❌ BLOCKER: Report output path missing"),
            Some(FindingKind::Blocker),
        );
        assert_eq!(line_kind("# BLOCKER: foo"), Some(FindingKind::Blocker));
        assert_eq!(
            line_kind("## ✅ RESOLVED: clock-domain decision recorded"),
            Some(FindingKind::Resolved),
        );
    }

    #[test]
    fn line_kind_matches_plural_and_case_variants() {
        // Agents drift across forms; the gate parser must be lenient
        // in the FINDING half so blockers aren't silently dropped on
        // a case slip or a stray plural.
        assert_eq!(line_kind("BLOCKERS: two open"), Some(FindingKind::Blocker));
        assert_eq!(line_kind("Blocker: foo"), Some(FindingKind::Blocker));
        assert_eq!(line_kind("blocker: foo"), Some(FindingKind::Blocker));
        assert_eq!(
            line_kind("- **BLOCKER:** ambiguous reset"),
            Some(FindingKind::Blocker),
        );
        assert_eq!(
            line_kind("> BLOCKER: blockquote-styled finding"),
            Some(FindingKind::Blocker),
        );
    }

    #[test]
    fn line_kind_rejects_inline_mentions_and_section_titles() {
        // Section heading discussing a blocker (no colon-after) is
        // NOT a finding -- it's prose. And mid-sentence mentions
        // never count.
        assert_eq!(line_kind("### BLOCKER 1 - stress.md target coverage"), None,);
        assert_eq!(
            line_kind("We discussed the BLOCKER: marker convention earlier."),
            None,
        );
        assert_eq!(line_kind(""), None);
        assert_eq!(line_kind("## Carried-Forward Items"), None);
    }

    #[test]
    fn extract_blocker_blocks_captures_heading_style_finding() {
        // End-to-end: a heading-with-emoji BLOCKER is correctly
        // extracted as a multi-line block (the regression that
        // motivated this change).
        let body = "\
## Prior BLOCKER 1: coverage.md\n\
\n\
### ❌ BLOCKER: Report output path missing\n\
\n\
The run command names a directory, not a file.\n\
\n\
The fix is to add a Report Output section.\n\
\n\
### ✅ RESOLVED: numeric threshold\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert!(blocks[0].contains("Report output path missing"));
        assert!(blocks[0].contains("The run command names a directory"));
        assert!(blocks[0].contains("The fix is to add"));
        assert!(!blocks[0].contains("RESOLVED"));
    }
}
