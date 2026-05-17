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
mod anthropic;
mod cancel;
mod claude;
mod codex;
mod gh_copilot;
pub mod interactive_pty;
mod mock;
mod ollama;
mod openai_compat;

pub(crate) use adaptation::{
    CLAUDE_CLI_RUNTIME, OPENAI_COMPAT_GENERIC_RUNTIME, RuntimeCapabilityProfile,
    apply_reasoning_history_policy, normalize_response_text, prepare_messages_for_openai_compat,
    resolve_model_family, resolve_runtime_profile,
};
pub use anthropic::AnthropicAgent;
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
    /// True when `dispatch_streaming` returned a partial response
    /// because the shared cancel flag flipped mid-stream. The
    /// `text` / `tool_calls` returned alongside hold whatever
    /// content arrived before the cancel; the orchestrator commits
    /// them as the assistant turn and then emits
    /// `SessionEnd::Cancelled`. False on clean completion and on
    /// the buffered (non-streaming) fallback path.
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAdaptationSummary {
    pub backend: String,
    pub runtime_profile_id: String,
    pub model_family_id: String,
    pub request_format: String,
    pub system_prompt_mode: String,
    pub credential_policy: String,
    pub supports_structured_reasoning: bool,
    pub supports_structured_tool_calls: bool,
    pub supports_thinking_controls: bool,
}

impl AgentAdaptationSummary {
    pub fn format(&self) -> String {
        format!(
            "backend={}, runtime={}, family={}, request={}, system={}, credentials={}, structured-reasoning={}, structured-tools={}, thinking-controls={}",
            self.backend,
            self.runtime_profile_id,
            self.model_family_id,
            self.request_format,
            self.system_prompt_mode,
            self.credential_policy,
            yes_no(self.supports_structured_reasoning),
            yes_no(self.supports_structured_tool_calls),
            yes_no(self.supports_thinking_controls),
        )
    }
}

/// Vendor-neutral tool descriptor the orchestrator hands to an
/// agent that supports native function calling. Each impl translates
/// to its on-the-wire shape (OpenAI `tools[].function.{name,
/// description, parameters}`, Anthropic `tools[].{name, description,
/// input_schema}`, etc).
///
/// Build one from each `crate::session::tools::Tool` at orchestrator
/// session start: `name = t.name()`, `description = t.description()`,
/// `parameters = t.args_schema()`.
#[derive(Debug, Clone)]
pub struct ToolAdvertise {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Vendor-neutral tool call returned by an agent that supports
/// native function calling. `id` is the wire-side call id (used to
/// thread tool results back into the next request); `arguments_json`
/// is the raw JSON-encoded argument blob the model emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedToolCall {
    pub id: Option<String>,
    pub name: String,
    pub arguments_json: String,
}

/// One incremental piece of an LLM response delivered by
/// `dispatch_streaming`. Backends emit `Text` chunks as the model
/// produces output so the dashboard can render tokens live; tool
/// calls are buffered internally and surfaced only via the final
/// return value (per-arg tool-call streaming is openai-compat-
/// specific and the dashboard has no live render path for partial
/// tool args today).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingChunk {
    /// Incremental assistant text. Backends accumulate the same
    /// content into their final returned `text` so non-streaming
    /// callers (or a fallback through `dispatch_with_tools`) see
    /// the complete response.
    Text(String),
    /// Incremental reasoning text emitted alongside the visible
    /// response. vLLM with `--reasoning-parser qwen3` (and OpenAI's
    /// reasoning-effort API) splits the model's `<think>...</think>`
    /// output into a separate `reasoning_content` channel; backends
    /// surface those deltas as `Reasoning` chunks so the chat panel
    /// can render a collapsed-by-default reasoning block and the
    /// orchestrator can thread the prior turn's thinking back into
    /// history. Unlike `Text`, reasoning is NOT accumulated into the
    /// `dispatch_streaming` return tuple -- the orchestrator buffers
    /// it from the chunk stream directly.
    Reasoning(String),
}

