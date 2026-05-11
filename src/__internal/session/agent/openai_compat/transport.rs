//! Shared HTTP client for OpenAI-compatible chat-completion endpoints.
//!
//! Ollama and LM Studio expose this same wire format on their local
//! servers; the existing TS extension uses an `OpenAiCompatibleBackend`
//! base class for the same reason. This Rust module mirrors that
//! pattern so the per-server agent impls stay tiny.

use serde::{Deserialize, Serialize};

use super::super::{
    normalize_response_text, prepare_messages_for_openai_compat, resolve_model_family,
};
use super::tool_calls::{NativeToolCall, ToolDescriptor};
use crate::session::agent::LlmCallMetrics;
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::{Error, Result};

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
}

fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("True") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn float_env(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|s| s.parse::<f32>().ok())
}

fn uint_env(name: &str) -> Option<u32> {
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
            temperature_override: float_env("SIM_FLOW_TEMPERATURE"),
            top_p_override: float_env("SIM_FLOW_TOP_P"),
            top_k_override: uint_env("SIM_FLOW_TOP_K"),
            min_p_override: float_env("SIM_FLOW_MIN_P"),
            presence_penalty_override: float_env("SIM_FLOW_PRESENCE_PENALTY"),
            repetition_penalty_override: float_env("SIM_FLOW_REPETITION_PENALTY"),
            tools: None,
            tool_choice: None,
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

#[derive(Debug, Clone, Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    messages: Vec<RequestMessage<'a>>,
    stream: bool,
    max_tokens: u32,
    /// Optional deterministic-sampling seed. `skip_serializing_if`
    /// keeps the wire body unchanged for servers that reject
    /// unknown fields (some openai-compat proxy shims do); when
    /// unset, no `seed` key appears.
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u32>,
    /// `chat_template_kwargs` is vLLM's pass-through for Jinja
    /// template parameters. Setting `enable_thinking: false`
    /// asks the qwen3.6 / deepseek-r1 / similar chat template to
    /// skip the `<think>...</think>` preamble entirely. Other
    /// servers either honor the field (llama.cpp, sglang) or
    /// silently ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_template_kwargs: Option<ChatTemplateKwargs>,
    /// Sampling parameters. All `skip_serializing_if = None` so the
    /// wire shape stays minimal for servers that reject unknown
    /// keys. Populated from the model family's
    /// `non_thinking_sampling` defaults when `disable_thinking` is
    /// on, with env-var per-knob overrides on top. Empty for
    /// families with `non_thinking_sampling: None` -- those let the
    /// server's defaults stand.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repetition_penalty: Option<f32>,
    /// Tool catalog. When `Some` and non-empty, the server is
    /// instructed to consider these functions as tool-call targets.
    /// `skip_serializing_if = None` keeps the wire body unchanged
    /// for fence-mode requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDescriptor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct ChatTemplateKwargs {
    enable_thinking: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
    /// On `role = "tool"` messages: the call id this message is
    /// replying to. Pairs with the assistant's `tool_calls[i].id`
    /// from the prior turn. OpenAI's spec requires this for the
    /// model to associate the result with its originating call.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    /// On `role = "assistant"` messages: the prior tool_calls this
    /// turn emitted, echoed back so the model sees its own
    /// in-flight call requests. Empty when the turn produced no
    /// tool calls (fence-mode or plain text).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<RequestToolCall<'a>>,
}

