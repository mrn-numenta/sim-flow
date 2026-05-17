//! `AnthropicAgent` -- direct Anthropic Messages API client.
//!
//! Hits `https://api.anthropic.com/v1/messages` over HTTPS rather
//! than shelling out to the `claude` CLI (which is what
//! [`ClaudeAgent`](super::claude::ClaudeAgent) does). Two reasons we
//! need both:
//!
//! - The CLI is the right path for users with a Claude Code /
//!   Claude Max subscription -- it shares their session quota and
//!   doesn't require a separate API key.
//! - The API is the right path for headless / CI / cost-controlled
//!   runs -- the call goes straight against an
//!   `ANTHROPIC_API_KEY` quota with per-token billing, no
//!   subscription surface in the loop.
//!
//! The model-robustness study (`docs/brainstorming/model-robustness-study.md`)
//! Phase 4 uses this agent so Opus 4.7 runs are billed against the
//! API key the study budget targets.
//!
//! ## Wire shape
//!
//! POST `/v1/messages` with:
//!
//! - Headers: `x-api-key`, `anthropic-version: 2023-06-01`,
//!   `content-type: application/json`.
//! - Body: `{ model, max_tokens, system?, messages, ... }`.
//!   System messages get merged into the top-level `system` string
//!   (Anthropic's Messages API does NOT accept role=system inside
//!   the messages array, unlike OpenAI's chat-completions).
//!
//! Response: `{ content: [{ type: "text", text: "..." }, ...], usage: { input_tokens, output_tokens }, stop_reason }`.
//! We concatenate every `type=text` block into a single string.
//! `stop_reason == "max_tokens"` is treated as a hard error (mirrors
//! the openai-compat agent's truncation policy) -- the orchestrator
//! cannot safely commit a partial response, so we surface it via
//! `Err(Error::Client)` and let the auto-driver decide whether to
//! retry / flip to manual.

pub mod tool_use;

use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use self::tool_use::{
    AnthropicToolDescriptor, AssistantContentBlock, ResponseContentBlock, UserContentBlock,
};
use super::cancel::run_cancellable;
use super::{
    AdvertisedToolCall, AgentAdaptationSummary, CliAgent, LlmCallMetrics, StreamingChunk,
    ToolAdvertise, apply_reasoning_history_policy, resolve_model_family,
};
use crate::keys::{Provider, resolve_api_key};
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::{Error, Result};

pub const DEFAULT_API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const DEFAULT_API_VERSION: &str = "2023-06-01";
/// Anthropic Messages API requires `max_tokens` on every request.
/// Default raised 8192 -> 32768 after the K=1 Opus 4.7 smoke run
/// hit `stop_reason=max_tokens` on 5/5 critique passes (4-blocker
/// findings + remediation guidance + JSON shape blow past 8K).
/// Opus 4.7 caps at 32K output server-side; the cap lands at the
/// model's wall rather than ours. Override via `SIM_FLOW_MAX_TOKENS`
/// for narrower-context Anthropic models if any ship later.
pub const DEFAULT_MAX_TOKENS: u32 = 32_768;
/// Default model when none is specified. Picked to match the
/// brainstorming doc's Phase 4 lineup.
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

pub struct AnthropicAgent {
    api_url: String,
    model: String,
    model_family_id: Option<String>,
    api_key: Option<String>,
    /// Shared mid-dispatch cancel flag. Polled by `run_cancellable`
    /// while the `ureq::post(...).send_json(...)` worker thread is
    /// outstanding. On flip the worker is abandoned (the response
    /// body eventually arrives at the orphaned thread and is
    /// dropped) and the dispatcher returns `Error::Cancelled`.
    cancel_flag: Option<Arc<AtomicBool>>,
}

impl AnthropicAgent {
    /// Construct an agent. `api_url` defaults to
    /// [`DEFAULT_API_URL`] when `None`; useful for tests pointing at
    /// a local mock server. `model` defaults to [`DEFAULT_MODEL`].
    ///
    /// Resolves the API key once, eagerly, so a missing-key failure
    /// surfaces at construction time (where the operator can fix it)
    /// rather than mid-run after the orchestrator has already
    /// dispatched a turn. A subsequent `dispatch` with no key still
    /// surfaces a clear `Error::Client` rather than panicking.
    pub fn new(
        api_url: Option<String>,
        model: Option<String>,
        model_family_id: Option<String>,
    ) -> Self {
        Self::new_with_cancel(api_url, model, model_family_id, None)
    }

