//! The chat-completions round-trip: build the wire body from the
//! request, post it via `ureq`, read the response with size cap,
//! decode the first choice, and surface a clear error for the
//! `finish_reason == "length"` truncation case.

use super::super::super::super::agent::LlmCallMetrics;
use super::super::super::super::agent::{
    normalize_response_text, prepare_messages_for_openai_compat, resolve_model_family,
};
use super::super::tool_calls::NativeToolCall;
use crate::session::protocol::LlmRole;
use crate::{Error, Result};

use super::request::OpenAiCompatibleRequest;
use super::wire::{
    ChatRequestBody, ChatResponseBody, ChatTemplateKwargs, Choice, RequestMessage, RequestToolCall,
    RequestToolFunction,
};

/// What `dispatch_chat_with_tools` returns. The thin
/// back-compat `dispatch_chat` wrapper below discards `tool_calls`
/// and returns just the `(text, metrics)` pair.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<NativeToolCall>,
    pub metrics: LlmCallMetrics,
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
    // for families that have a thinking section to manage. For
    // generic / non-thinking models the kwargs are meaningless and
    // some servers reject unknown template kwargs.
    let chat_template_kwargs = if model_family.supports_thinking_controls {
        let mut kwargs = ChatTemplateKwargs::default();
        if req.disable_thinking {
            kwargs.enable_thinking = Some(false);
        }
        // When thinking is enabled (the default) and the caller
        // supplied a budget, pass it through so the model self-
        // truncates instead of blowing past `max_tokens` while
        // mid-think. Skipped when thinking is disabled -- a budget
        // for a section the template skips is contradictory.
        if !req.disable_thinking && req.thinking_budget.is_some() {
            kwargs.thinking_budget = req.thinking_budget;
        }
        if kwargs.enable_thinking.is_some() || kwargs.thinking_budget.is_some() {
            Some(kwargs)
        } else {
            None
        }
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
    // Temporary debug print: confirm the native-mode wire is
    // sending tools on each request. Gated on
    // SIM_FLOW_DEBUG_TOOLS=1 so production stderr stays quiet.
    if matches!(
        std::env::var("SIM_FLOW_DEBUG_TOOLS").ok().as_deref(),
        Some("1")
    ) {
        let tools_count = body.tools.as_ref().map(|t| t.len()).unwrap_or(0);
        eprintln!(
            "  [debug] llm_request: tools_count={tools_count}, tool_choice={:?}",
            body.tool_choice,
        );
    }
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
pub(super) fn decode_choice(
    choice: Option<Choice>,
    model_family: &super::super::super::adaptation::ModelFamilyProfile,
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

pub(super) fn role_str(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    }
}

pub(super) fn trim_trailing_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

/// Return the last `<= max` bytes of `s`, walking forward to the
/// nearest `char` boundary so we never split a multi-byte
/// codepoint. The previous implementation used `&s[s.len() - max..]`
/// which panics when the cut lands inside a UTF-8 char. Both call
/// sites pass LLM-controlled error bodies, so a non-ASCII byte
/// from vLLM / LM Studio / a stray emoji could crash the
/// orchestrator from the error path itself.
/// See orchestrator audit #7 (2026-05-16).
pub(super) fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}
