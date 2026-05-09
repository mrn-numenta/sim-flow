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

mod adaptation;
mod claude;
mod codex;
mod gh_copilot;
pub mod interactive_pty;
mod mock;
mod ollama;
mod openai_compat;
mod openai_compatible;

pub(crate) use adaptation::{
    CLAUDE_CLI_RUNTIME, OPENAI_COMPAT_GENERIC_RUNTIME, RuntimeCapabilityProfile,
    apply_reasoning_history_policy, normalize_response_text, prepare_messages_for_openai_compat,
    resolve_model_family,
};
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
    /// Optional explicit model-family override. When unset the Rust
    /// agent path infers a family from the configured model id.
    pub model_family_id: Option<String>,
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

/// Resolve the `base_url` an agent named `name` would be constructed
/// with, given the supplied `config`. Returns `None` for backend
/// names that don't take a base URL (claude / codex / gh-copilot)
/// or for an unknown backend name.
///
/// Precedence (when applicable):
///   1. `config.base_url` (the generic `--llm-base-url` flag)
///   2. The matching legacy per-backend URL
///      (`config.ollama_base_url` for ollama, `config.openai_base_url`
///      for the openai-compat family)
///   3. Backend-specific default (only `vllm` substitutes here;
///      ollama / lmstudio / openai-compat fall through to `None`,
///      letting the agent constructor pick its own conventional
///      default).
///
/// Lifted out of `build_cli_agent` so tests can verify precedence
/// without having to reach inside the boxed trait object.
pub(crate) fn resolved_base_url(name: &str, config: &AgentConfig) -> Option<String> {
    let pick = |fallback: Option<String>| config.base_url.clone().or(fallback);
    match name {
        "ollama" => pick(config.ollama_base_url.clone()),
        "lmstudio" | "lm-studio" | "openai-compat" | "openai_compat" | "openai" => {
            pick(config.openai_base_url.clone())
        }
        "vllm" => {
            pick(config.openai_base_url.clone()).or_else(|| Some(VLLM_DEFAULT_BASE_URL.to_string()))
        }
        _ => None,
    }
}

