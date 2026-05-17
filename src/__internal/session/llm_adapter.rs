//! `LlmAdapter` trait: the orchestrator's interface to "the thing
//! that turns prompts into model output."
//!
//! Separated from [`super::presenter::Presenter`] so UI surfaces
//! (`Presenter` impls) don't have to know how to dispatch LLM calls.
//! Every LLM backend lives behind this trait: OpenAI-compat (vLLM /
//! LM Studio / openai-compat / openai aliases), Anthropic, Ollama,
//! `claude` / `codex` / `gh-copilot` CLIs. They keep speaking HTTP
//! / spawning subprocesses; only the trait they implement changes
//! name.
//!
//! Under the hood `LlmAdapter` is `CliAgent` renamed -- the existing
//! agent impls (`OpenAiCompatAgent`, `AnthropicAgent`, `ClaudeAgent`,
//! `CodexAgent`, `OllamaAgent`, `GhCopilotAgent`) all satisfy it as
//! soon as the rename is mechanical. The old name stays for now via
//! a `pub use` re-export so the incremental refactor keeps building
//! between commits; step 5 (`TerminalHost` deletion) is where the
//! re-export goes away.

use crate::Result;
use crate::session::agent::{
    AdvertisedToolCall, AgentAdaptationSummary, LlmCallMetrics, StreamingChunk, ToolAdvertise,
};
use crate::session::protocol::LlmMessage;

/// Dispatch interface every LLM backend implements. Sync because the
/// orchestrator is sync; HTTP-driven adapters block the calling
/// thread for the duration of the request.
///
/// `Send + Sync`: production adapters (`ClaudeAgent`,
/// `OpenAiCompatAgent`, `AnthropicAgent`, `OllamaAgent`, `CodexAgent`,
/// `GhCopilotAgent`) hold immutable config and have no interior
/// mutability, so they compose to `Sync` trivially. `MockAgent` uses
/// `Mutex` (not `RefCell`) for its scripted-response queue so it
/// satisfies the bound too. The bound is load-bearing for the
/// parallel plan-detail walk dispatcher in
/// `session::auto::run_plan_detail_walk_parallel`, which shares one
/// adapter across worker threads.
///
/// `dispatch_with_tools` is the native-tool-call path; native-aware
/// backends thread the tool catalog into the request and parse
/// returned tool invocations. Backends that don't support native
/// tools (e.g. subprocess CLIs) inherit the default impl which
/// drops the catalog and returns the plain dispatch result with no
/// tool calls.
pub trait LlmAdapter: Send + Sync {
    /// Short identifier for diagnostics ("openai-compat", "claude",
    /// "ollama", etc.). Surfaced in tracing/metrics.
    fn name(&self) -> &str;

    /// Run the prompt and return the assistant's text + per-call
    /// metrics. Errors propagate as `crate::Error`.
    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)>;

    /// Native-tool-call variant. Default impl drops the catalog and
    /// returns the existing `dispatch` result with no tool calls --
    /// good enough for subprocess CLIs and openai-compat endpoints
    /// without `--enable-auto-tool-choice`. Native-aware adapters
    /// override.
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
    /// tuple. The orchestrator forwards Text chunks as
    /// `AssistantText { final_chunk: false }` events so the chat
    /// panel renders tokens live; the final return drives the
    /// `final_chunk: true` close + tool-call routing.
    ///
    /// Mid-stream cancel: on cancel flag flip the backend stops
    /// reading, drops the transport, and returns
    /// `Ok((buffered_text, [], metrics { cancelled: true, .. }))`.
    /// The orchestrator commits the partial turn into history and
    /// then emits `SessionEnd::Cancelled`.
    ///
    /// Default implementation buffers via `dispatch_with_tools` and
    /// emits one synthetic final chunk.
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

    /// Optional adaptation summary for the diagnostic line emitted
    /// when `--llm-debug-adaptation` is set. `None` means "no
    /// adaptation tracking for this backend."
    fn adaptation_summary(&self) -> Option<AgentAdaptationSummary> {
        None
    }
}

/// Blanket: every `CliAgent` (the legacy trait) satisfies
/// `LlmAdapter`. Lets the rewiring step add new call sites that take
/// `&mut dyn LlmAdapter` without needing the agent impls renamed
/// first. After step 5 (`TerminalHost` deletion + cleanup) the
/// adapters get migrated to impl `LlmAdapter` directly and this
/// blanket goes away.
impl<A: crate::session::agent::CliAgent + ?Sized> LlmAdapter for A {
    fn name(&self) -> &str {
        crate::session::agent::CliAgent::name(self)
    }
    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        crate::session::agent::CliAgent::dispatch(self, messages)
    }
    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        crate::session::agent::CliAgent::dispatch_with_tools(self, messages, tools)
    }
    fn dispatch_streaming(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        crate::session::agent::CliAgent::dispatch_streaming(self, messages, tools, on_chunk)
    }
    fn adaptation_summary(&self) -> Option<AgentAdaptationSummary> {
        crate::session::agent::CliAgent::adaptation_summary(self)
    }
}
