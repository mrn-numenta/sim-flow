//! Shared HTTP client for OpenAI-compatible chat-completion endpoints.
//!
//! Ollama and LM Studio expose this same wire format on their local
//! servers; the existing TS extension uses an `OpenAiCompatibleBackend`
//! base class for the same reason. This Rust module mirrors that
//! pattern so the per-server agent impls stay tiny.

use serde::{Deserialize, Serialize};

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
    /// `SIM_FLOW_MAX_TOKENS` env var, falling back to 32768. 16384
    /// was the prior default but qwen3.6's milestone-level work
    /// sessions (multi-file scoreboard / testbench writes) blow
    /// past it on the thinking-plus-tool-calls path, hitting
    /// `finish_reason=length` and leaving the milestone with zero
    /// files on disk. 32768 still sits well under any local model's
    /// context window so a runaway is killed by the server, not by
    /// us hitting the context wall.
    pub max_tokens: u32,
}

impl<'a> OpenAiCompatibleRequest<'a> {
    pub fn new(base_url: &'a str, model: &'a str, messages: &'a [LlmMessage]) -> Self {
        let max_tokens = std::env::var("SIM_FLOW_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(32_768);
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
            messages,
            api_key: None,
            max_response_bytes,
            max_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    messages: Vec<RequestMessage<'a>>,
    stream: bool,
    max_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
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
}

/// Send a synchronous chat-completions request and return the
/// assistant's full text plus per-call metrics (prompt /
/// completion tokens from the response `usage` object, total
/// wall-clock round-trip time). The endpoint defaults to
/// `<base_url>/chat/completions`.
pub fn dispatch_chat(req: OpenAiCompatibleRequest<'_>) -> Result<(String, LlmCallMetrics)> {
    let started = std::time::Instant::now();
    // Merge consecutive leading system messages into one. The
    // orchestrator's `build_initial_messages` stacks 4-5 system
    // messages at the front (convention + instructions, tool catalog,
    // session inputs, spec TOC, framework TOC) which OpenAI / LM
    // Studio happily accept verbatim. vLLM enforces qwen / llama
    // chat templates that require EXACTLY ONE system message at the
    // start ("System message must be at the beginning."), so we
    // collapse the stack here. Concatenation order matches the
    // orchestrator's emit order; a `\n\n---\n\n` separator preserves
    // the visual block boundaries the model would otherwise lose.
    let merged_messages = merge_leading_system_messages(req.messages);
    let body = ChatRequestBody {
        model: req.model,
        messages: merged_messages
            .iter()
            .map(|m| RequestMessage {
                role: m.role,
                content: m.content.as_ref(),
            })
            .collect(),
        stream: false,
        max_tokens: req.max_tokens,
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
    let text = decode_choice(parsed.choices.into_iter().next())?;
    Ok((text, metrics))
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
fn decode_choice(choice: Option<Choice>) -> Result<String> {
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
        Ok(content)
    } else if !reasoning.is_empty() {
        Ok(reasoning)
    } else {
        Ok(String::new())
    }
}

fn role_str(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
    }
}

/// Owned message wrapper used by `merge_leading_system_messages` so
/// the merged-system entry (a freshly-allocated `String`) can sit
/// alongside borrowed references to original message contents.
struct MergedMessage<'a> {
    role: &'static str,
    content: std::borrow::Cow<'a, str>,
}

/// Collapse consecutive leading system messages into one. Required
/// for backends whose chat template rejects multi-system stacks
/// (vLLM / qwen, certain llama templates). Trailing system messages
/// inside the conversation are left alone — the orchestrator doesn't
/// emit those today; if a future feature does, the backend will
/// reject the request and we'll see the body in the LlmError
/// diagnostic so we can add a per-backend adapter then.
fn merge_leading_system_messages(messages: &[LlmMessage]) -> Vec<MergedMessage<'_>> {
    let leading_system_count = messages
        .iter()
        .take_while(|m| matches!(m.role, LlmRole::System))
        .count();
    let mut out: Vec<MergedMessage<'_>> = Vec::with_capacity(messages.len());
    if leading_system_count >= 2 {
        let mut merged = String::new();
        for (i, m) in messages.iter().take(leading_system_count).enumerate() {
            if i > 0 {
                merged.push_str("\n\n---\n\n");
            }
            merged.push_str(&m.content);
        }
        out.push(MergedMessage {
            role: "system",
            content: std::borrow::Cow::Owned(merged),
        });
    } else if leading_system_count == 1 {
        out.push(MergedMessage {
            role: "system",
            content: std::borrow::Cow::Borrowed(&messages[0].content),
        });
    }
    for m in messages.iter().skip(leading_system_count) {
        out.push(MergedMessage {
            role: role_str(m.role),
            content: std::borrow::Cow::Borrowed(&m.content),
        });
    }
    out
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
        let merged = merge_leading_system_messages(&messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, "system");
        assert!(merged[0].content.contains("first"));
        assert!(merged[0].content.contains("second"));
        assert!(merged[0].content.contains("third"));
        assert!(merged[0].content.contains("---"));
        assert_eq!(merged[1].role, "user");
        assert_eq!(merged[1].content.as_ref(), "hi");
    }

    #[test]
    fn merge_leading_system_passes_through_single_system() {
        let messages = vec![msg(LlmRole::System, "only"), msg(LlmRole::User, "hi")];
        let merged = merge_leading_system_messages(&messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].content.as_ref(), "only");
    }

    #[test]
    fn merge_leading_system_handles_no_system() {
        let messages = vec![msg(LlmRole::User, "hi")];
        let merged = merge_leading_system_messages(&messages);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].role, "user");
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
        let merged = merge_leading_system_messages(&messages);
        assert_eq!(merged.len(), 5); // 2 leading collapsed -> 1, plus 4 originals
        assert_eq!(merged[0].role, "system");
        assert!(merged[0].content.contains("leading-1"));
        assert!(merged[0].content.contains("leading-2"));
        assert_eq!(merged[1].role, "user");
        assert_eq!(merged[2].role, "assistant");
        assert_eq!(merged[3].role, "system"); // mid-stream system left in place
        assert_eq!(merged[3].content.as_ref(), "mid-stream");
        assert_eq!(merged[4].role, "user");
    }

    fn choice(content: Option<&str>, reasoning: Option<&str>, finish: Option<&str>) -> Choice {
        Choice {
            message: ResponseMessage {
                content: content.map(String::from),
                reasoning: reasoning.map(String::from),
            },
            finish_reason: finish.map(String::from),
        }
    }

    #[test]
    fn decode_choice_returns_content_on_normal_stop() {
        let c = choice(Some("hello"), None, Some("stop"));
        assert_eq!(decode_choice(Some(c)).unwrap(), "hello");
    }

    #[test]
    fn decode_choice_falls_back_to_reasoning_when_content_empty() {
        let c = choice(None, Some("thinking text"), Some("stop"));
        assert_eq!(decode_choice(Some(c)).unwrap(), "thinking text");
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
        let err = decode_choice(Some(c)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("truncated at max_tokens"), "got: {msg}");
        assert!(msg.contains("cargo tarpaulin"), "tail should appear: {msg}");
    }

    #[test]
    fn decode_choice_errors_on_finish_length_with_only_reasoning() {
        let c = choice(None, Some("step 1: ..."), Some("length"));
        let err = decode_choice(Some(c)).unwrap_err();
        assert!(format!("{err}").contains("truncated at max_tokens"));
    }

    #[test]
    fn decode_choice_returns_empty_when_no_choice() {
        assert_eq!(decode_choice(None).unwrap(), "");
    }
}