    pub fn new_with_cancel(
        api_url: Option<String>,
        model: Option<String>,
        model_family_id: Option<String>,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Self {
        let api_key = resolve_api_key(Provider::Anthropic)
            .ok()
            .flatten()
            .map(|r| r.key);
        Self {
            api_url: api_url.unwrap_or_else(|| DEFAULT_API_URL.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            model_family_id,
            api_key,
            cancel_flag,
        }
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Shared dispatch path used by both `dispatch` (no tools) and
    /// `dispatch_with_tools`. Returns `(assistant_text, tool_calls,
    /// metrics)`. The tools list is forwarded to Anthropic via the
    /// request body's `tools` field when `Some`.
    fn dispatch_inner(
        &self,
        messages: &[LlmMessage],
        tools: Option<Vec<AnthropicToolDescriptor>>,
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let started = Instant::now();
        let Some(api_key) = self.api_key.as_deref() else {
            return Err(Error::Client(
                "anthropic backend: no API key found. Set the `ANTHROPIC_API_KEY` \
                 env var or run `sim-flow keys set anthropic <key>` to persist one."
                    .into(),
            ));
        };
        let max_tokens = std::env::var("SIM_FLOW_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);
        let family = resolve_model_family(self.model_family_id.as_deref(), Some(&self.model));
        let prepared = apply_reasoning_history_policy(messages, family);
        let (system, conversation) = split_system_and_messages(&prepared);

        let body = MessagesRequestBody {
            model: &self.model,
            max_tokens,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            messages: conversation,
            tools,
            stream: false,
        };

        // Serialize the request body once in the main thread so the
        // worker closure below doesn't need to borrow `body` (whose
        // 'a lifetime would tie the worker to this stack frame). Send
        // the resulting bytes via `send_bytes` instead of `send_json`.
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|err| Error::Client(format!("anthropic api: serialize body: {err}")))?;
        let url = self.api_url.clone();
        let api_key_owned = api_key.to_string();
        // Wrap the blocking ureq call in `run_cancellable` so the
        // shared cancel flag can abort the dispatch from the
        // control-socket thread mid-call. The worker thread is
        // abandoned on cancel; its eventual response is dropped.
        let resp = run_cancellable(
            self.cancel_flag.clone(),
            move || -> Result<ureq::Response> {
                match ureq::post(&url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &api_key_owned)
                    .set("anthropic-version", DEFAULT_API_VERSION)
                    .send_bytes(&body_bytes)
                {
                    Ok(r) => Ok(r),
                    Err(ureq::Error::Status(code, resp)) => {
                        let body = resp.into_string().unwrap_or_default();
                        Err(Error::Client(format!(
                            "anthropic api returned HTTP {code}: {body}"
                        )))
                    }
                    Err(err) => Err(Error::Client(format!("anthropic api transport: {err}"))),
                }
            },
        )?;
        let body: MessagesResponseBody = resp
            .into_json()
            .map_err(|err| Error::Client(format!("anthropic api: decode response: {err}")))?;
        let text = collect_text(&body.content);
        let tool_calls = collect_tool_uses(&body.content);
        if let Some(stop_reason) = body.stop_reason.as_deref()
            && stop_reason == "max_tokens"
        {
            return Err(Error::Client(format!(
                "anthropic api: response truncated at max_tokens (stop_reason=max_tokens). \
                 Refusing to commit a partial response. Raise SIM_FLOW_MAX_TOKENS (current: {max_tokens}), \
                 ask the agent to write fewer files per turn, or simplify the prompt. \
                 Tail: {tail}",
                tail = text
                    .chars()
                    .rev()
                    .take(280)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>(),
            )));
        }
        let metrics = LlmCallMetrics {
            tokens_in: body.usage.as_ref().and_then(|u| u.input_tokens),
            tokens_out: body.usage.as_ref().and_then(|u| u.output_tokens),
            wall_ms: started.elapsed().as_millis() as u64,
            cancelled: false,
        };
        Ok((text, tool_calls, metrics))
    }

    /// Streaming variant: posts with `stream=true`, reads SSE frames
    /// on a worker thread, and forwards `text_delta` chunks to
    /// `on_chunk` as they arrive. Tool-use blocks (`tool_use` /
    /// `input_json_delta` events) accumulate into the returned
    /// `tool_calls` vector. The shared `cancel_flag` is polled on
    /// each chunk boundary; on flip the reader is abandoned and we
    /// return the partial response with `metrics.cancelled = true`.
    fn dispatch_inner_streaming(
        &self,
        messages: &[LlmMessage],
        tools: Option<Vec<AnthropicToolDescriptor>>,
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let started = Instant::now();
        let Some(api_key) = self.api_key.as_deref() else {
            return Err(Error::Client(
                "anthropic backend: no API key found. Set the `ANTHROPIC_API_KEY` \
                 env var or run `sim-flow keys set anthropic <key>` to persist one."
                    .into(),
            ));
        };
        let max_tokens = std::env::var("SIM_FLOW_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);
        let family = resolve_model_family(self.model_family_id.as_deref(), Some(&self.model));
        let prepared = apply_reasoning_history_policy(messages, family);
        let (system, conversation) = split_system_and_messages(&prepared);

        let body = MessagesRequestBody {
            model: &self.model,
            max_tokens,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            messages: conversation,
            tools,
            stream: true,
        };
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|err| Error::Client(format!("anthropic api: serialize body: {err}")))?;
        let url = self.api_url.clone();
        let api_key_owned = api_key.to_string();

        // Reader thread: opens the HTTP stream, splits the response
        // body into SSE frames, parses each into a `RawSseEvent`, and
        // forwards through a channel. The main thread polls the
        // channel + cancel flag so mid-stream cancellation returns
        // promptly with whatever content has already streamed.
        let (event_tx, event_rx) = std::sync::mpsc::channel::<Result<RawSseEvent>>();
        std::thread::spawn(move || {
            let resp = match ureq::post(&url)
                .set("content-type", "application/json")
                .set("x-api-key", &api_key_owned)
                .set("anthropic-version", DEFAULT_API_VERSION)
                .set("accept", "text/event-stream")
                .send_bytes(&body_bytes)
            {
                Ok(r) => r,
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    let _ = event_tx.send(Err(Error::Client(format!(
                        "anthropic api returned HTTP {code}: {body}"
                    ))));
                    return;
                }
                Err(err) => {
                    let _ = event_tx.send(Err(Error::Client(format!(
                        "anthropic api transport: {err}"
                    ))));
                    return;
                }
            };
            let mut reader = std::io::BufReader::new(resp.into_reader());
            let mut current_event: Option<String> = None;
            let mut current_data = String::new();
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\r', '\n']);
                        if trimmed.is_empty() {
                            if let Some(event) = current_event.take() {
                                let data = std::mem::take(&mut current_data);
                                if event_tx.send(Ok(RawSseEvent { event, data })).is_err() {
                                    return;
                                }
                            }
                        } else if let Some(rest) = trimmed.strip_prefix("event: ") {
                            current_event = Some(rest.to_string());
                        } else if let Some(rest) = trimmed.strip_prefix("data: ") {
                            if !current_data.is_empty() {
                                current_data.push('\n');
                            }
                            current_data.push_str(rest);
                        }
                        // Other SSE fields (id:, retry:) are ignored;
                        // Anthropic doesn't emit them today.
                    }
                    Err(err) => {
                        let _ = event_tx.send(Err(Error::Client(format!(
                            "anthropic api: SSE read failed: {err}"
                        ))));
                        return;
                    }
                }
            }
        });

