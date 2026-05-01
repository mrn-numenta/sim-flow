//! LLM dispatchers used by the in-process `TerminalHost`.
//!
//! `CliAgent` is the abstraction the orchestrator uses when running
//! standalone (`sim-flow session ...` from a plain terminal). The
//! external-host path (`--jsonl`) doesn't use these - hosts dispatch
//! `RequestLlmResponse` events to whatever LLM client they own.
//!
//! Implementations live in submodules:
//!
//! - [`ClaudeAgent`] - subprocess wrapper for the `claude` CLI
//!   (Claude Code subscription).
//! - [`CodexAgent`] - subprocess wrapper for OpenAI's `codex` CLI.
//!   Best-effort; surfaces errors clearly when the CLI flag set
//!   doesn't match.
//! - [`GhCopilotAgent`] - subprocess wrapper for `gh copilot`.
//!   Experimental; prefer the `vscode` LLM source for Copilot Chat.
//! - [`OllamaAgent`] / [`LmStudioAgent`] - HTTP wrappers around
//!   each tool's OpenAI-compatible chat-completions endpoint.
//! - [`MockAgent`] - canned-response queue used by unit tests.

mod claude;
mod codex;
mod gh_copilot;
pub mod interactive_pty;
mod lmstudio;
mod mock;
mod ollama;
mod openai_compatible;

pub use claude::ClaudeAgent;
pub(crate) use claude::normalize_model_for_cli;
pub use codex::CodexAgent;
pub use gh_copilot::GhCopilotAgent;
pub use interactive_pty::{
    ExitInfo, InteractivePtySession, ProxyHandle, PtyWriter, finish_proxy, proxy_until_exit,
    start_pty_proxy,
};
pub use lmstudio::LmStudioAgent;
pub use mock::MockAgent;
pub use ollama::OllamaAgent;

use crate::Result;
use crate::session::protocol::LlmMessage;

pub trait CliAgent: Send {
    fn name(&self) -> &str;
    fn dispatch(&self, messages: &[LlmMessage]) -> Result<String>;
}

/// Optional configuration for the HTTP-based agents. Subprocess
/// agents (claude, codex, gh-copilot) ignore everything except
/// `model`.
#[derive(Debug, Clone, Default)]
pub struct AgentConfig {
    pub model: Option<String>,
    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
}

/// Build a `CliAgent` from a backend name. Returns `None` when the
/// name doesn't match a known agent so callers can surface a helpful
/// error listing available choices.
pub fn build_cli_agent(name: &str, config: AgentConfig) -> Option<Box<dyn CliAgent>> {
    match name {
        "claude" | "claude-cli" => Some(Box::new(ClaudeAgent::new(config.model))),
        "codex" | "codex-cli" => Some(Box::new(CodexAgent::new(config.model))),
        "gh-copilot" | "gh_copilot" => Some(Box::new(GhCopilotAgent::new())),
        "ollama" => Some(Box::new(OllamaAgent::new(
            config.ollama_base_url,
            config.model,
        ))),
        "lmstudio" | "lm-studio" => Some(Box::new(LmStudioAgent::new(
            config.lmstudio_base_url,
            config.model,
        ))),
        _ => None,
    }
}

/// Names accepted by `build_cli_agent`. Stable for help text /
/// error messages.
pub const KNOWN_AGENTS: &[&str] = &["claude", "codex", "gh-copilot", "ollama", "lmstudio"];
