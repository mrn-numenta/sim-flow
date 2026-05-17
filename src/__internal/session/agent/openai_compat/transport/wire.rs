//! Wire-shape structs for the chat-completions request / response
//! body. Kept distinct from the caller-facing `OpenAiCompatibleRequest`
//! so on-wire renames don't bleed into the request builder.

use serde::{Deserialize, Serialize};

use super::super::tool_calls::{NativeToolCall, ToolDescriptor};

#[derive(Debug, Clone, Serialize)]
pub(super) struct ChatRequestBody<'a> {
    pub(super) model: &'a str,
    pub(super) messages: Vec<RequestMessage<'a>>,
    pub(super) stream: bool,
    /// Streaming options. Set when `stream = true` to request
    /// `include_usage = true` so the final SSE chunk carries a
    /// `usage` object (otherwise we'd only have chunk content
    /// and no token totals for metrics). Skipped on non-streaming
    /// requests so the wire body stays byte-identical to the
    /// existing shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<StreamOptions>,
    pub(super) max_tokens: u32,
    /// Optional deterministic-sampling seed. `skip_serializing_if`
    /// keeps the wire body unchanged for servers that reject
    /// unknown fields (some openai-compat proxy shims do); when
    /// unset, no `seed` key appears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) seed: Option<u32>,
    /// `chat_template_kwargs` is vLLM's pass-through for Jinja
    /// template parameters. Setting `enable_thinking: false`
    /// asks the qwen3.6 / deepseek-r1 / similar chat template to
    /// skip the `<think>...</think>` preamble entirely. Other
    /// servers either honor the field (llama.cpp, sglang) or
    /// silently ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) chat_template_kwargs: Option<ChatTemplateKwargs>,
    /// Sampling parameters. All `skip_serializing_if = None` so the
    /// wire shape stays minimal for servers that reject unknown
    /// keys. Populated from the model family's
    /// `non_thinking_sampling` defaults when `disable_thinking` is
    /// on, with env-var per-knob overrides on top. Empty for
    /// families with `non_thinking_sampling: None` -- those let the
    /// server's defaults stand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) min_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) repetition_penalty: Option<f32>,
    /// Tool catalog. When `Some` and non-empty, the server is
    /// instructed to consider these functions as tool-call targets.
    /// `skip_serializing_if = None` keeps the wire body unchanged
    /// for fence-mode requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<ToolDescriptor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<&'static str>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct StreamOptions {
    /// Ask the server to emit a final SSE chunk carrying a `usage`
    /// object once streaming finishes. Lets us populate
    /// `LlmCallMetrics.tokens_{in,out}` for streamed calls; without
    /// this we'd only see deltas and miss the totals.
    pub(super) include_usage: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct ChatTemplateKwargs {
    /// Set when the caller wants the model's chat template to skip
    /// the `<think>...</think>` preamble entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) enable_thinking: Option<bool>,
    /// Token budget for the reasoning preamble, when the template
    /// understands it. The serializer omits the field when `None`
    /// so requests without a budget keep the historical wire body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thinking_budget: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RequestMessage<'a> {
    pub(super) role: &'a str,
    pub(super) content: &'a str,
    /// On `role = "tool"` messages: the call id this message is
    /// replying to. Pairs with the assistant's `tool_calls[i].id`
    /// from the prior turn. OpenAI's spec requires this for the
    /// model to associate the result with its originating call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<&'a str>,
    /// On `role = "assistant"` messages: the prior tool_calls this
    /// turn emitted, echoed back so the model sees its own
    /// in-flight call requests. Empty when the turn produced no
    /// tool calls (fence-mode or plain text).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<RequestToolCall<'a>>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RequestToolCall<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<&'a str>,
    #[serde(rename = "type")]
    pub(super) kind: &'static str, // always "function"
    pub(super) function: RequestToolFunction<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RequestToolFunction<'a> {
    pub(super) name: &'a str,
    pub(super) arguments: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ChatResponseBody {
    pub(super) choices: Vec<Choice>,
    /// `usage` is part of the OpenAI chat-completions response shape.
    /// vLLM and LM Studio both populate it. Optional because some
    /// pre-1.0 servers (and proxy shims) omit it; we surface what we
    /// can without erroring.
    #[serde(default)]
    pub(super) usage: Option<UsageBody>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct UsageBody {
    #[serde(default)]
    pub(super) prompt_tokens: Option<u32>,
    #[serde(default)]
    pub(super) completion_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Choice {
    pub(super) message: ResponseMessage,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
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
///
/// vLLM with `--reasoning-parser qwen3` (and OpenAI's reasoning-effort
/// API) names the field `reasoning_content`, not `reasoning`. The
/// alias accepts both shapes so a server change between minor
/// versions doesn't silently drop thinking output.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ResponseMessage {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default, alias = "reasoning_content")]
    pub(super) reasoning: Option<String>,
    /// Native tool calls returned by `--enable-auto-tool-choice` /
    /// OpenAI's tool-use endpoint. May be empty or omitted on
    /// non-tool turns; we tolerate either shape.
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<NativeToolCall>>,
}
