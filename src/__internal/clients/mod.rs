//! Concrete AI client implementations.
//!
//! The real-CLI clients (Claude, Codex, Copilot) are thin subprocess
//! wrappers. The mock client is used by tests and by the smoke integration
//! test to exercise the orchestrator without spending LLM turns.

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod mock;

use std::sync::Arc;

use crate::client::Client;
use crate::config::{ClientName, Config};

/// Build a client instance for the given config selection.
///
/// The mock client is returned when `ClientName::Mock` is selected and
/// picks up canned responses from the `SIM_FLOW_MOCK_RESPONSES_DIR`
/// environment variable (see [`mock::MockClient`] for details).
pub fn build(config: &Config, name: ClientName) -> Arc<dyn Client> {
    match name {
        ClientName::Claude => Arc::new(claude::ClaudeClient::new(config.claude.clone())),
        ClientName::Codex => Arc::new(codex::CodexClient::new(config.codex.clone())),
        ClientName::Copilot => Arc::new(copilot::CopilotClient::new(config.copilot.clone())),
        ClientName::Mock => Arc::new(mock::MockClient::from_env()),
    }
}
