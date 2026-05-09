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

use super::openai_compatible::{OpenAiCompatibleRequest, dispatch_chat};
use super::{CliAgent, LlmCallMetrics, OPENAI_COMPAT_GENERIC_RUNTIME, RuntimeCapabilityProfile};
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
    ) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
            model_family_id,
            runtime_profile: OPENAI_COMPAT_GENERIC_RUNTIME,
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
}
