//! `OllamaAgent` - Ollama's local OpenAI-compatible endpoint.

use super::openai_compatible::{OpenAiCompatibleRequest, dispatch_chat};
use super::{
    AgentAdaptationSummary, CliAgent, LlmCallMetrics, OPENAI_COMPAT_GENERIC_RUNTIME,
    RuntimeCapabilityProfile, resolve_model_family, resolve_runtime_profile,
};
use crate::Result;
use crate::session::protocol::LlmMessage;

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
pub const DEFAULT_MODEL: &str = "llama3.1";

pub struct OllamaAgent {
    base_url: String,
    model: String,
    model_family_id: Option<String>,
    runtime_profile: RuntimeCapabilityProfile,
}

impl OllamaAgent {
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

impl CliAgent for OllamaAgent {
    fn name(&self) -> &str {
        "ollama"
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