/// Build a `CliAgent` from a backend name. Returns `None` when the
/// name doesn't match a known agent so callers can surface a helpful
/// error listing available choices.
pub fn build_cli_agent(name: &str, config: AgentConfig) -> Option<Box<dyn CliAgent>> {
    let resolved = resolved_base_url(name, &config);
    match name {
        "claude" | "claude-cli" => Some(Box::new(ClaudeAgent::new(
            config.model,
            config.model_family_id,
        ))),
        "codex" | "codex-cli" => Some(Box::new(CodexAgent::new(config.model))),
        "gh-copilot" | "gh_copilot" => Some(Box::new(GhCopilotAgent::new())),
        "ollama" => Some(Box::new(OllamaAgent::new(
            resolved,
            config.model,
            config.model_family_id,
        ))),
        // LM Studio uses the OpenAI-compat agent with its
        // conventional `:1234/v1` default. Aliasing it explicitly
        // (rather than asking users to type `openai-compat`) makes
        // the dashboard's Source dropdown read naturally.
        // vLLM uses the same OpenAI-compat path with a `:8000/v1`
        // default that `resolved_base_url` substitutes.
        "lmstudio" | "lm-studio" | "vllm" | "openai-compat" | "openai_compat" | "openai" => {
            Some(Box::new(OpenAiCompatAgent::new(
                resolved,
                config.model,
                config.model_family_id,
            )))
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        base: Option<&str>,
        ollama: Option<&str>,
        openai: Option<&str>,
        model: Option<&str>,
    ) -> AgentConfig {
        AgentConfig {
            model: model.map(String::from),
            model_family_id: None,
            base_url: base.map(String::from),
            ollama_base_url: ollama.map(String::from),
            openai_base_url: openai.map(String::from),
        }
    }

    // ---------- resolved_base_url ----------

    #[test]
    fn resolved_base_url_unknown_backend_returns_none() {
        assert_eq!(
            resolved_base_url("nope", &cfg(None, None, None, None)),
            None
        );
    }

    #[test]
    fn resolved_base_url_subprocess_backends_return_none() {
        for name in [
            "claude",
            "claude-cli",
            "codex",
            "codex-cli",
            "gh-copilot",
            "gh_copilot",
        ] {
            assert_eq!(
                resolved_base_url(name, &cfg(Some("http://x"), None, None, None)),
                None,
                "{name} should not resolve a base url",
            );
        }
    }

    #[test]
    fn resolved_base_url_ollama_uses_ollama_field_when_no_generic() {
        let r = resolved_base_url("ollama", &cfg(None, Some("http://o:1/v1"), None, None));
        assert_eq!(r.as_deref(), Some("http://o:1/v1"));
    }

    #[test]
    fn resolved_base_url_ollama_falls_through_to_none_without_overrides() {
        // `OllamaAgent::new` is what substitutes the conventional default.
        let r = resolved_base_url("ollama", &cfg(None, None, None, None));
        assert_eq!(r, None);
    }

    #[test]
    fn resolved_base_url_generic_wins_over_ollama_field() {
        let r = resolved_base_url(
            "ollama",
            &cfg(Some("http://generic"), Some("http://legacy"), None, None),
        );
        assert_eq!(r.as_deref(), Some("http://generic"));
    }

    #[test]
    fn resolved_base_url_lmstudio_uses_openai_field() {
        let r = resolved_base_url(
            "lmstudio",
            &cfg(None, None, Some("http://lm:1234/v1"), None),
        );
        assert_eq!(r.as_deref(), Some("http://lm:1234/v1"));
        // Alias spelling produces the same answer.
        let r2 = resolved_base_url(
            "lm-studio",
            &cfg(None, None, Some("http://lm:1234/v1"), None),
        );
        assert_eq!(r2, r);
    }

    #[test]
    fn resolved_base_url_generic_wins_over_openai_field() {
        let r = resolved_base_url(
            "openai-compat",
            &cfg(Some("http://generic"), None, Some("http://legacy"), None),
        );
        assert_eq!(r.as_deref(), Some("http://generic"));
    }

    #[test]
    fn resolved_base_url_openai_compat_aliases_match() {
        let c = cfg(None, None, Some("http://x/v1"), None);
        assert_eq!(
            resolved_base_url("openai-compat", &c).as_deref(),
            Some("http://x/v1")
        );
        assert_eq!(
            resolved_base_url("openai_compat", &c).as_deref(),
            Some("http://x/v1")
        );
        assert_eq!(
            resolved_base_url("openai", &c).as_deref(),
            Some("http://x/v1")
        );
    }

    #[test]
    fn resolved_base_url_vllm_substitutes_default_when_nothing_provided() {
        let r = resolved_base_url("vllm", &cfg(None, None, None, None));
        assert_eq!(r.as_deref(), Some(VLLM_DEFAULT_BASE_URL));
    }

    #[test]
    fn resolved_base_url_vllm_openai_field_wins_over_default() {
        let r = resolved_base_url("vllm", &cfg(None, None, Some("http://my-vllm/v1"), None));
        assert_eq!(r.as_deref(), Some("http://my-vllm/v1"));
    }

    #[test]
    fn resolved_base_url_vllm_generic_wins_over_openai_field_and_default() {
        let r = resolved_base_url(
            "vllm",
            &cfg(Some("http://generic"), None, Some("http://legacy"), None),
        );
        assert_eq!(r.as_deref(), Some("http://generic"));
    }

    // ---------- build_cli_agent ----------

    #[test]
    fn build_cli_agent_unknown_returns_none() {
        assert!(build_cli_agent("does-not-exist", AgentConfig::default()).is_none());
    }

    #[test]
    fn build_cli_agent_known_names_round_trip() {
        for name in KNOWN_AGENTS {
            let agent = build_cli_agent(name, AgentConfig::default());
            assert!(agent.is_some(), "{name} should build a CliAgent");
        }
    }

    #[test]
    fn build_cli_agent_subprocess_backends_ignore_urls() {
        // The `ClaudeAgent` / `CodexAgent` / `GhCopilotAgent`
        // constructors don't take a base_url, so we just confirm
        // the trait `name()` matches the requested backend.
        let claude = build_cli_agent(
            "claude",
            cfg(
                Some("http://nope"),
                Some("http://nope"),
                Some("http://nope"),
                Some("opus"),
            ),
        )
        .expect("claude should build");
        assert_eq!(claude.name(), "claude");
        let codex = build_cli_agent("codex-cli", AgentConfig::default()).expect("codex");
        assert_eq!(codex.name(), "codex");
        let copilot = build_cli_agent("gh_copilot", AgentConfig::default()).expect("copilot");
        assert_eq!(copilot.name(), "gh-copilot");
    }

    // ---------- agent constructors ----------

    #[test]
    fn ollama_agent_substitutes_default_url_and_model() {
        let a = OllamaAgent::new(None, None, None);
        assert_eq!(a.base_url(), super::ollama::DEFAULT_BASE_URL);
        assert_eq!(a.model(), super::ollama::DEFAULT_MODEL);
        assert_eq!(
            a.runtime_profile().request_format,
            "openai_chat_completions"
        );
    }

    #[test]
    fn ollama_agent_respects_overrides() {
        let a = OllamaAgent::new(
            Some("http://x:9/v1".into()),
            Some("qwen3.6".into()),
            Some("qwen3_6".into()),
        );
        assert_eq!(a.base_url(), "http://x:9/v1");
        assert_eq!(a.model(), "qwen3.6");
        assert_eq!(a.model_family_id(), Some("qwen3_6"));
    }

    #[test]
    fn openai_compat_agent_substitutes_default_url() {
        let a = OpenAiCompatAgent::new(None, None, None);
        assert_eq!(a.base_url(), super::openai_compat::DEFAULT_BASE_URL);
        assert_eq!(a.model(), super::openai_compat::DEFAULT_MODEL);
        assert_eq!(
            a.runtime_profile().request_format,
            "openai_chat_completions"
        );
    }

    #[test]
    fn openai_compat_agent_respects_overrides() {
        let a = OpenAiCompatAgent::new(
            Some("http://lm-studio:1234/v1".into()),
            Some("custom".into()),
            None,
        );
        assert_eq!(a.base_url(), "http://lm-studio:1234/v1");
        assert_eq!(a.model(), "custom");
    }

    #[test]
    fn openai_compat_agent_preserves_explicit_model_family_override() {
        let a = OpenAiCompatAgent::new(
            None,
            Some("moonshotai/Kimi-VL-A3B-Thinking-2506".into()),
            Some("gemma4".into()),
        );
        assert_eq!(a.model_family_id(), Some("gemma4"));
    }

    #[test]
    fn claude_agent_uses_claude_cli_runtime_profile() {
        let a = ClaudeAgent::new(Some("claude-sonnet-4-6".into()), None);
        assert_eq!(a.runtime_profile().request_format, "subprocess_prompt");
    }

    #[test]
    fn vllm_default_url_constant_is_well_formed() {
        // Trivial guard against accidental edits to the vLLM default.
        assert_eq!(VLLM_DEFAULT_BASE_URL, "http://localhost:8000/v1");
    }
}
