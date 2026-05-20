//! Shared HTTP client for OpenAI-compatible chat-completion endpoints.
//!
//! Ollama and LM Studio expose this same wire format on their local
//! servers; the existing TS extension uses an `OpenAiCompatibleBackend`
//! base class for the same reason. This Rust module mirrors that
//! pattern so the per-server agent impls stay tiny.
//!
//! Submodules group the implementation:
//!   - [`request`] -- `OpenAiCompatibleRequest` + builders + env helpers
//!   - [`wire`] -- on-the-wire chat-completions body / response shapes
//!   - [`dispatch`] -- `dispatch_chat_with_tools` round-trip + decode

mod dispatch;
mod dispatch_streaming;
mod request;
mod retry;
mod wire;

#[cfg(test)]
mod tests;

pub use dispatch::dispatch_chat_with_tools;
pub use dispatch_streaming::dispatch_chat_with_tools_streaming;
pub use request::OpenAiCompatibleRequest;

use crate::Result;
use crate::session::agent::LlmCallMetrics;

/// Send a synchronous chat-completions request and return the
/// assistant's full text plus per-call metrics (prompt /
/// completion tokens from the response `usage` object, total
/// wall-clock round-trip time). The endpoint defaults to
/// `<base_url>/chat/completions`.
///
/// Back-compat shim. Callers that need tool_calls should switch to
/// `dispatch_chat_with_tools`. `cancel_flag = None` skips the
/// control-socket cancellation channel, equivalent to a permanently
/// false flag.
pub fn dispatch_chat(
    req: OpenAiCompatibleRequest<'_>,
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(String, LlmCallMetrics)> {
    let resp = dispatch_chat_with_tools(req, cancel_flag)?;
    Ok((resp.text, resp.metrics))
}
