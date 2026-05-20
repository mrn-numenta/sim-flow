//! Caller-facing request shape + builders. Reads the
//! `SIM_FLOW_*` env knobs at construction time so each callsite
//! gets a request pre-populated from the operator's overrides.

use super::super::tool_calls::ToolDescriptor;
use crate::session::protocol::LlmMessage;

/// Configuration the caller hands to `dispatch_chat`. Tools are not
/// yet exposed (the orchestrator's tool-use path goes through the
/// fenced-block fallback for these backends in M3); we keep the
/// shape forward-compatible.
pub struct OpenAiCompatibleRequest<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub model_family_id: Option<&'a str>,
    pub messages: &'a [LlmMessage],
    pub api_key: Option<&'a str>,
    /// Stop early once we've collected this many bytes of response.
    /// Defaults to 64 KB; rarely matters in practice but bounds the
    /// chat-side memory footprint if a model goes off the rails.
    pub max_response_bytes: usize,
    /// Server-side completion-token cap forwarded as `max_tokens` in
    /// the chat-completions request body. Bounds runaway generation
    /// from chain-of-thought-heavy local models (e.g. nemotron with
    /// reasoning enabled), which can otherwise burn 90K+ tokens
    /// before producing the actual artifact. Defaults read from the
    /// `SIM_FLOW_MAX_TOKENS` env var, falling back to 65536.
    ///
    /// Default history: 16384 -> 32768 -> 65536. Each bump was
    /// triggered by a real truncation observed on long-turn steps
    /// (DM2a/DM2b/DM2d for qwen3.6 most recently -- see the Phase 0
    /// findings in docs/brainstorming/model-robustness-study.md).
    /// 65536 still sits well under any locally-served model's
    /// `max_model_len` (qwen3.6 = 262144, gemma-4 = 131072, kimi-vl
    /// = 131072), so a runaway is bounded by the server's context
    /// wall rather than by this default. Override via the env var
    /// for narrower-context backends.
    pub max_tokens: u32,
    /// Optional deterministic sampling seed. When set, included as
    /// the `seed` field in the chat-completions request body. vLLM,
    /// llama.cpp, and any other openai-compat server that honors
    /// the field will replay identical token streams given the same
    /// prompt + temperature + seed. Used by the model-robustness
    /// study harness so K=3 trials are reproducible per-trial-index
    /// rather than fully stochastic (Phase 0 saw 0/3 reproducible
    /// because we hadn't yet plumbed this). Read from the
    /// `SIM_FLOW_SEED` env var; unset means "no seed sent" (server
    /// picks).
    pub seed: Option<u32>,
    /// When `true`, include `chat_template_kwargs: {"enable_thinking":
    /// false}` in the request body so models with a thinking-section
    /// chat template (qwen3.6, deepseek-r1, ...) skip the `<think>...
    /// </think>` preamble. Saves tokens on every turn and improves
    /// answer quality on tool-heavy prompts where the thinking
    /// expands faster than the actual decision. The flag is purely
    /// "ask the chat template not to think"; if the server doesn't
    /// recognize the kwarg it's silently ignored (vLLM threads it
    /// through to the model's Jinja template).
    ///
    /// Read from the `SIM_FLOW_DISABLE_THINKING` env var (any
    /// truthy value: `1`, `true`, `yes`). Default `false`.
    pub disable_thinking: bool,
    /// Soft upper bound on the number of tokens the model is asked
    /// to spend on its reasoning preamble. Forwarded to the server
    /// via `chat_template_kwargs.thinking_budget` so model families
    /// whose chat template understands the kwarg (qwen3.6, similar
    /// open-reasoning models) self-truncate when the budget is hit;
    /// templates that don't understand it ignore it silently.
    ///
    /// Read from the `SIM_FLOW_THINKING_BUDGET` env var (positive
    /// integer). `None` lets the model decide -- typical for short
    /// turns where reasoning legitimately fits in the response cap;
    /// set when long reasoning chains start consuming the
    /// `max_tokens` budget before the model emits a tool call.
    pub thinking_budget: Option<u32>,
    /// Per-knob overrides for the sampling parameters the family
    /// would otherwise pull from its `non_thinking_sampling`
    /// default. Sourced from env vars (`SIM_FLOW_TEMPERATURE`,
    /// `SIM_FLOW_TOP_P`, `SIM_FLOW_TOP_K`, `SIM_FLOW_MIN_P`,
    /// `SIM_FLOW_PRESENCE_PENALTY`, `SIM_FLOW_REPETITION_PENALTY`).
    /// Each is independent; setting one doesn't suppress the
    /// others. Useful for ad-hoc tuning during the
    /// model-robustness study.
    pub temperature_override: Option<f32>,
    pub top_p_override: Option<f32>,
    pub top_k_override: Option<u32>,
    pub min_p_override: Option<f32>,
    pub presence_penalty_override: Option<f32>,
    pub repetition_penalty_override: Option<f32>,
    /// Tool catalog to advertise on this request. When `Some` and
    /// non-empty, serializes as the OpenAI `tools` field; the
    /// server is expected to return `tool_calls` in the response
    /// when the model decided to call one. Today the orchestrator
    /// only populates this when running in native-tool-call mode
    /// (Phase B+ of the native-tool-calls migration); the legacy
    /// fenced-block path leaves it `None` so the wire body stays
    /// minimal.
    pub tools: Option<Vec<ToolDescriptor>>,
    /// Sets the OpenAI `tool_choice` parameter. `"auto"` lets the
    /// model decide; `"required"` forces a tool call; specific
    /// `{"type":"function","function":{"name":"..."}}` shapes are
    /// not modeled yet. `None` means the field is omitted from the
    /// wire body, which is equivalent to `"auto"` on every
    /// conformant backend but avoids tripping minimal proxies that
    /// reject the field outright.
    pub tool_choice: Option<&'static str>,
    /// Wall-clock budget (seconds) the dispatch loop has to retry
    /// transient failures (connection refused, 408/429/502/503/504,
    /// network timeouts) before giving up. `0` disables retries
    /// entirely. Default is 600 seconds (10 minutes); override via
    /// `SIM_FLOW_RETRY_BUDGET_SECS` env var. Motivated by parallel
    /// runs sharing one vLLM server -- the server briefly refuses
    /// connections during model reload and 503s under request
    /// pressure; without retries every blip aborts the run.
    pub retry_budget_secs: u32,
}

