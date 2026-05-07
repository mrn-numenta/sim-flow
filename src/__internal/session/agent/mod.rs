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
//! - [`OllamaAgent`] / [`OpenAiCompatAgent`] - HTTP wrappers around
//!   each tool's OpenAI-compatible chat-completions endpoint.
//!   `OpenAiCompatAgent` is the generic flavor; pick it for any
//!   server speaking `/v1/chat/completions` (LM Studio, vLLM,
//!   llama.cpp server, TGI, ...).
//! - [`MockAgent`] - canned-response queue used by unit tests.

mod claude;
mod codex;
mod gh_copilot;
pub mod interactive_pty;
mod mock;
mod ollama;
mod openai_compat;
mod openai_compatible;

pub use claude::ClaudeAgent;
pub(crate) use claude::normalize_model_for_cli;
pub use codex::CodexAgent;
pub use gh_copilot::GhCopilotAgent;
pub use interactive_pty::{
    ExitInfo, InteractivePtySession, ProxyHandle, PtyWriter, finish_proxy, proxy_until_exit,
    start_pty_proxy,
};
pub use mock::MockAgent;
pub use ollama::OllamaAgent;
pub use openai_compat::OpenAiCompatAgent;

use crate::Result;
use crate::session::protocol::LlmMessage;

/// Per-call metrics captured at agent dispatch time. Populated as
/// fully as the backend allows: HTTP backends (OpenAI-compat /
/// vLLM / LM Studio / llama.cpp, Ollama, OpenAI, Anthropic) report
/// `prompt_tokens` / `completion_tokens`
/// in their response body and we measure the round-trip locally.
/// Subprocess and PTY backends (claude, codex, gh-copilot) leave
/// the token fields `None` because the CLI doesn't surface them;
/// `wall_ms` is always populated.
///
/// Consumed by the orchestrator + TerminalHost to emit per-call
/// `tracing::info!` events under target `sim_flow::metrics` and to
/// aggregate per-sub-session totals (token spend, time spent in
/// LLM calls, calls/turn) for cost / progress reporting.
#[derive(Debug, Clone, Default)]
pub struct LlmCallMetrics {
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub wall_ms: u64,
}

pub trait CliAgent: Send {
    fn name(&self) -> &str;
    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)>;
}

/// Optional configuration for the HTTP-based agents. Subprocess
/// agents (claude, codex, gh-copilot) ignore everything except
/// `model`.
#[derive(Debug, Clone, Default)]
pub struct AgentConfig {
    pub model: Option<String>,
    /// Generic base-URL override. Wins over the per-backend
    /// `ollama_base_url` / `openai_base_url` fields when set.
    /// Use this for `vllm` / generic openai-compat servers and
    /// for any user-defined endpoint that doesn't fit the
    /// conventional default. None means "use the backend's
    /// conventional default".
    pub base_url: Option<String>,
    pub ollama_base_url: Option<String>,
    pub openai_base_url: Option<String>,
}

/// Default OpenAI-compatible base URL for a vLLM server. vLLM
/// listens on port 8000 by default; the `/v1` path is the
/// OpenAI-compat shim it serves.
pub const VLLM_DEFAULT_BASE_URL: &str = "http://localhost:8000/v1";

/// Build a `CliAgent` from a backend name. Returns `None` when the
/// name doesn't match a known agent so callers can surface a helpful
/// error listing available choices.
pub fn build_cli_agent(name: &str, config: AgentConfig) -> Option<Box<dyn CliAgent>> {
    // The generic `base_url` field wins over the legacy per-
    // backend fields. Callers using `--llm-base-url` set
    // `config.base_url`; older callers using
    // `--ollama-base-url` etc. still work via the fallbacks.
    let pick = |fallback: Option<String>| config.base_url.clone().or(fallback);
    match name {
        "claude" | "claude-cli" => Some(Box::new(ClaudeAgent::new(config.model))),
        "codex" | "codex-cli" => Some(Box::new(CodexAgent::new(config.model))),
        "gh-copilot" | "gh_copilot" => Some(Box::new(GhCopilotAgent::new())),
        "ollama" => Some(Box::new(OllamaAgent::new(
            pick(config.ollama_base_url),
            config.model,
        ))),
        // LM Studio uses the OpenAI-compat agent with its
        // conventional `:1234/v1` default. Aliasing it explicitly
        // (rather than asking users to type `openai-compat`) makes
        // the dashboard's Source dropdown read naturally.
        "lmstudio" | "lm-studio" => Some(Box::new(OpenAiCompatAgent::new(
            pick(config.openai_base_url),
            config.model,
        ))),
        // vLLM is also OpenAI-compat but defaults to `:8000/v1`.
        // The `base_url` override path (when set) wins; otherwise
        // we substitute the vLLM default before falling through
        // to whatever else the AgentConfig has.
        "vllm" => Some(Box::new(OpenAiCompatAgent::new(
            config
                .base_url
                .clone()
                .or(config.openai_base_url)
                .or_else(|| Some(VLLM_DEFAULT_BASE_URL.to_string())),
            config.model,
        ))),
        // Generic openai-compat backend: matches anyone running an
        // OpenAI-compat server somewhere. Defaults to LM Studio's
        // `:1234/v1` for back-compat with existing flows.
        "openai-compat" | "openai_compat" | "openai" => Some(Box::new(OpenAiCompatAgent::new(
            pick(config.openai_base_url),
            config.model,
        ))),
        _ => None,
    }
}

/// Names accepted by `build_cli_agent`. Stable for help text /
/// error messages.
pub const KNOWN_AGENTS: &[&str] = &[
    "claude",
    "codex",
    "gh-copilot",
    "ollama",
    "lmstudio",
    "vllm",
    "openai-compat",
];
