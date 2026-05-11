//! `OpenAiCompatAgent` — generic OpenAI chat-completions client.
//!
//! Talks to anything that implements the OpenAI `/v1/chat/completions`
//! protocol: LM Studio, vLLM, llama.cpp server, TGI, Mistral
//! Inference API, etc. The caller picks the endpoint via `base_url`;
//! there is no single canonical default since every server uses a
//! different port (LM Studio 1234, vLLM 8000, llama.cpp 8080,
//! Ollama 11434, ...). For Ollama specifically prefer
//! [`crate::session::agent::OllamaAgent`] which handles the
//! `/api/...` paths Ollama sometimes uses outside the OpenAI-compat
//! surface.

pub mod tool_calls;
pub mod transport;

// `dispatch_chat` is the back-compat wrapper kept for the legacy
// fenced-block path. `dispatch_chat_with_tools` carries the richer
// shape with tool_calls; OpenAiCompatAgent::dispatch_with_tools
// routes through it when the orchestrator asks.
pub use transport::{OpenAiCompatibleRequest, dispatch_chat};

use self::tool_calls::ToolDescriptor;
use self::transport::dispatch_chat_with_tools;
use super::{
    AdvertisedToolCall, AgentAdaptationSummary, CliAgent, LlmCallMetrics,
    OPENAI_COMPAT_GENERIC_RUNTIME, RuntimeCapabilityProfile, ToolAdvertise, resolve_model_family,
    resolve_runtime_profile,
};
use crate::Result;
use crate::session::protocol::LlmMessage;

/// Fallback URL used when no `base_url` is provided. Matches LM
/// Studio's default port — the most common local-OpenAI-compat
/// server in the wild — but vLLM / llama.cpp / TGI users override
/// via `--base-url`.
pub const DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";

/// Generic placeholder when no model is configured. Real backends
/// will reject this and surface a clear error; users normally pass
/// `--llm-model <name>` or set the matching config field.
pub const DEFAULT_MODEL: &str = "local-model";

pub struct OpenAiCompatAgent {
    base_url: String,
    model: String,
    model_family_id: Option<String>,
    runtime_profile: RuntimeCapabilityProfile,
}

impl OpenAiCompatAgent {
    pub fn new(
        base_url: Option<String>,
        model: Option<String>,
        model_family_id: Option<String>,
        runtime_profile_id: Option<String>,
    ) -> Self {
        let runtime_profile = resolve_runtime_profile(
            runtime_profile_id.as_deref(),
            OPENAI_COMPAT_GENERIC_RUNTIME,
            &["openai_compat_generic"],
        )
        .unwrap_or(OPENAI_COMPAT_GENERIC_RUNTIME);
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
            model_family_id,
            runtime_profile,
        }
    }

    #[cfg(test)]
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    #[cfg(test)]
    pub(crate) fn model_family_id(&self) -> Option<&str> {
        self.model_family_id.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn runtime_profile(&self) -> RuntimeCapabilityProfile {
        self.runtime_profile
    }
}

impl CliAgent for OpenAiCompatAgent {
    fn name(&self) -> &str {
        "openai-compat"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        debug_assert_eq!(
            self.runtime_profile.request_format,
            "openai_chat_completions"
        );
        dispatch_chat(
            OpenAiCompatibleRequest::new(&self.base_url, &self.model, messages)
                .with_model_family_id(self.model_family_id.as_deref()),
        )
    }

    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        if tools.is_empty() {
            let (text, metrics) = self.dispatch(messages)?;
            return Ok((text, Vec::new(), metrics));
        }
        let wire_tools: Vec<ToolDescriptor> = tools
            .iter()
            .map(|t| {
                ToolDescriptor::function(
                    t.name.clone(),
                    t.description.clone(),
                    t.parameters.clone(),
                )
            })
            .collect();
        let req = OpenAiCompatibleRequest::new(&self.base_url, &self.model, messages)
            .with_model_family_id(self.model_family_id.as_deref())
            .with_tools(wire_tools, Some("auto"));
        let resp = dispatch_chat_with_tools(req)?;
        let calls = resp
            .tool_calls
            .into_iter()
            .map(|c| AdvertisedToolCall {
                id: c.id,
                name: c.function.name,
                arguments_json: c.function.arguments,
            })
            .collect();
        Ok((resp.text, calls, resp.metrics))
    }

    fn adaptation_summary(&self) -> Option<AgentAdaptationSummary> {
        let family = resolve_model_family(self.model_family_id.as_deref(), Some(&self.model));
        Some(AgentAdaptationSummary {
            backend: self.name().to_string(),
            runtime_profile_id: self.runtime_profile.id.as_str().to_string(),
            model_family_id: family.id.to_string(),
            request_format: self.runtime_profile.request_format.to_string(),
            system_prompt_mode: self.runtime_profile.system_prompt_mode.to_string(),
            credential_policy: self.runtime_profile.credential_policy.to_string(),
            supports_structured_reasoning: self.runtime_profile.supports_structured_reasoning,
            supports_structured_tool_calls: self.runtime_profile.supports_structured_tool_calls,
            supports_thinking_controls: family.supports_thinking_controls,
        })
    }
}
