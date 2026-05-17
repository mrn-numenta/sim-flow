//! Streaming variant of `dispatch_chat_with_tools`.
//!
//! Posts with `stream: true` + `stream_options: { include_usage: true }`,
//! reads the OpenAI-style SSE response (`data: <json>\n\n` frames with
//! a terminating `data: [DONE]`), forwards `delta.content` to the
//! caller's `on_chunk` callback, and accumulates `delta.tool_calls[]`
//! by index until the final chunk fires the `usage` block.
//!
//! Cancel: the SSE reader runs on a worker thread that ships parsed
//! events through an mpsc channel. The main thread polls the cancel
//! flag on every event. On a flip, the worker is abandoned (its
//! in-flight `ureq` reader eventually completes and its response is
//! dropped) and the dispatcher returns the partial response with
//! `metrics.cancelled = true`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::super::super::super::agent::{
    LlmCallMetrics, StreamingChunk, normalize_response_text, prepare_messages_for_openai_compat,
    resolve_model_family,
};
use super::super::tool_calls::NativeToolCall;
use crate::{Error, Result};

use super::dispatch::{ChatResponse, role_str, tail, trim_trailing_slash};
use super::request::OpenAiCompatibleRequest;
use super::wire::{
    ChatRequestBody, ChatTemplateKwargs, RequestMessage, RequestToolCall, RequestToolFunction,
    StreamOptions,
};

/// Streaming variant of [`super::dispatch_chat_with_tools`]. Drains
/// the response as SSE frames, forwards `delta.content` chunks to
/// `on_chunk`, accumulates tool-call deltas by index, and returns
/// the full assembled `(text, tool_calls, metrics)` once the stream
/// ends.
pub fn dispatch_chat_with_tools_streaming(
    req: OpenAiCompatibleRequest<'_>,
    cancel_flag: Option<Arc<AtomicBool>>,
    on_chunk: &mut dyn FnMut(StreamingChunk),
) -> Result<ChatResponse> {
    let started = Instant::now();
    let model_family = resolve_model_family(req.model_family_id, Some(req.model));
    let prepared_messages = prepare_messages_for_openai_compat(req.messages, model_family);
    let chat_template_kwargs = if model_family.supports_thinking_controls {
        let mut kwargs = ChatTemplateKwargs::default();
        if req.disable_thinking {
            kwargs.enable_thinking = Some(false);
        }
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
        stream: true,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
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

    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| Error::Client(format!("openai-compat: serialize request: {e}")))?;
    let url = format!("{}/chat/completions", trim_trailing_slash(req.base_url));
    let api_key_owned: Option<String> = req.api_key.map(|k| k.to_string());

    let (event_tx, event_rx) = std::sync::mpsc::channel::<Result<SsePayload>>();
    std::thread::spawn(move || {
        let mut request = ureq::post(&url)
            .set("content-type", "application/json")
            .set("accept", "text/event-stream");
        if let Some(ref key) = api_key_owned {
            request = request.set("authorization", &format!("Bearer {key}"));
        }
        let resp = match request.send_bytes(&body_bytes) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                let _ = event_tx.send(Err(Error::Client(format!(
                    "openai-compat: server returned {code}: {}",
                    tail(&body, 2048),
                ))));
                return;
            }
            Err(e) => {
                let _ = event_tx.send(Err(Error::Client(format!(
                    "openai-compat: HTTP error: {e}"
                ))));
                return;
            }
        };
        if !(200..300).contains(&resp.status()) {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            let _ = event_tx.send(Err(Error::Client(format!(
                "openai-compat: server returned {status}: {body}"
            ))));
            return;
        }
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(resp.into_reader());
        let mut line = String::new();
        let mut buffered_data = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        if !buffered_data.is_empty() {
                            let data = std::mem::take(&mut buffered_data);
                            if data.trim() == "[DONE]" {
                                let _ = event_tx.send(Ok(SsePayload::Done));
                                return;
                            }
                            if event_tx.send(Ok(SsePayload::Chunk(data))).is_err() {
                                return;
                            }
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("data: ") {
                        if !buffered_data.is_empty() {
                            buffered_data.push('\n');
                        }
                        buffered_data.push_str(rest);
                    }
                    // Comments / id: / event: fields are ignored.
                }
                Err(err) => {
                    let _ = event_tx.send(Err(Error::Client(format!(
                        "openai-compat: SSE read failed: {err}"
                    ))));
                    return;
                }
            }
        }
    });

    let mut text = String::new();
    let mut tool_calls_by_index: std::collections::BTreeMap<u32, ToolCallAccumulator> =
        std::collections::BTreeMap::new();
    let mut prompt_tokens: Option<u32> = None;
    let mut completion_tokens: Option<u32> = None;
    let mut finish_reason: Option<String> = None;
    let mut cancelled = false;

    loop {
        if let Some(ref flag) = cancel_flag
            && flag.load(Ordering::Acquire)
        {
            cancelled = true;
            break;
        }
        match event_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(SsePayload::Done)) => break,
            Ok(Ok(SsePayload::Chunk(data))) => {
                handle_openai_compat_sse_chunk(
                    &data,
                    &mut text,
                    &mut tool_calls_by_index,
                    &mut prompt_tokens,
                    &mut completion_tokens,
                    &mut finish_reason,
                    on_chunk,
                )?;
            }
            Ok(Err(err)) => return Err(err),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Match `dispatch_chat_with_tools`' truncation policy: refuse a
    // partial response on `finish_reason == length`. Streaming gives
    // us better visibility (we know exactly how many tokens we got)
    // but the orchestrator's contract is the same -- never commit a
    // truncated turn.
    if !cancelled
        && let Some(reason) = finish_reason.as_deref()
        && reason == "length"
    {
        return Err(Error::Client(format!(
            "openai-compat: response truncated at max_tokens (finish_reason=length). \
             Refusing to commit a partial response. Raise SIM_FLOW_MAX_TOKENS, ask the \
             agent to write fewer files per turn, or simplify the prompt. Tail: {}",
            tail(&text, 512),
        )));
    }

    let normalized_text = if text.is_empty() {
        text
    } else {
        normalize_response_text(model_family, &text)
    };
    let tool_calls: Vec<NativeToolCall> = tool_calls_by_index
        .into_values()
        .map(NativeToolCall::from)
        .collect();
    let metrics = LlmCallMetrics {
        tokens_in: prompt_tokens,
        tokens_out: completion_tokens,
        wall_ms: started.elapsed().as_millis() as u64,
        cancelled,
    };
    Ok(ChatResponse {
        text: normalized_text,
        tool_calls,
        metrics,
    })
}

