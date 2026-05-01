//! `OllamaAgent` - Ollama's local OpenAI-compatible endpoint.

use super::CliAgent;
use super::openai_compatible::{OpenAiCompatibleRequest, dispatch_chat};
use crate::Result;
use crate::session::protocol::LlmMessage;

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
pub const DEFAULT_MODEL: &str = "llama3.1";

pub struct OllamaAgent {
    base_url: String,
    model: String,
}

impl OllamaAgent {
    pub fn new(base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }
}

impl CliAgent for OllamaAgent {
    fn name(&self) -> &str {
        "ollama"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<String> {
        dispatch_chat(OpenAiCompatibleRequest::new(
            &self.base_url,
            &self.model,
            messages,
        ))
    }
}