        let mut blocks: BTreeMap<usize, BlockAccumulator> = BTreeMap::new();
        let mut text = String::new();
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut stop_reason: Option<String> = None;
        let mut cancelled = false;

        loop {
            if let Some(ref flag) = self.cancel_flag
                && flag.load(Ordering::Acquire)
            {
                cancelled = true;
                break;
            }
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(raw)) => {
                    handle_anthropic_sse_event(
                        &raw,
                        &mut blocks,
                        &mut text,
                        &mut input_tokens,
                        &mut output_tokens,
                        &mut stop_reason,
                        on_chunk,
                    )?;
                    if matches!(raw.event.as_str(), "message_stop") {
                        break;
                    }
                }
                Ok(Err(err)) => return Err(err),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // max_tokens stop reason is a hard error on the non-streaming
        // path (we refuse to commit a partial response). Streaming
        // arguably DOES have a partial response and the user can
        // make do, but keeping symmetry with `dispatch_inner` here
        // simplifies the orchestrator's contract: a streaming call
        // either returns clean text or `Cancelled`-flagged metrics
        // (user-driven) or `Err` (server-side problem). We surface
        // it as Err and let the operator retry with a higher cap.
        if !cancelled
            && let Some(reason) = stop_reason.as_deref()
            && reason == "max_tokens"
        {
            return Err(Error::Client(format!(
                "anthropic api: response truncated at max_tokens (stop_reason=max_tokens). \
                 Refusing to commit a partial response. Raise SIM_FLOW_MAX_TOKENS (current: {max_tokens}), \
                 ask the agent to write fewer files per turn, or simplify the prompt. \
                 Tail: {tail}",
                tail = text
                    .chars()
                    .rev()
                    .take(280)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>(),
            )));
        }

        let tool_calls = collect_tool_calls_from_blocks(blocks);
        let metrics = LlmCallMetrics {
            tokens_in: input_tokens,
            tokens_out: output_tokens,
            wall_ms: started.elapsed().as_millis() as u64,
            cancelled,
        };
        Ok((text, tool_calls, metrics))
    }
}

/// Single SSE event delivered by the reader thread. Anthropic emits
/// one `event: <name>\n` line per frame followed by one `data: <json>\n`
/// line (sometimes multi-line for long deltas) terminated by a blank
/// line; this is the parsed result.
struct RawSseEvent {
    event: String,
    data: String,
}