#[derive(Debug, Clone, Serialize)]
struct RequestToolCall<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
    #[serde(rename = "type")]
    kind: &'static str, // always "function"
    function: RequestToolFunction<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct RequestToolFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatResponseBody {
    choices: Vec<Choice>,
    /// `usage` is part of the OpenAI chat-completions response shape.
    /// vLLM and LM Studio both populate it. Optional because some
    /// pre-1.0 servers (and proxy shims) omit it; we surface what we
    /// can without erroring.
    #[serde(default)]
    usage: Option<UsageBody>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageBody {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct Choice {
    message: ResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Both `content` and `reasoning` are optional and may arrive as
/// JSON `null` rather than missing — vLLM emits `"content": null`
/// alongside a populated `"reasoning"` field when a qwen / llama
/// chat template treats the assistant turn as reasoning-only, and
/// it does the same when `finish_reason == "length"` truncates the
/// answer mid-stream. Using `Option<String>` (with `serde(default)`
/// for the missing-field case) tolerates both shapes; the dispatch
/// path then prefers `content`, falls back to `reasoning`, and
/// surfaces a clear error when both are empty.
#[derive(Debug, Clone, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    /// Native tool calls returned by `--enable-auto-tool-choice` /
    /// OpenAI's tool-use endpoint. May be empty or omitted on
    /// non-tool turns; we tolerate either shape.
    #[serde(default)]
    tool_calls: Option<Vec<NativeToolCall>>,
}

/// What `dispatch_chat_with_tools` returns. The thin
/// back-compat `dispatch_chat` wrapper below discards `tool_calls`
/// and returns just the `(text, metrics)` pair.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<NativeToolCall>,
    pub metrics: LlmCallMetrics,
}

/// Send a synchronous chat-completions request and return the
/// assistant's full text plus per-call metrics (prompt /
/// completion tokens from the response `usage` object, total
/// wall-clock round-trip time). The endpoint defaults to
/// `<base_url>/chat/completions`.
///
/// Back-compat shim. Callers that need tool_calls should switch to
/// `dispatch_chat_with_tools`.
pub fn dispatch_chat(req: OpenAiCompatibleRequest<'_>) -> Result<(String, LlmCallMetrics)> {
    let resp = dispatch_chat_with_tools(req)?;
    Ok((resp.text, resp.metrics))
}

/// Same as `dispatch_chat` but also returns any native tool calls
/// the model emitted. The transport stays decoupled from the tool
/// registry: it just hands the raw OpenAI tool-call records back so
/// the orchestrator can map them through the `Tool` dispatcher.
pub fn dispatch_chat_with_tools(req: OpenAiCompatibleRequest<'_>) -> Result<ChatResponse> {
    let started = std::time::Instant::now();
    let model_family = resolve_model_family(req.model_family_id, Some(req.model));
    let prepared_messages = prepare_messages_for_openai_compat(req.messages, model_family);
    // Only thread the thinking-control kwarg through to servers
    // for families that have a thinking section to disable. For
    // generic / non-thinking models the kwarg is meaningless and
    // some servers reject unknown template kwargs.
    let chat_template_kwargs = if req.disable_thinking && model_family.supports_thinking_controls {
        Some(ChatTemplateKwargs {
            enable_thinking: false,
        })
    } else {
        None
    };
    // Resolve per-knob sampling params. Family `non_thinking_sampling`
    // applies when we're running this family with thinking disabled;
    // env-var overrides win over the family default on each knob
    // independently. Knobs without a family default OR an env
    // override remain `None` and are not serialized -- the server's
    // configured default stands.
    let family_defaults = if req.disable_thinking {
        model_family.non_thinking_sampling
    } else {
        None
    };
    let temperature = req
        .temperature_override
        .or(family_defaults.map(|s| s.temperature));
    let top_p = req.top_p_override.or(family_defaults.map(|s| s.top_p));
    let top_k = req.top_k_override.or(family_defaults.map(|s| s.top_k));
    let min_p = req.min_p_override.or(family_defaults.map(|s| s.min_p));
    let presence_penalty = req
        .presence_penalty_override
        .or(family_defaults.map(|s| s.presence_penalty));
    let repetition_penalty = req
        .repetition_penalty_override
        .or(family_defaults.map(|s| s.repetition_penalty));
    let body = ChatRequestBody {
        model: req.model,
        messages: prepared_messages
            .iter()
            .map(|m| RequestMessage {
                role: role_str(m.role),
                content: m.content.as_ref(),
                tool_call_id: m.tool_call_id.as_deref(),
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|c| RequestToolCall {
                        id: c.id.as_deref(),
                        kind: "function",
                        function: RequestToolFunction {
                            name: c.name.as_ref(),
                            arguments: c.arguments_json.as_ref(),
                        },
                    })
                    .collect(),
            })
            .collect(),
        stream: false,
        max_tokens: req.max_tokens,
        seed: req.seed,
        chat_template_kwargs,
        temperature,
        top_p,
        top_k,
        min_p,
        presence_penalty,
        repetition_penalty,
        tools: req.tools,
        tool_choice: req.tool_choice,
    };
    let url = format!("{}/chat/completions", trim_trailing_slash(req.base_url));
    let mut request = ureq::post(&url)
        .set("content-type", "application/json")
        .set("accept", "application/json");
    if let Some(key) = req.api_key {
        request = request.set("authorization", &format!("Bearer {key}"));
    }
    // ureq returns 4xx/5xx as `Error::Status(code, Response)`. The
    // status arm of `send_json`'s Result therefore doesn't reach the
    // success path's status check below; pull the body out of the
    // status error explicitly so vLLM / LM Studio's actual complaint
    // (e.g. "max_tokens exceeds max_model_len", "model not found")
    // surfaces in the LlmError diagnostic instead of a bare "status
    // code 400".
    let response = match request.send_json(
        serde_json::to_value(&body)
            .map_err(|e| Error::Client(format!("openai-compat: serialize request: {e}")))?,
    ) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            return Err(Error::Client(format!(
                "openai-compat: server returned {code}: {}",
                tail(&body, 2048),
            )));
        }
        Err(e) => {
            return Err(Error::Client(format!("openai-compat: HTTP error: {e}")));
        }
    };
    if !(200..300).contains(&response.status()) {
        let status = response.status();
        let body = response.into_string().unwrap_or_default();
        return Err(Error::Client(format!(
            "openai-compat: server returned {status}: {body}"
        )));
    }
    // Limit response body size so a misbehaving server can't OOM us.
    let mut reader = response.into_reader().take(req.max_response_bytes as u64);
    let mut buf = String::new();
    use std::io::Read;
    reader
        .read_to_string(&mut buf)
        .map_err(|e| Error::Client(format!("openai-compat: read body: {e}")))?;
    let parsed: ChatResponseBody = serde_json::from_str(&buf).map_err(|e| {
        Error::Client(format!(
            "openai-compat: failed to parse response: {e}\nbody tail: {}",
            tail(&buf, 1024)
        ))
    })?;
    let metrics = LlmCallMetrics {
        tokens_in: parsed.usage.as_ref().and_then(|u| u.prompt_tokens),
        tokens_out: parsed.usage.as_ref().and_then(|u| u.completion_tokens),
        wall_ms: started.elapsed().as_millis() as u64,
    };
    let choice = parsed.choices.into_iter().next();
    let tool_calls = choice
        .as_ref()
        .and_then(|c| c.message.tool_calls.clone())
        .unwrap_or_default();
    let text = decode_choice(choice, model_family)?;
    Ok(ChatResponse {
        text,
        tool_calls,
        metrics,
    })
}

