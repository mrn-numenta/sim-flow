//! Session-orchestrator option struct and shared constants.
//!
//! `OrchestratorOptions` is the input the CLI dispatch layer passes
//! to `run_session`. It lives here (rather than in `dispatch.rs`) so
//! the helpers in this directory can reference its public fields
//! without pulling in the entire turn-loop module. Misc per-session
//! constants and a tiny clock helper that several submodules share
//! also live here.

use std::path::PathBuf;

use crate::client::SessionKind;

pub(super) const FRAMEWORK_DOCS_ROOT_ENV: &str = "SIM_FLOW_FRAMEWORK_DOCS_ROOT";

/// Wall-clock epoch seconds, defaulting to 0 if the system clock is
/// misbehaving. Used for the `started_unix` column on every LLM
/// metrics row -- a row with a 0 timestamp is uglier than crashing
/// the session, so we swallow the error and let downstream
/// consumers notice the bogus value.
pub(super) fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One-strike-warning prefix injected into the next user message
/// the orchestrator builds when the runaway-loop detector sees
/// `cap - 1` structurally-identical responses in a row. The next
/// identical response will trip the abort; this gives the agent
/// one explicit chance to break the cycle by re-reading the prior
/// tool / build error rather than retrying the same call shape.
pub(super) const LOOP_HINT_PREFIX: &str = "Loop guard warning: your last response was structurally identical to the prior one. \
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
    /// Optional explicit model-family override the host should pass
    /// through to its backend/runtime selection.
    pub llm_model_family_id: Option<String>,
    /// Optional explicit runtime-profile override the host should
    /// pass through to its backend/runtime selection.
    pub llm_runtime_profile_id: Option<String>,
    /// When true, hosts should surface extra adaptation diagnostics
    /// (backend/runtime/model-family/capabilities) around LLM calls.
    pub llm_debug_adaptation: bool,
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
    /// Pin this session to a specific milestone within the step's
    /// `milestone_walk` (instead of `find_current_milestone`'s
    /// first-pending / highest-touched heuristics). Used by the
    /// parallel plan-detail walk dispatcher in
    /// `session::auto::run_plan_detail_walk_parallel` so each worker
    /// thread operates on the stub it was assigned. `None` keeps
    /// today's behavior: the orchestrator picks the milestone via
    /// `find_current_milestone`. Value is the bare milestone
    /// filename (e.g. `"milestone-03-decode.md"`) or the
    /// project-relative path; both forms resolve to the same stub.
    pub milestone_name: Option<String>,
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
            llm_model_family_id: None,
            llm_runtime_profile_id: None,
            llm_debug_adaptation: false,
            llm_base_url: None,
            auto: false,
            max_auto_iters: 3,
            max_llm_requests: 50,
            max_identical_responses: 3,
            agent_has_native_fs_tools: false,
            no_preamble: true,
            milestone_name: None,
        }
    }
}