/// Per-`content_block.index` accumulator. Text blocks collect
/// `text_delta`s into a String; tool_use blocks collect the
/// `input_json_delta`s into a JSON-fragment String that's only valid
/// once the corresponding `content_block_stop` arrives.
enum BlockAccumulator {
    Text(String),
    ToolUse {
        id: Option<String>,
        name: Option<String>,
        partial_json: String,
    },
}

fn handle_anthropic_sse_event(
    raw: &RawSseEvent,
    blocks: &mut BTreeMap<usize, BlockAccumulator>,
    text: &mut String,
    input_tokens: &mut Option<u32>,
    output_tokens: &mut Option<u32>,
    stop_reason: &mut Option<String>,
    on_chunk: &mut dyn FnMut(StreamingChunk),
) -> Result<()> {
    if raw.data.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(&raw.data).map_err(|err| {
        Error::Client(format!(
            "anthropic api: malformed SSE data for event `{}`: {err}",
            raw.event
        ))
    })?;
    match raw.event.as_str() {
        "message_start" => {
            if let Some(t) = value
                .pointer("/message/usage/input_tokens")
                .and_then(|v| v.as_u64())
            {
                *input_tokens = Some(t as u32);
            }
            if let Some(t) = value
                .pointer("/message/usage/output_tokens")
                .and_then(|v| v.as_u64())
            {
                *output_tokens = Some(t as u32);
            }
        }
        "content_block_start" => {
            let index = value
                .pointer("/index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let block_type = value
                .pointer("/content_block/type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match block_type {
                "text" => {
                    let initial = value
                        .pointer("/content_block/text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !initial.is_empty() {
                        text.push_str(&initial);
                        on_chunk(StreamingChunk::Text(initial.clone()));
                    }
                    blocks.insert(index, BlockAccumulator::Text(initial));
                }
                "tool_use" => {
                    let id = value
                        .pointer("/content_block/id")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let name = value
                        .pointer("/content_block/name")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    blocks.insert(
                        index,
                        BlockAccumulator::ToolUse {
                            id,
                            name,
                            partial_json: String::new(),
                        },
                    );
                }
                _ => {
                    // Unknown block type (image, etc.); skip.
                }
            }
        }
        "content_block_delta" => {
            let index = value
                .pointer("/index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let delta_type = value
                .pointer("/delta/type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match delta_type {
                "text_delta" => {
                    let piece = value
                        .pointer("/delta/text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !piece.is_empty() {
                        text.push_str(piece);
                        if let Some(BlockAccumulator::Text(buf)) = blocks.get_mut(&index) {
                            buf.push_str(piece);
                        }
                        on_chunk(StreamingChunk::Text(piece.to_string()));
                    }
                }
                "input_json_delta" => {
                    let piece = value
                        .pointer("/delta/partial_json")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !piece.is_empty()
                        && let Some(BlockAccumulator::ToolUse { partial_json, .. }) =
                            blocks.get_mut(&index)
                    {
                        partial_json.push_str(piece);
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            // Block is finalized in `blocks`; nothing more to do
            // until message_stop folds tool_use blocks into the
            // final tool_calls vector.
        }
        "message_delta" => {
            if let Some(reason) = value.pointer("/delta/stop_reason").and_then(|v| v.as_str()) {
                *stop_reason = Some(reason.to_string());
            }
            if let Some(t) = value
                .pointer("/usage/output_tokens")
                .and_then(|v| v.as_u64())
            {
                *output_tokens = Some(t as u32);
            }
        }
        "message_stop" => {
            // Handled by caller's loop break.
        }
        "ping" => {
            // Keep-alive; nothing to do.
        }
        _ => {
            // Unknown event type. Anthropic adds new events over time
            // (`message_start_overload`, etc.); ignore so a new field
            // doesn't crash the stream parser.
        }
    }
    Ok(())
}

fn collect_tool_calls_from_blocks(
    blocks: BTreeMap<usize, BlockAccumulator>,
) -> Vec<AdvertisedToolCall> {
    blocks
        .into_values()
        .filter_map(|b| match b {
            BlockAccumulator::ToolUse {
                id,
                name,
                partial_json,
            } => {
                let arguments_json = if partial_json.is_empty() {
                    "{}".to_string()
                } else {
                    partial_json
                };
                let name = name.unwrap_or_default();
                Some(AdvertisedToolCall {
                    id,
                    name,
                    arguments_json,
                })
            }
            _ => None,
        })
        .collect()
}

impl CliAgent for AnthropicAgent {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        let (text, _calls, metrics) = self.dispatch_inner(messages, None)?;
        Ok((text, metrics))
    }

    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let wire_tools = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| AnthropicToolDescriptor {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: t.parameters.clone(),
                    })
                    .collect(),
            )
        };
        self.dispatch_inner(messages, wire_tools)
    }

    fn dispatch_streaming(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let wire_tools = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| AnthropicToolDescriptor {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: t.parameters.clone(),
                    })
                    .collect(),
            )
        };
        self.dispatch_inner_streaming(messages, wire_tools, on_chunk)
    }

    fn adaptation_summary(&self) -> Option<AgentAdaptationSummary> {
        let family = resolve_model_family(self.model_family_id.as_deref(), Some(&self.model));
        Some(AgentAdaptationSummary {
            backend: self.name().to_string(),
            // The direct-API path doesn't go through a per-backend
            // runtime profile (no chat-template juggling), so we
            // report a synthetic descriptor that's stable for
            // metrics aggregation.
            runtime_profile_id: "anthropic_messages_api".to_string(),
            model_family_id: family.id.to_string(),
            request_format: "anthropic_messages".to_string(),
            system_prompt_mode: "top-level-system-string".to_string(),
            credential_policy: "anthropic_api_key".to_string(),
            supports_structured_reasoning: false,
            supports_structured_tool_calls: true,
            supports_thinking_controls: family.supports_thinking_controls,
        })
    }
}

