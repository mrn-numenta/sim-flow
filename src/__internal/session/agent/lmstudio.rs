//! `LmStudioAgent` - LM Studio's local OpenAI-compatible endpoint.

use super::CliAgent;
use super::openai_compatible::{OpenAiCompatibleRequest, dispatch_chat};
use crate::Result;
use crate::session::protocol::LlmMessage;

pub const DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";
/// LM Studio doesn't have a meaningful default - the user picks the
/// loaded model in the UI. We pass `local-model` as a placeholder
/// when nothing is configured; users normally set `--llm-model
/// <name>` to whatever is loaded.
pub const DEFAULT_MODEL: &str = "local-model";

pub struct LmStudioAgent {
    base_url: String,
    model: String,
}

impl LmStudioAgent {
    pub fn new(base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }
}

impl CliAgent for LmStudioAgent {
    fn name(&self) -> &str {
        "lmstudio"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<String> {
        dispatch_chat(OpenAiCompatibleRequest::new(
            &self.base_url,
            &self.model,
            messages,
        ))
    }
}