/// Convert a single chat-completions choice into the assistant text
/// the orchestrator should see, OR into a hard error when the
/// response was truncated at max_tokens.
///
/// Truncation policy: `finish_reason == "length"` is a hard error
/// regardless of `content` presence. Today's debugging surfaced two
/// real failure modes where the model wrote a partial markdown file
/// (mid-bash-block, no closing backtick) and the orchestrator
/// committed the truncated bytes to disk; the gate then advanced
/// past a step whose artifact was structurally broken. Returning
/// Err here makes the orchestrator's LlmError path engage: no tool
/// calls / artifacts from this turn are committed, and the agent's
/// next turn sees the truncation diagnostic and can simplify or
/// raise the cap.
///
/// Content fallback: prefer `content` when populated; otherwise
/// fall back to `reasoning` (vLLM / qwen sometimes emit only
/// `reasoning` when the chat template treats a turn as
/// reasoning-only).
fn decode_choice(
    choice: Option<Choice>,
    model_family: &super::super::adaptation::ModelFamilyProfile,
) -> Result<String> {
    let Some(c) = choice else {
        return Ok(String::new());
    };
    let truncated = c.finish_reason.as_deref() == Some("length");
    let content = c.message.content.unwrap_or_default();
    let reasoning = c.message.reasoning.unwrap_or_default();
    if truncated {
        let tail_for_diag = if !content.is_empty() {
            tail(&content, 512)
        } else {
            tail(&reasoning, 512)
        };
        return Err(Error::Client(format!(
            "openai-compat: response truncated at max_tokens (finish_reason=length). \
             Refusing to commit a partial response (the agent's tool calls / file writes \
             would be incomplete). Raise SIM_FLOW_MAX_TOKENS, ask the agent to write \
             fewer files per turn, or simplify the prompt. Tail: {tail_for_diag}",
        )));
    }
    if !content.is_empty() {
        Ok(normalize_response_text(model_family, &content))
    } else if !reasoning.is_empty() {
        Ok(normalize_response_text(model_family, &reasoning))
    } else {
        Ok(String::new())
    }
}

fn role_str(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    }
}