/// Split the message stack into Anthropic's required shape: a
/// single `system` string (concatenated leading-system messages with
/// blank-line separators) and a list of user/assistant turns. The
/// Messages API rejects `role=system` inside the messages array, so
/// we have to extract them. Trailing system messages mid-conversation
/// (which the orchestrator does NOT currently emit) are concatenated
/// into the leading system string and a tracing warning is left as a
/// follow-up note in the comment; we don't try to round-trip them as
/// `user`-role turns because that confuses the model more than it
/// helps.
fn split_system_and_messages(messages: &[LlmMessage]) -> (String, Vec<MessagePayload>) {
    let mut system_blocks: Vec<&str> = Vec::new();
    let mut conversation: Vec<MessagePayload> = Vec::new();
    // Anthropic requires consecutive tool_result blocks (in reply to
    // a multi-call assistant turn) to be coalesced into a single
    // user-role message. We accumulate them here and flush when the
    // role changes or the stack ends.
    let mut pending_tool_results: Vec<UserContentBlock> = Vec::new();
    let flush_tool_results = |pending: &mut Vec<UserContentBlock>,
                              conversation: &mut Vec<MessagePayload>| {
        if !pending.is_empty() {
            let blocks = std::mem::take(pending);
            conversation.push(MessagePayload {
                role: "user",
                content: AnthropicMessageContent::User(blocks),
            });
        }
    };
    for msg in messages {
        match msg.role {
            LlmRole::System => {
                flush_tool_results(&mut pending_tool_results, &mut conversation);
                system_blocks.push(msg.content.as_str());
            }
            LlmRole::User => {
                flush_tool_results(&mut pending_tool_results, &mut conversation);
                conversation.push(MessagePayload {
                    role: "user",
                    content: AnthropicMessageContent::Text(msg.content.clone()),
                });
            }
            LlmRole::Assistant => {
                flush_tool_results(&mut pending_tool_results, &mut conversation);
                if msg.tool_calls.is_empty() {
                    // No native tool calls: simple text turn.
                    conversation.push(MessagePayload {
                        role: "assistant",
                        content: AnthropicMessageContent::Text(msg.content.clone()),
                    });
                } else {
                    // Mixed turn: text (if any) followed by one
                    // tool_use block per call. Anthropic's spec
                    // requires tool_use `input` as a JSON object,
                    // NOT a stringified one -- parse the orchestrator's
                    // arguments_json here. On parse failure, fall
                    // back to an empty object so the request still
                    // ships rather than blocking the turn.
                    let mut blocks: Vec<AssistantContentBlock> = Vec::new();
                    if !msg.content.is_empty() {
                        blocks.push(AssistantContentBlock::Text {
                            text: msg.content.clone(),
                        });
                    }
                    for call in &msg.tool_calls {
                        let input = serde_json::from_str(&call.arguments_json)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        blocks.push(AssistantContentBlock::ToolUse {
                            id: call.id.clone().unwrap_or_default(),
                            name: call.name.clone(),
                            input,
                        });
                    }
                    conversation.push(MessagePayload {
                        role: "assistant",
                        content: AnthropicMessageContent::Assistant(blocks),
                    });
                }
            }
            LlmRole::Tool => {
                // Accumulate into the pending tool_result batch.
                // Anthropic requires every prior `tool_use.id` to
                // be answered before the next assistant turn; we
                // emit one user-role message per consecutive run
                // of Tool messages so multi-call turns thread
                // correctly. A Tool message with no tool_call_id
                // (legacy fenced fallback) falls back to an
                // anonymous tool_result rather than a text turn so
                // the model still sees the result content.
                pending_tool_results.push(UserContentBlock::ToolResult {
                    tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                    content: msg.content.clone(),
                    is_error: false,
                });
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut conversation);
    (system_blocks.join("\n\n"), conversation)
}

fn collect_text(blocks: &[ResponseContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if block.kind.as_deref() == Some("text")
            && let Some(text) = &block.text
        {
            out.push_str(text);
        }
    }
    out
}

/// Extract every `tool_use` content block from the response and
/// convert into the vendor-neutral `AdvertisedToolCall` shape the
/// orchestrator dispatches through. `input` (a JSON object on the
/// wire) gets re-serialized as a JSON string to match the same
/// `arguments_json` contract OpenAI's native tool calls use --
/// downstream parsers don't care which vendor produced the call.
fn collect_tool_uses(blocks: &[ResponseContentBlock]) -> Vec<AdvertisedToolCall> {
    let mut out = Vec::new();
    for block in blocks {
        if block.kind.as_deref() != Some("tool_use") {
            continue;
        }
        let Some(name) = block.name.clone() else {
            continue;
        };
        let arguments_json = block
            .input
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());
        out.push(AdvertisedToolCall {
            id: block.id.clone(),
            name,
            arguments_json,
        });
    }
    out
}

#[derive(Debug, Serialize)]
struct MessagesRequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<MessagePayload>,
    /// Tool catalog. `Some` populated only on dispatch_with_tools;
    /// the default `dispatch` path leaves this `None` so the wire
    /// body stays byte-identical to pre-Phase-C requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDescriptor>>,
    /// Server-sent events streaming flag. False (default) returns
    /// a single JSON response body; true causes the server to
    /// emit `event: <name>\ndata: <json>\n\n` frames the client
    /// reads incrementally. Used by `dispatch_streaming` so the
    /// orchestrator can forward text chunks as
    /// `AssistantText { final_chunk: false }` events while the
    /// model is still generating.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Debug, Serialize)]
struct MessagePayload {
    role: &'static str,
    content: AnthropicMessageContent,
}

/// Anthropic accepts message content either as a simple string OR as
/// a heterogeneous array of typed content blocks (text / tool_use /
/// tool_result / image / etc.). We emit the simple string form when
/// possible to keep the wire body minimal and tests stable;
/// tool-aware turns (assistant emissions with tool_use blocks, user
/// replies with tool_result blocks) require the array form.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicMessageContent {
    Text(String),
    Assistant(Vec<AssistantContentBlock>),
    User(Vec<UserContentBlock>),
}

