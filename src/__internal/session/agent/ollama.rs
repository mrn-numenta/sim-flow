//! `OllamaAgent` - Ollama's local OpenAI-compatible endpoint.

use super::openai_compat::transport::dispatch_chat_with_tools_streaming;
use super::openai_compat::{OpenAiCompatibleRequest, dispatch_chat};
use super::{
    AdvertisedToolCall, AgentAdaptationSummary, CliAgent, LlmCallMetrics,
    OPENAI_COMPAT_GENERIC_RUNTIME, RuntimeCapabilityProfile, StreamingChunk, ToolAdvertise,
    resolve_model_family, resolve_runtime_profile,
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
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl OllamaAgent {
    pub fn new(
        base_url: Option<String>,
        model: Option<String>,
        model_family_id: Option<String>,
        runtime_profile_id: Option<String>,
    ) -> Self {
        Self::new_with_cancel(base_url, model, model_family_id, runtime_profile_id, None)
    }

    pub fn new_with_cancel(
        base_url: Option<String>,
        model: Option<String>,
        model_family_id: Option<String>,
        runtime_profile_id: Option<String>,
        cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
            cancel_flag,
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
            self.cancel_flag.clone(),
        )
    }

    fn dispatch_streaming(
        &self,
        messages: &[LlmMessage],
        _tools: &[ToolAdvertise],
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        // Ollama's OpenAI-compat shim doesn't reliably implement
        // native tool-call streaming; we route tool catalogs through
        // the fenced-block fallback on Ollama anyway, so drop the
        // tools here and stream text-only. Switching to native-tool
        // streaming on Ollama is a follow-up tied to its server-side
        // tool support stabilizing.
        let req = OpenAiCompatibleRequest::new(&self.base_url, &self.model, messages)
            .with_model_family_id(self.model_family_id.as_deref());
        let resp = dispatch_chat_with_tools_streaming(req, self.cancel_flag.clone(), on_chunk)?;
        Ok((resp.text, Vec::new(), resp.metrics))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_default_base_url_and_model() {
        let agent = OllamaAgent::new(None, None, None, None);
        assert_eq!(agent.base_url(), DEFAULT_BASE_URL);
        assert_eq!(agent.model(), DEFAULT_MODEL);
        assert!(agent.model_family_id().is_none());
    }

    #[test]
    fn new_respects_overrides() {
        let agent = OllamaAgent::new(
            Some("http://host:8080/v1".into()),
            Some("qwen3.6:14b".into()),
            Some("qwen3".into()),
            None,
        );
        assert_eq!(agent.base_url(), "http://host:8080/v1");
        assert_eq!(agent.model(), "qwen3.6:14b");
        assert_eq!(agent.model_family_id(), Some("qwen3"));
    }

    #[test]
    fn name_returns_ollama() {
        let agent = OllamaAgent::new(None, None, None, None);
        assert_eq!(agent.name(), "ollama");
    }

    #[test]
    fn runtime_profile_defaults_to_generic() {
        let agent = OllamaAgent::new(None, None, None, None);
        let profile = agent.runtime_profile();
        assert_eq!(
            profile.id.as_str(),
            OPENAI_COMPAT_GENERIC_RUNTIME.id.as_str()
        );
    }

    #[test]
    fn adaptation_summary_reports_ollama_as_backend() {
        let agent = OllamaAgent::new(None, None, None, None);
        let summary = agent.adaptation_summary().expect("summary");
        assert_eq!(summary.backend, "ollama");
        assert_eq!(summary.request_format, "openai_chat_completions");
    }

    #[test]
    fn dispatch_errors_against_unreachable_host() {
        // Local URL with no listener: dispatch should return an
        // error rather than hanging. Exercises the routing into
        // dispatch_chat without depending on a real server.
        let agent = OllamaAgent::new(Some("http://127.0.0.1:0/v1".into()), None, None, None);
        let result = agent.dispatch(&[]);
        assert!(result.is_err());
    }
}