enum SsePayload {
    Chunk(String),
    Done,
}

/// Per-`tool_calls[].index` accumulator. OpenAI streams tool calls
/// as deltas keyed by `index`; the first delta usually carries the
/// `id` and `function.name`, subsequent deltas append to
/// `function.arguments`. The final assembled NativeToolCall has the
/// concatenated arguments JSON string.
#[derive(Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl From<ToolCallAccumulator> for NativeToolCall {
    fn from(value: ToolCallAccumulator) -> Self {
        NativeToolCall {
            id: value.id,
            kind: Some("function".to_string()),
            function: super::super::tool_calls::NativeToolFunction {
                name: value.name.unwrap_or_default(),
                arguments: if value.arguments.is_empty() {
                    "{}".to_string()
                } else {
                    value.arguments
                },
            },
        }
    }
}

/// Parse a single SSE chunk payload (the JSON after `data: `) and
/// fold its deltas into the running accumulators. Returns `Err` only
/// on JSON parse failure; an empty payload is silently skipped.
fn handle_openai_compat_sse_chunk(
    data: &str,
    text: &mut String,
    tool_calls: &mut std::collections::BTreeMap<u32, ToolCallAccumulator>,
    prompt_tokens: &mut Option<u32>,
    completion_tokens: &mut Option<u32>,
    finish_reason: &mut Option<String>,
    on_chunk: &mut dyn FnMut(StreamingChunk),
) -> Result<()> {
    if data.trim().is_empty() {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(data).map_err(|err| {
        Error::Client(format!(
            "openai-compat: malformed SSE chunk: {err}; data: {}",
            tail(data, 512)
        ))
    })?;
    if let Some(usage) = value.get("usage")
        && let Some(usage_obj) = usage.as_object()
    {
        if let Some(t) = usage_obj.get("prompt_tokens").and_then(|v| v.as_u64()) {
            *prompt_tokens = Some(t as u32);
        }
        if let Some(t) = usage_obj.get("completion_tokens").and_then(|v| v.as_u64()) {
            *completion_tokens = Some(t as u32);
        }
    }
    let Some(choices) = value.get("choices").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    let Some(choice) = choices.first() else {
        return Ok(());
    };
    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        *finish_reason = Some(reason.to_string());
    }
    let Some(delta) = choice.get("delta").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    if let Some(content) = delta.get("content").and_then(|v| v.as_str())
        && !content.is_empty()
    {
        text.push_str(content);
        on_chunk(StreamingChunk::Text(content.to_string()));
    }
    // vLLM with `--reasoning-parser qwen3` (and OpenAI's reasoning-
    // effort API) splits the model's `<think>...</think>` output into
    // a separate channel that streams as `delta.reasoning_content`.
    // Some proxies / older builds use the shorter `delta.reasoning`
    // name; accept either to stay tolerant of server-side variation.
    let reasoning_delta = delta
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .or_else(|| delta.get("reasoning").and_then(|v| v.as_str()));
    if let Some(piece) = reasoning_delta
        && !piece.is_empty()
    {
        on_chunk(StreamingChunk::Reasoning(piece.to_string()));
    }
    if let Some(tc_array) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tc_array {
            let Some(index) = tc.get("index").and_then(|v| v.as_u64()) else {
                continue;
            };
            let entry = tool_calls.entry(index as u32).or_default();
            if let Some(id) = tc.get("id").and_then(|v| v.as_str())
                && entry.id.is_none()
            {
                entry.id = Some(id.to_string());
            }
            if let Some(name) = tc.pointer("/function/name").and_then(|v| v.as_str())
                && entry.name.is_none()
            {
                entry.name = Some(name.to_string());
            }
            if let Some(args) = tc.pointer("/function/arguments").and_then(|v| v.as_str()) {
                entry.arguments.push_str(args);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_chunk_handler_accumulates_text_delta() {
        let mut text = String::new();
        let mut tool_calls = std::collections::BTreeMap::new();
        let mut prompt: Option<u32> = None;
        let mut completion: Option<u32> = None;
        let mut finish: Option<String> = None;
        let mut chunks: Vec<String> = Vec::new();
        let mut reasoning_chunks: Vec<String> = Vec::new();
        let mut on_chunk = |c: StreamingChunk| match c {
            StreamingChunk::Text(t) => chunks.push(t),
            StreamingChunk::Reasoning(t) => reasoning_chunks.push(t),
        };
        for data in [
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"Hello"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":", world!"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":4}}"#,
        ] {
            handle_openai_compat_sse_chunk(
                data,
                &mut text,
                &mut tool_calls,
                &mut prompt,
                &mut completion,
                &mut finish,
                &mut on_chunk,
            )
            .unwrap();
        }
        assert_eq!(text, "Hello, world!");
        assert_eq!(chunks, vec!["Hello".to_string(), ", world!".to_string()]);
        assert!(reasoning_chunks.is_empty());
        assert_eq!(prompt, Some(12));
        assert_eq!(completion, Some(4));
        assert_eq!(finish.as_deref(), Some("stop"));
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn sse_chunk_handler_emits_reasoning_chunks_from_reasoning_content_field() {
        // vLLM with `--reasoning-parser qwen3` streams the model's
        // thinking text as `delta.reasoning_content` deltas, interleaved
        // with `delta.content` once the visible answer begins. The
        // parser must surface both channels distinctly so the
        // orchestrator can forward them to separate UI surfaces.
        let mut text = String::new();
        let mut tool_calls = std::collections::BTreeMap::new();
        let mut prompt: Option<u32> = None;
        let mut completion: Option<u32> = None;
        let mut finish: Option<String> = None;
        let mut text_chunks: Vec<String> = Vec::new();
        let mut reasoning_chunks: Vec<String> = Vec::new();
        let mut on_chunk = |c: StreamingChunk| match c {
            StreamingChunk::Text(t) => text_chunks.push(t),
            StreamingChunk::Reasoning(t) => reasoning_chunks.push(t),
        };
        for data in [
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"Let me "}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"think..."}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"The answer "}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"is 42."}}]}"#,
            // Alias path: some proxies use the shorter `reasoning`
            // field name; treat it identically.
            r#"{"choices":[{"index":0,"delta":{"reasoning":" (afterthought)"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ] {
            handle_openai_compat_sse_chunk(
                data,
                &mut text,
                &mut tool_calls,
                &mut prompt,
                &mut completion,
                &mut finish,
                &mut on_chunk,
            )
            .unwrap();
        }
        assert_eq!(text, "The answer is 42.");
        assert_eq!(
            text_chunks,
            vec!["The answer ".to_string(), "is 42.".to_string()]
        );
        assert_eq!(
            reasoning_chunks,
            vec![
                "Let me ".to_string(),
                "think...".to_string(),
                " (afterthought)".to_string(),
            ]
        );
        assert_eq!(finish.as_deref(), Some("stop"));
    }

    #[test]
    fn sse_chunk_handler_accumulates_tool_call_arguments_by_index() {
        let mut text = String::new();
        let mut tool_calls = std::collections::BTreeMap::new();
        let mut prompt: Option<u32> = None;
        let mut completion: Option<u32> = None;
        let mut finish: Option<String> = None;
        let mut chunks: Vec<String> = Vec::new();
        let mut on_chunk = |c: StreamingChunk| match c {
            StreamingChunk::Text(t) => chunks.push(t),
            StreamingChunk::Reasoning(_) => {}
        };
        for data in [
            // First delta carries id + name + opening args fragment.
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"sr"}}]}}]}"#,
            // Second delta appends to args.
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"c/lib.rs\"}"}}]}}]}"#,
            // Finish chunk closes the choice.
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        ] {
            handle_openai_compat_sse_chunk(
                data,
                &mut text,
                &mut tool_calls,
                &mut prompt,
                &mut completion,
                &mut finish,
                &mut on_chunk,
            )
            .unwrap();
        }
        assert!(text.is_empty());
        assert!(chunks.is_empty());
        assert_eq!(tool_calls.len(), 1);
        let acc = tool_calls.remove(&0).unwrap();
        let call: NativeToolCall = acc.into();
        assert_eq!(call.id.as_deref(), Some("call_a"));
        assert_eq!(call.function.name, "read_file");
        assert_eq!(call.function.arguments, r#"{"path":"src/lib.rs"}"#);
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }
}