#[derive(Debug, Deserialize)]
struct MessagesResponseBody {
    #[serde(default)]
    content: Vec<ResponseContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageBlock>,
}

#[derive(Debug, Deserialize)]
struct UsageBlock {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn split_collapses_leading_systems_into_one_string() {
        let messages = vec![
            msg(LlmRole::System, "first"),
            msg(LlmRole::System, "second"),
            msg(LlmRole::User, "hello"),
            msg(LlmRole::Assistant, "hi"),
            msg(LlmRole::User, "again"),
        ];
        let (system, convo) = split_system_and_messages(&messages);
        assert_eq!(system, "first\n\nsecond");
        assert_eq!(convo.len(), 3);
        assert_eq!(convo[0].role, "user");
        assert!(matches!(&convo[0].content, AnthropicMessageContent::Text(s) if s == "hello"));
        assert_eq!(convo[1].role, "assistant");
        assert_eq!(convo[2].role, "user");
        assert!(matches!(&convo[2].content, AnthropicMessageContent::Text(s) if s == "again"));
    }

    #[test]
    fn split_with_no_system_returns_empty_system_string() {
        let messages = vec![msg(LlmRole::User, "hi")];
        let (system, convo) = split_system_and_messages(&messages);
        assert_eq!(system, "");
        assert_eq!(convo.len(), 1);
    }