pub trait CliAgent: Send + Sync {
    fn name(&self) -> &str;
    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)>;
    /// Dispatch with a tool catalog advertised to the model. Default
    /// implementation drops the catalog and returns the existing
    /// text+metrics with no tool calls -- agents that don't support
    /// native function calling (subprocess CLIs, OpenAI-compat
    /// without `--enable-auto-tool-choice`) get this behavior for
    /// free. Native-tool-aware agents override to thread tools into
    /// the request and parse tool_calls / tool_use out of the
    /// response.
    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        _tools: &[ToolAdvertise],
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let (text, metrics) = self.dispatch(messages)?;
        Ok((text, Vec::new(), metrics))
    }
    /// Streaming variant of `dispatch_with_tools`. Backends that
    /// support server-sent events / line-buffered streaming emit
    /// `StreamingChunk::Text` callbacks as the model produces output;
    /// the final return is the complete `(text, tool_calls, metrics)`
    /// tuple, identical in shape to `dispatch_with_tools`. The
    /// callback signature is `&mut dyn FnMut(StreamingChunk)` so the
    /// caller can route chunks anywhere (e.g. the orchestrator
    /// forwards them as `AssistantText { final_chunk: false }`
    /// events for live rendering).
    ///
    /// Cancel semantics during streaming: on a cancel flag flip the
    /// backend stops reading the stream, drops the underlying
    /// transport, and returns `Ok((buffered_text, [],
    /// LlmCallMetrics { cancelled: true, .. }))`. The orchestrator
    /// treats `cancelled = true` as a clean partial turn -- it
    /// commits the streamed content into the prompt history and
    /// then emits `SessionEnd::Cancelled`. This trades the
    /// "abandon the worker, lose partial content" behavior of
    /// `dispatch_with_tools` for "stop reading, keep partial
    /// content" -- the chat panel sees whatever streamed before
    /// the click as a finalized bubble instead of a "No response
    /// received." stub.
    ///
    /// Default implementation buffers the entire response via
    /// `dispatch_with_tools` and emits one synthetic `Text` chunk
    /// at the end, so non-streaming backends (mock, anything that
    /// hasn't been migrated yet) keep working transparently.
    fn dispatch_streaming(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let (text, calls, metrics) = self.dispatch_with_tools(messages, tools)?;
        if !text.is_empty() {
            on_chunk(StreamingChunk::Text(text.clone()));
        }
        Ok((text, calls, metrics))
    }
    fn adaptation_summary(&self) -> Option<AgentAdaptationSummary> {
        None
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
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
    /// Optional explicit runtime capability profile override.
    pub runtime_profile_id: Option<String>,
    /// Emit extra adaptation diagnostics around LLM dispatches.
    pub debug_adaptation: bool,
    /// Generic base-URL override. Wins over the per-backend
    /// `ollama_base_url` / `openai_base_url` fields when set.
    /// Use this for `vllm` / generic openai-compat servers and
    /// for any user-defined endpoint that doesn't fit the
    /// conventional default. None means "use the backend's
    /// conventional default".
    pub base_url: Option<String>,
    pub ollama_base_url: Option<String>,
    pub openai_base_url: Option<String>,
    /// Shared cancellation flag. Set by the control-socket listener
    /// (see `SocketPresenter`) when the dashboard pushes a cancel
    /// while an LLM dispatch is mid-call. Backends that support
    /// mid-dispatch cancellation (subprocess: kill the child; HTTP:
    /// abandon the ureq call) check this on a short polling cadence
    /// and return `Error::Cancelled` when it flips. Backends that
    /// don't implement mid-call cancellation simply ignore the
    /// flag; their cancel still arrives via the main protocol
    /// socket's `HostEvent::Cancel` at the next `host.recv()`
    /// boundary. The orchestrator routes `Error::Cancelled` from
    /// dispatch through the same SessionEnd::Cancelled path it
    /// uses for that wire event, so both routes converge.
    ///
    /// Optional so existing callers (tests, in-process unit tests,
    /// any code path that doesn't construct a control socket) keep
    /// working without changes -- `None` is "no cancellation
    /// channel", equivalent to a permanently-false flag.
    pub cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
        // `anthropic` only honors a generic `base_url` override
        // (proxy / mock). No per-backend default goes in here --
        // the agent constructor substitutes `api.anthropic.com`.
        "anthropic" | "anthropic-api" => config.base_url.clone(),
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
        "anthropic" | "anthropic-api" => Some(Box::new(AnthropicAgent::new_with_cancel(
            // `anthropic` always talks to api.anthropic.com unless
            // the caller supplied an explicit `--llm-base-url`
            // (rare; useful only for proxies / mock servers).
            resolved,
            config.model,
            config.model_family_id,
            config.cancel_flag.clone(),
        ))),
        "claude" | "claude-cli" => Some(Box::new(ClaudeAgent::new_with_cancel(
            config.model,
            config.model_family_id,
            config.runtime_profile_id,
            config.cancel_flag.clone(),
        ))),
        "codex" | "codex-cli" => Some(Box::new(CodexAgent::new_with_cancel(
            config.model,
            config.cancel_flag.clone(),
        ))),
        "gh-copilot" | "gh_copilot" => Some(Box::new(GhCopilotAgent::new_with_cancel(
            config.cancel_flag.clone(),
        ))),
        "ollama" => Some(Box::new(OllamaAgent::new_with_cancel(
            resolved,
            config.model,
            config.model_family_id,
            config.runtime_profile_id,
            config.cancel_flag.clone(),
        ))),
        // LM Studio uses the OpenAI-compat agent with its
        // conventional `:1234/v1` default. Aliasing it explicitly
        // (rather than asking users to type `openai-compat`) makes
        // the dashboard's Source dropdown read naturally.
        // vLLM uses the same OpenAI-compat path with a `:8000/v1`
        // default that `resolved_base_url` substitutes.
        "lmstudio" | "lm-studio" | "vllm" | "openai-compat" | "openai_compat" | "openai" => {
            Some(Box::new(OpenAiCompatAgent::new_with_cancel(
                resolved,
                config.model,
                config.model_family_id,
                config.runtime_profile_id,
                config.cancel_flag.clone(),
            )))
        }
        _ => None,
    }
}

