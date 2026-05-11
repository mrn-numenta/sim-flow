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

use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{
    AgentAdaptationSummary, CliAgent, LlmCallMetrics, apply_reasoning_history_policy,
    resolve_model_family,
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
        let api_key = resolve_api_key(Provider::Anthropic)
            .ok()
            .flatten()
            .map(|r| r.key);
        Self {
            api_url: api_url.unwrap_or_else(|| DEFAULT_API_URL.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            model_family_id,
            api_key,
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
}

impl CliAgent for AnthropicAgent {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
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
        };

        let response_text = ureq::post(&self.api_url)
            .set("content-type", "application/json")
            .set("x-api-key", api_key)
            .set("anthropic-version", DEFAULT_API_VERSION)
            .send_json(&body);

        let resp = match response_text {
            Ok(r) => r,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(Error::Client(format!(
                    "anthropic api returned HTTP {code}: {body}"
                )));
            }
            Err(err) => {
                return Err(Error::Client(format!("anthropic api transport: {err}")));
            }
        };
        let body: MessagesResponseBody = resp
            .into_json()
            .map_err(|err| Error::Client(format!("anthropic api: decode response: {err}")))?;
        let text = collect_text(&body.content);
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
        };
        Ok((text, metrics))
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
            supports_structured_tool_calls: false,
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
    for msg in messages {
        match msg.role {
            LlmRole::System => system_blocks.push(msg.content.as_str()),
            LlmRole::User => conversation.push(MessagePayload {
                role: "user",
                content: msg.content.clone(),
            }),
            LlmRole::Assistant => conversation.push(MessagePayload {
                role: "assistant",
                content: msg.content.clone(),
            }),
        }
    }
    (system_blocks.join("\n\n"), conversation)
}

fn collect_text(blocks: &[ContentBlock]) -> String {
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

#[derive(Debug, Serialize)]
struct MessagesRequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<MessagePayload>,
}

#[derive(Debug, Serialize)]
struct MessagePayload {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct MessagesResponseBody {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
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
        assert_eq!(convo[0].content, "hello");
        assert_eq!(convo[1].role, "assistant");
        assert_eq!(convo[2].role, "user");
        assert_eq!(convo[2].content, "again");
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
            ContentBlock {
                kind: Some("text".into()),
                text: Some("alpha".into()),
            },
            ContentBlock {
                kind: Some("tool_use".into()),
                text: None,
            },
            ContentBlock {
                kind: Some("text".into()),
                text: Some(" beta".into()),
            },
        ];
        assert_eq!(collect_text(&blocks), "alpha beta");
    }

    #[test]
    fn request_body_omits_system_when_none() {
        let body = MessagesRequestBody {
            model: "claude-opus-4-7",
            max_tokens: 8192,
            system: None,
            messages: vec![MessagePayload {
                role: "user",
                content: "hi".into(),
            }],
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("\"system\""), "json: {json}");
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
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"system\":\"rules\""));
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
}