fn trim_trailing_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[s.len() - max..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::agent::adaptation::{
        GEMMA4_MODEL_FAMILY, QWEN3_6_MODEL_FAMILY, prepare_messages_for_openai_compat,
    };

    fn empty_body(model: &str, max_tokens: u32) -> ChatRequestBody<'_> {
        ChatRequestBody {
            model,
            messages: vec![],
            stream: false,
            max_tokens,
            seed: None,
            chat_template_kwargs: None,
            temperature: None,
            top_p: None,
            top_k: None,
            min_p: None,
            presence_penalty: None,
            repetition_penalty: None,
            tools: None,
            tool_choice: None,
        }
    }

    #[test]
    fn request_body_omits_seed_and_kwargs_by_default() {
        // Default request: no seed, no chat_template_kwargs. Some
        // openai-compat proxies reject unknown keys, so the body
        // must stay minimal when the caller hasn't asked for the
        // new knobs.
        let body = empty_body("qwen3.6", 65_536);
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("\"seed\""), "json: {json}");
        assert!(!json.contains("\"chat_template_kwargs\""), "json: {json}");
        assert!(!json.contains("\"temperature\""), "json: {json}");
        assert!(!json.contains("\"presence_penalty\""), "json: {json}");
    }

    #[test]
    fn request_body_includes_seed_when_set() {
        let mut body = empty_body("qwen3.6", 64);
        body.seed = Some(42);
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"seed\":42"), "json: {json}");
    }

    #[test]
    fn request_body_includes_enable_thinking_false_when_kwargs_set() {
        // The kwarg shape is what vLLM threads into the Qwen
        // chat template. Pin the exact wire shape so a refactor
        // doesn't silently rename / move the field.
        let mut body = empty_body("qwen3.6", 64);
        body.chat_template_kwargs = Some(ChatTemplateKwargs {
            enable_thinking: false,
        });
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            json.contains("\"chat_template_kwargs\":{\"enable_thinking\":false}"),
            "json: {json}",
        );
    }

    #[test]
    fn request_body_serializes_sampling_knobs_when_set() {
        // All six sampling knobs are independent skip-if-None
        // fields. Pin their on-the-wire shape so a future refactor
        // can't silently rename them and break vLLM acceptance.
        let mut body = empty_body("qwen3.6", 64);
        body.temperature = Some(0.7);
        body.top_p = Some(0.8);
        body.top_k = Some(20);
        body.min_p = Some(0.0);
        body.presence_penalty = Some(1.5);
        body.repetition_penalty = Some(1.0);
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"temperature\":0.7"), "json: {json}");
        assert!(json.contains("\"top_p\":0.8"), "json: {json}");
        assert!(json.contains("\"top_k\":20"), "json: {json}");
        assert!(json.contains("\"min_p\":0.0"), "json: {json}");
        assert!(json.contains("\"presence_penalty\":1.5"), "json: {json}");
        assert!(json.contains("\"repetition_penalty\":1.0"), "json: {json}");
    }

    #[test]
    fn request_body_serializes_tools_and_tool_choice_when_set() {
        let mut body = empty_body("qwen3.6", 64);
        body.tools = Some(vec![ToolDescriptor::function(
            "list_dir".into(),
            "List a directory".into(),
            serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        )]);
        body.tool_choice = Some("auto");
        let v: serde_json::Value = serde_json::to_value(&body).unwrap();
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "list_dir");
        assert_eq!(v["tool_choice"], "auto");
    }

    #[test]
    fn request_body_omits_tools_by_default() {
        // Fence-mode callers (the default today) leave tools unset
        // so vLLM / LM Studio's tool-call parser stays dormant and
        // the wire shape is identical to the pre-Phase-B wire body.
        let body = empty_body("qwen3.6", 64);
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("\"tools\""), "json: {json}");
        assert!(!json.contains("\"tool_choice\""), "json: {json}");
    }

    #[test]
    fn role_str_maps_each_role() {
        assert_eq!(role_str(LlmRole::System), "system");
        assert_eq!(role_str(LlmRole::User), "user");
        assert_eq!(role_str(LlmRole::Assistant), "assistant");
    }

    #[test]
    fn trim_trailing_slash_keeps_paths_intact() {
        assert_eq!(trim_trailing_slash("http://x/v1"), "http://x/v1");
        assert_eq!(trim_trailing_slash("http://x/v1/"), "http://x/v1");
    }

    fn msg(role: LlmRole, content: &str) -> LlmMessage {
        LlmMessage {
            role,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn merge_leading_system_collapses_multiple_to_one() {
        // vLLM / qwen chat templates require a single leading system
        // message; the orchestrator emits 4-5. Verify we collapse
        // them with the visible separator preserved.
        let messages = vec![
            msg(LlmRole::System, "first"),
            msg(LlmRole::System, "second"),
            msg(LlmRole::System, "third"),
            msg(LlmRole::User, "hi"),
        ];
        let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
        assert_eq!(merged.len(), 2);
        assert_eq!(role_str(merged[0].role), "system");
        assert!(merged[0].content.contains("first"));
        assert!(merged[0].content.contains("second"));
        assert!(merged[0].content.contains("third"));
        assert!(merged[0].content.contains("---"));
        assert_eq!(role_str(merged[1].role), "user");
        assert_eq!(merged[1].content.as_str(), "hi");
    }

    #[test]
    fn merge_leading_system_passes_through_single_system() {
        let messages = vec![msg(LlmRole::System, "only"), msg(LlmRole::User, "hi")];
        let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].content.as_str(), "only");
    }

    #[test]
    fn merge_leading_system_handles_no_system() {
        let messages = vec![msg(LlmRole::User, "hi")];
        let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
        assert_eq!(merged.len(), 1);
        assert_eq!(role_str(merged[0].role), "user");
    }

    #[test]
    fn merge_leading_system_does_not_touch_mid_conversation_system() {
        // If the orchestrator ever emits a system message after a
        // user / assistant turn, leave it alone — the merge fix only
        // addresses the LEADING-stack case. A mid-conversation system
        // message will still get rejected by qwen / vLLM, but the
        // 4xx body will surface in the LlmError diagnostic so we'll
        // catch it instead of silently breaking the stack.
        let messages = vec![
            msg(LlmRole::System, "leading-1"),
            msg(LlmRole::System, "leading-2"),
            msg(LlmRole::User, "first"),
            msg(LlmRole::Assistant, "reply"),
            msg(LlmRole::System, "mid-stream"),
            msg(LlmRole::User, "second"),
        ];
        let merged = prepare_messages_for_openai_compat(&messages, &GEMMA4_MODEL_FAMILY);
        assert_eq!(merged.len(), 5); // 2 leading collapsed -> 1, plus 4 originals
        assert_eq!(role_str(merged[0].role), "system");
        assert!(merged[0].content.contains("leading-1"));
        assert!(merged[0].content.contains("leading-2"));
        assert_eq!(role_str(merged[1].role), "user");
        assert_eq!(role_str(merged[2].role), "assistant");
        assert_eq!(role_str(merged[3].role), "system"); // mid-stream system left in place
        assert_eq!(merged[3].content.as_str(), "mid-stream");
        assert_eq!(role_str(merged[4].role), "user");
    }

    fn choice(content: Option<&str>, reasoning: Option<&str>, finish: Option<&str>) -> Choice {
        Choice {
            message: ResponseMessage {
                content: content.map(String::from),
                reasoning: reasoning.map(String::from),
                tool_calls: None,
            },
            finish_reason: finish.map(String::from),
        }
    }

    #[test]
    fn decode_choice_returns_content_on_normal_stop() {
        let c = choice(Some("hello"), None, Some("stop"));
        assert_eq!(
            decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap(),
            "hello"
        );
    }

    #[test]
    fn decode_choice_falls_back_to_reasoning_when_content_empty() {
        let c = choice(None, Some("thinking text"), Some("stop"));
        assert_eq!(
            decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap(),
            "thinking text"
        );
    }

    #[test]
    fn decode_choice_strips_qwen_think_tags_from_content() {
        let c = choice(Some("<think>plan</think>final answer"), None, Some("stop"));
        assert_eq!(
            decode_choice(Some(c), &QWEN3_6_MODEL_FAMILY).unwrap(),
            "final answer"
        );
    }

    #[test]
    fn decode_choice_errors_on_finish_length_with_content() {
        // The today's-bug case: vLLM returns a partially-written
        // markdown file with finish_reason=length. We must NOT
        // commit the partial bytes; the orchestrator's LlmError
        // path engages instead.
        let c = choice(
            Some("# Coverage\n\n```bash\ncargo tarpaulin"),
            None,
            Some("length"),
        );
        let err = decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("truncated at max_tokens"), "got: {msg}");
        assert!(msg.contains("cargo tarpaulin"), "tail should appear: {msg}");
    }

    #[test]
    fn decode_choice_errors_on_finish_length_with_only_reasoning() {
        let c = choice(None, Some("step 1: ..."), Some("length"));
        let err = decode_choice(Some(c), &GEMMA4_MODEL_FAMILY).unwrap_err();
        assert!(format!("{err}").contains("truncated at max_tokens"));
    }

    #[test]
    fn decode_choice_returns_empty_when_no_choice() {
        assert_eq!(decode_choice(None, &GEMMA4_MODEL_FAMILY).unwrap(), "");
    }
}
