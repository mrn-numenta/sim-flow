//! `Presenter` trait: the orchestrator's interface to "the thing the
//! user sees and types into."
//!
//! Today the orchestrator drives a single `Host` trait that does two
//! jobs at once: it owns the user-facing presentation (showing
//! assistant text, requesting user input, surfacing diagnostics) AND
//! it dispatches LLM calls (when the orchestrator emits
//! `Event::RequestLlmResponse`, the `Host` impl is expected to
//! synthesize the response). Conflating these two roles is what
//! forced the VS Code extension to be the LLM client for every
//! backend, including `openai-compat` / `ollama` / `anthropic` that
//! the orchestrator can perfectly well dispatch itself.
//!
//! `Presenter` is the cleaner seam: it only carries user-facing
//! events. LLM dispatch moves to a separate [`LlmAdapter`] trait
//! ([`super::llm_adapter`]) so any UI surface -- VS Code, egui,
//! makepad, web, terminal -- implements just `Presenter` and the
//! orchestrator handles LLM dispatch internally regardless of UI.
//!
//! The transition is incremental: `Presenter` lives alongside `Host`
//! today, the orchestrator still uses `Host`, and a blanket impl
//! lets every `Host` implementer satisfy `Presenter` as well. The
//! orchestrator rewiring (step 2) flips the dependency to
//! `Presenter` + `LlmAdapter`; then `Host` and `TerminalHost` get
//! deleted.
//!
//! [`LlmAdapter`]: super::llm_adapter::LlmAdapter

use crate::Result;
use crate::session::protocol::{Event, HostEvent};

/// What every UI surface implements. Sync because the orchestrator
/// is sync; async surfaces wrap a blocking adapter (the same shape
/// as the existing `Host` trait).
///
/// The shape mirrors `Host` exactly except that `RequestLlmResponse`
/// (an `Event` variant) and `LlmChunk` / `LlmEnd` / `LlmError`
/// (`HostEvent` variants) are now handled by [`LlmAdapter`] inside
/// the orchestrator. Once the rewiring lands, the orchestrator no
/// longer emits `Event::RequestLlmResponse` to the presenter at all
/// for any backend, so presenter impls don't need a special case for
/// it.
///
/// [`LlmAdapter`]: super::llm_adapter::LlmAdapter
pub trait Presenter {
    /// Send a user-facing event to the presenter (assistant text,
    /// diagnostics, gate results, etc.). Errors propagate; the
    /// orchestrator stops the session on a failed send.
    fn send(&mut self, event: &Event) -> Result<()>;

    /// Block waiting for the next user-originated event from the
    /// presenter (user-typed text, button click, followup
    /// selection). Returns `Ok(None)` when the presenter channel
    /// closes cleanly.
    fn recv(&mut self) -> Result<Option<HostEvent>>;
}

/// Blanket implementation: any `Host` is automatically a `Presenter`.
/// Lets us introduce `Presenter` without breaking existing call sites
/// during the multi-commit refactor. After step 2 lands (orchestrator
/// takes `&mut dyn Presenter` directly), this impl becomes the only
/// way the legacy `JsonlHost` etc. participate -- and after step 5
/// (`Host` deletion) it goes away with them.
impl<H: super::host::Host + ?Sized> Presenter for H {
    fn send(&mut self, event: &Event) -> Result<()> {
        self.write(event)
    }
    fn recv(&mut self) -> Result<Option<HostEvent>> {
        self.read()
    }
}