    #[test]
    fn collect_text_concatenates_text_blocks_in_order() {
        let blocks = vec![
            ResponseContentBlock {
                kind: Some("text".into()),
                text: Some("alpha".into()),
                id: None,
                name: None,
                input: None,
            },
            ResponseContentBlock {
                kind: Some("tool_use".into()),
                text: None,
                id: Some("toolu_1".into()),
                name: Some("read_file".into()),
                input: Some(serde_json::json!({"path": "a.md"})),
            },
            ResponseContentBlock {
                kind: Some("text".into()),
                text: Some(" beta".into()),
                id: None,
                name: None,
                input: None,
            },
        ];
        assert_eq!(collect_text(&blocks), "alpha beta");
        let calls = collect_tool_uses(&blocks);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id.as_deref(), Some("toolu_1"));
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments_json, r#"{"path":"a.md"}"#);
    }

    #[test]
    fn request_body_omits_system_when_none() {
        let body = MessagesRequestBody {
            model: "claude-opus-4-7",
            max_tokens: 8192,
            system: None,
            messages: vec![MessagePayload {
                role: "user",
                content: AnthropicMessageContent::Text("hi".into()),
            }],
            tools: None,
            stream: false,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("\"system\""), "json: {json}");
        assert!(!json.contains("\"tools\""), "json: {json}");
        assert!(!json.contains("\"stream\""), "json: {json}");
        assert!(json.contains("\"max_tokens\":8192"));
        assert!(json.contains("\"model\":\"claude-opus-4-7\""));
    }

    #[test]
    fn request_body_includes_system_when_set() {
        let body = MessagesRequestBody {
            model: "claude-opus-4-7",
            max_tokens: 8192,
            system: Some("rules".into()),
            messages: vec![],
            tools: None,
            stream: false,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"system\":\"rules\""));
    }

    #[test]
    fn request_body_includes_tools_when_set() {
        // Pin the Anthropic-specific tool descriptor shape: `name`,
        // `description`, `input_schema`. NOT OpenAI's
        // {type:"function", function:{...}}.
        let body = MessagesRequestBody {
            model: "claude-opus-4-7",
            max_tokens: 8192,
            system: None,
            messages: vec![],
            tools: Some(vec![AnthropicToolDescriptor {
                name: "list_dir".into(),
                description: "List a directory".into(),
                input_schema: serde_json::json!({"type":"object"}),
            }]),
            stream: false,
        };
        let v: serde_json::Value = serde_json::to_value(&body).unwrap();
        assert_eq!(v["tools"][0]["name"], "list_dir");
        assert_eq!(v["tools"][0]["description"], "List a directory");
        assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
        // OpenAI-style wrappers must NOT appear -- a regression there
        // would silently break Anthropic tool-use.
        assert!(v["tools"][0].get("type").is_none());
        assert!(v["tools"][0].get("function").is_none());
        assert!(v["tools"][0].get("parameters").is_none());
    }

    #[test]
    fn assistant_with_tool_calls_serializes_as_block_array() {
        // The assistant turn must serialize its content as an array
        // of typed blocks (text + tool_use) when tool_calls is
        // non-empty; the existing simple-string form would lose the
        // tool_use binding.
        let messages = vec![LlmMessage {
            role: LlmRole::Assistant,
            content: "calling list_dir".into(),
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![crate::session::protocol::LlmToolCall {
                id: Some("toolu_1".into()),
                name: "list_dir".into(),
                arguments_json: r#"{"path":"."}"#.into(),
            }],
        }];
        let (_system, convo) = split_system_and_messages(&messages);
        assert_eq!(convo.len(), 1);
        let json = serde_json::to_value(&convo[0]).unwrap();
        assert_eq!(json["role"], "assistant");
        assert!(json["content"].is_array());
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "calling list_dir");
        assert_eq!(json["content"][1]["type"], "tool_use");
        assert_eq!(json["content"][1]["id"], "toolu_1");
        assert_eq!(json["content"][1]["name"], "list_dir");
        assert_eq!(json["content"][1]["input"]["path"], ".");
    }

    #[test]
    fn tool_role_messages_coalesce_into_one_user_with_tool_result_blocks() {
        // Anthropic requires every prior tool_use.id to be replied
        // to in a SINGLE user message before the next assistant
        // turn. The converter coalesces consecutive Tool-role
        // messages into one user message with N tool_result blocks.
        let messages = vec![
            LlmMessage {
                role: LlmRole::Tool,
                content: "result 1".into(),
                attachments: Vec::new(),
                tool_call_id: Some("toolu_a".into()),
                tool_calls: Vec::new(),
            },
            LlmMessage {
                role: LlmRole::Tool,
                content: "result 2".into(),
                attachments: Vec::new(),
                tool_call_id: Some("toolu_b".into()),
                tool_calls: Vec::new(),
            },
            LlmMessage {
                role: LlmRole::User,
                content: "what next?".into(),
                attachments: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
            },
        ];
        let (_system, convo) = split_system_and_messages(&messages);
        assert_eq!(convo.len(), 2);
        let json = serde_json::to_value(&convo[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert!(json["content"].is_array());
        assert_eq!(json["content"][0]["type"], "tool_result");
        assert_eq!(json["content"][0]["tool_use_id"], "toolu_a");
        assert_eq!(json["content"][0]["content"], "result 1");
        assert_eq!(json["content"][1]["type"], "tool_result");
        assert_eq!(json["content"][1]["tool_use_id"], "toolu_b");
        assert_eq!(json["content"][1]["content"], "result 2");
    }

    #[test]
    fn agent_constructor_falls_back_to_defaults() {
        // No env override -> uses DEFAULT_API_URL + DEFAULT_MODEL.
        let agent = AnthropicAgent::new(None, None, None);
        assert_eq!(agent.api_url(), DEFAULT_API_URL);
        assert_eq!(agent.model(), DEFAULT_MODEL);
    }

    #[test]
    fn agent_constructor_respects_overrides() {
        let agent = AnthropicAgent::new(
            Some("http://localhost:9999/v1/messages".into()),
            Some("claude-opus-4-7".into()),
            None,
        );
        assert_eq!(agent.api_url(), "http://localhost:9999/v1/messages");
        assert_eq!(agent.model(), "claude-opus-4-7");
    }

    #[test]
    fn collect_text_concatenates_only_text_blocks() {
        let blocks = vec![
            ResponseContentBlock {
                kind: Some("text".into()),
                text: Some("hello ".into()),
                id: None,
                name: None,
                input: None,
            },
            ResponseContentBlock {
                kind: Some("tool_use".into()),
                text: None,
                id: Some("toolu_a".into()),
                name: Some("read_file".into()),
                input: Some(serde_json::json!({"path": "x"})),
            },
            ResponseContentBlock {
                kind: Some("text".into()),
                text: Some("world".into()),
                id: None,
                name: None,
                input: None,
            },
        ];
        assert_eq!(collect_text(&blocks), "hello world");
    }

    #[test]
    fn collect_tool_uses_skips_non_tool_blocks_and_handles_missing_input() {
        let blocks = vec![
            ResponseContentBlock {
                kind: Some("text".into()),
                text: Some("preamble".into()),
                id: None,
                name: None,
                input: None,
            },
            ResponseContentBlock {
                kind: Some("tool_use".into()),
                text: None,
                id: Some("toolu_a".into()),
                name: Some("read_file".into()),
                input: Some(serde_json::json!({"path": "x"})),
            },
            ResponseContentBlock {
                // Missing name -> filtered out.
                kind: Some("tool_use".into()),
                text: None,
                id: Some("toolu_b".into()),
                name: None,
                input: Some(serde_json::json!({})),
            },
            ResponseContentBlock {
                // Missing input -> argument string becomes "{}".
                kind: Some("tool_use".into()),
                text: None,
                id: Some("toolu_c".into()),
                name: Some("list_dir".into()),
                input: None,
            },
        ];
        let calls = collect_tool_uses(&blocks);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].id.as_deref(), Some("toolu_a"));
        // Compact JSON: arguments_json comes from Value::to_string().
        assert!(calls[0].arguments_json.contains("\"path\""));
        assert_eq!(calls[1].name, "list_dir");
        assert_eq!(calls[1].arguments_json, "{}");
    }

    #[test]
    fn sse_event_handler_accumulates_text_deltas_and_forwards_chunks() {
        // Drive the SSE event handler through the canonical Anthropic
        // streaming sequence: message_start (carries usage),
        // content_block_start (text block), N x content_block_delta
        // (text_delta), content_block_stop, message_delta (final
        // usage + stop_reason), message_stop. Verify text accumulates
        // correctly, on_chunk receives every delta, and the metrics
        // come out of message_delta's usage block.
        let mut blocks: BTreeMap<usize, BlockAccumulator> = BTreeMap::new();
        let mut text = String::new();
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut stop_reason: Option<String> = None;
        let mut chunks: Vec<String> = Vec::new();
        let mut on_chunk = |c: StreamingChunk| match c {
            StreamingChunk::Text(t) => chunks.push(t),
        };

        for (event, data) in [
            (
                "message_start",
                r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":1}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", world!"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ] {
            handle_anthropic_sse_event(
                &RawSseEvent {
                    event: event.into(),
                    data: data.into(),
                },
                &mut blocks,
                &mut text,
                &mut input_tokens,
                &mut output_tokens,
                &mut stop_reason,
                &mut on_chunk,
            )
            .unwrap();
        }

        assert_eq!(text, "Hello, world!");
        assert_eq!(chunks, vec!["Hello".to_string(), ", world!".to_string()]);
        assert_eq!(input_tokens, Some(12));
        assert_eq!(output_tokens, Some(7));
        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn sse_event_handler_accumulates_tool_use_input_json_deltas() {
        // Anthropic streams tool_use args as `input_json_delta`
        // fragments; the handler must concatenate them in order so
        // the final arguments_json is the complete JSON object.
        let mut blocks: BTreeMap<usize, BlockAccumulator> = BTreeMap::new();
        let mut text = String::new();
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut stop_reason: Option<String> = None;
        let mut chunks: Vec<String> = Vec::new();
        let mut on_chunk = |c: StreamingChunk| match c {
            StreamingChunk::Text(t) => chunks.push(t),
        };
        for (event, data) in [
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_x","name":"read_file","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"sr"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"c/lib.rs\"}"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":1}"#,
            ),
        ] {
            handle_anthropic_sse_event(
                &RawSseEvent {
                    event: event.into(),
                    data: data.into(),
                },
                &mut blocks,
                &mut text,
                &mut input_tokens,
                &mut output_tokens,
                &mut stop_reason,
                &mut on_chunk,
            )
            .unwrap();
        }
        assert!(text.is_empty());
        assert!(chunks.is_empty());
        let calls = collect_tool_calls_from_blocks(blocks);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].id.as_deref(), Some("toolu_x"));
        assert_eq!(calls[0].arguments_json, r#"{"path":"src/lib.rs"}"#);
    }
}
