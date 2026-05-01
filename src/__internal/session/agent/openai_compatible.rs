//! Shared HTTP client for OpenAI-compatible chat-completion endpoints.
//!
//! Ollama and LM Studio expose this same wire format on their local
//! servers; the existing TS extension uses an `OpenAiCompatibleBackend`
//! base class for the same reason. This Rust module mirrors that
//! pattern so the per-server agent impls stay tiny.

use serde::{Deserialize, Serialize};

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
}

impl<'a> OpenAiCompatibleRequest<'a> {
    pub fn new(base_url: &'a str, model: &'a str, messages: &'a [LlmMessage]) -> Self {
        Self {
            base_url,
            model,
            messages,
            api_key: None,
            max_response_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    messages: Vec<RequestMessage<'a>>,
    stream: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatResponseBody {
    choices: Vec<Choice>,
}

#[derive(Debug, Clone, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
}

/// Send a synchronous chat-completions request and return the
/// assistant's full text. The endpoint defaults to
/// `<base_url>/chat/completions`.
pub fn dispatch_chat(req: OpenAiCompatibleRequest<'_>) -> Result<String> {
    let body = ChatRequestBody {
        model: req.model,
        messages: req
            .messages
            .iter()
            .map(|m| RequestMessage {
                role: role_str(m.role),
                content: &m.content,
            })
            .collect(),
        stream: false,
    };
    let url = format!("{}/chat/completions", trim_trailing_slash(req.base_url));
    let mut request = ureq::post(&url)
        .set("content-type", "application/json")
        .set("accept", "application/json");
    if let Some(key) = req.api_key {
        request = request.set("authorization", &format!("Bearer {key}"));
    }
    let response = request
        .send_json(
            serde_json::to_value(&body)
                .map_err(|e| Error::Client(format!("openai-compat: serialize request: {e}")))?,
        )
        .map_err(|e| Error::Client(format!("openai-compat: HTTP error: {e}")))?;
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
    let text = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(text)
}

fn role_str(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
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
}