/// Names accepted by `build_cli_agent`. Stable for help text /
/// error messages.
pub const KNOWN_AGENTS: &[&str] = &[
    "anthropic",
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
            runtime_profile_id: None,
            debug_adaptation: false,
            base_url: base.map(String::from),
            ollama_base_url: ollama.map(String::from),
            openai_base_url: openai.map(String::from),
            cancel_flag: None,
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
        let a = OllamaAgent::new(None, None, None, None);
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
            None,
        );
        assert_eq!(a.base_url(), "http://x:9/v1");
        assert_eq!(a.model(), "qwen3.6");
        assert_eq!(a.model_family_id(), Some("qwen3_6"));
    }

    #[test]
    fn openai_compat_agent_substitutes_default_url() {
        let a = OpenAiCompatAgent::new(None, None, None, None);
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
            None,
        );
        assert_eq!(a.model_family_id(), Some("gemma4"));
    }

    #[test]
    fn claude_agent_uses_claude_cli_runtime_profile() {
        let a = ClaudeAgent::new(Some("claude-sonnet-4-6".into()), None, None);
        assert_eq!(a.runtime_profile().request_format, "subprocess_prompt");
    }

    #[test]
    fn default_dispatch_with_tools_drops_catalog_and_returns_no_calls() {
        // Non-tool-aware agents (the trait default impl) must
        // return the same (text, metrics) as plain dispatch and
        // never invent tool calls regardless of catalog size.
        use crate::session::protocol::{LlmAttachment, LlmRole};

        struct StubAgent;
        impl CliAgent for StubAgent {
            fn name(&self) -> &str {
                "stub"
            }
            fn dispatch(&self, _messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
                Ok(("hello".to_string(), LlmCallMetrics::default()))
            }
        }

        let agent = StubAgent;
        let messages: Vec<LlmMessage> = vec![LlmMessage {
            role: LlmRole::User,
            content: "x".into(),
            attachments: Vec::<LlmAttachment>::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        }];
        let tools = vec![ToolAdvertise {
            name: "list_dir".into(),
            description: "list dir".into(),
            parameters: serde_json::json!({"type":"object"}),
        }];
        let (text, calls, _m) = agent.dispatch_with_tools(&messages, &tools).unwrap();
        assert_eq!(text, "hello");
        assert!(calls.is_empty());
    }

    #[test]
    fn vllm_default_url_constant_is_well_formed() {
        // Trivial guard against accidental edits to the vLLM default.
        assert_eq!(VLLM_DEFAULT_BASE_URL, "http://localhost:8000/v1");
    }

    #[test]
    fn yes_no_returns_yes_and_no_strings() {
        assert_eq!(yes_no(true), "yes");
        assert_eq!(yes_no(false), "no");
    }

    #[test]
    fn resolved_base_url_anthropic_only_honors_generic_base() {
        // No generic -> None (constructor substitutes api.anthropic.com).
        assert_eq!(
            resolved_base_url("anthropic", &cfg(None, None, None, None)),
            None,
        );
        // Generic set -> returned verbatim.
        assert_eq!(
            resolved_base_url("anthropic", &cfg(Some("http://proxy"), None, None, None)).as_deref(),
            Some("http://proxy"),
        );
        // anthropic-api alias maps identically.
        assert_eq!(
            resolved_base_url(
                "anthropic-api",
                &cfg(Some("http://proxy"), None, None, None)
            )
            .as_deref(),
            Some("http://proxy"),
        );
        // The legacy per-backend openai/ollama fields are ignored.
        assert_eq!(
            resolved_base_url(
                "anthropic",
                &cfg(None, Some("http://o"), Some("http://oai"), None)
            ),
            None,
        );
    }

    #[test]
    fn resolved_base_url_vllm_falls_back_to_default_when_no_overrides() {
        let r = resolved_base_url("vllm", &cfg(None, None, None, None));
        assert_eq!(r.as_deref(), Some(VLLM_DEFAULT_BASE_URL));
        // openai_base_url override wins over the default.
        let r = resolved_base_url("vllm", &cfg(None, None, Some("http://prox:1234/v1"), None));
        assert_eq!(r.as_deref(), Some("http://prox:1234/v1"));
        // Generic --llm-base-url wins over everything.
        let r = resolved_base_url(
            "vllm",
            &cfg(Some("http://gen"), None, Some("http://lega"), None),
        );
        assert_eq!(r.as_deref(), Some("http://gen"));
    }

    #[test]
    fn build_cli_agent_returns_none_for_unknown_backend() {
        let cfg = AgentConfig {
            model: None,
            model_family_id: None,
            runtime_profile_id: None,
            debug_adaptation: false,
            base_url: None,
            ollama_base_url: None,
            openai_base_url: None,
            cancel_flag: None,
        };
        assert!(build_cli_agent("not-a-real-backend", cfg).is_none());
    }
}