pub(super) fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("True") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

pub(super) fn float_env(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|s| s.parse::<f32>().ok())
}

pub(super) fn uint_env(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|s| s.parse::<u32>().ok())
}

impl<'a> OpenAiCompatibleRequest<'a> {
    pub fn new(base_url: &'a str, model: &'a str, messages: &'a [LlmMessage]) -> Self {
        let max_tokens = std::env::var("SIM_FLOW_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(65_536);
        let seed = std::env::var("SIM_FLOW_SEED")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        let disable_thinking = truthy_env("SIM_FLOW_DISABLE_THINKING");
        let thinking_budget = uint_env("SIM_FLOW_THINKING_BUDGET");
        // Response-body cap. 64 KB was too tight for verbose
        // models -- qwen3.6's milestone-04 response with embedded
        // Rust code legitimately exceeded that and the JSON parser
        // EOF'd mid-string (`failed to parse response: EOF while
        // parsing a string at line 1 column 65536`). 512 KB is
        // still small relative to a workstation's memory budget
        // and well above any legitimate single-turn answer; if a
        // server returns more than that it's almost certainly
        // garbage or runaway behavior. Override via
        // `SIM_FLOW_MAX_RESPONSE_BYTES` if needed.
        let max_response_bytes = std::env::var("SIM_FLOW_MAX_RESPONSE_BYTES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(512 * 1024);
        let retry_budget_secs = std::env::var("SIM_FLOW_RETRY_BUDGET_SECS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(600);
        Self {
            base_url,
            model,
            model_family_id: None,
            messages,
            api_key: None,
            max_response_bytes,
            max_tokens,
            seed,
            disable_thinking,
            thinking_budget,
            temperature_override: float_env("SIM_FLOW_TEMPERATURE"),
            top_p_override: float_env("SIM_FLOW_TOP_P"),
            top_k_override: uint_env("SIM_FLOW_TOP_K"),
            min_p_override: float_env("SIM_FLOW_MIN_P"),
            presence_penalty_override: float_env("SIM_FLOW_PRESENCE_PENALTY"),
            repetition_penalty_override: float_env("SIM_FLOW_REPETITION_PENALTY"),
            tools: None,
            tool_choice: None,
            retry_budget_secs,
        }
    }

    pub fn with_tools(
        mut self,
        tools: Vec<ToolDescriptor>,
        tool_choice: Option<&'static str>,
    ) -> Self {
        if tools.is_empty() {
            self.tools = None;
            self.tool_choice = None;
        } else {
            self.tools = Some(tools);
            self.tool_choice = tool_choice;
        }
        self
    }

    pub fn with_model_family_id(mut self, model_family_id: Option<&'a str>) -> Self {
        self.model_family_id = model_family_id;
        self
    }
}
