//! In-memory CLI agent for tests. Drains a scripted queue of
//! responses; the TerminalHost integration tests build one of these
//! up front and then run a session against it.

use std::cell::RefCell;
use std::collections::VecDeque;

use super::{AdvertisedToolCall, CliAgent, LlmCallMetrics, ToolAdvertise};
use crate::Result;
use crate::session::protocol::LlmMessage;

/// Test agent that returns canned responses in FIFO order.
///
/// `RefCell` is fine here because all uses live inside a single
/// thread (the test runner). For production agents the trait is
/// `Send`, so anything stateful uses `Arc<Mutex<...>>`.
pub struct MockAgent {
    label: String,
    responses: RefCell<VecDeque<MockResponse>>,
    /// Records every messages-vector passed in so tests can assert
    /// what the orchestrator sent.
    pub seen: RefCell<Vec<Vec<LlmMessage>>>,
    /// Records every tool catalog passed to dispatch_with_tools so
    /// tests can assert native-mode dispatch was actually used (vs
    /// the trait's default fall-through that silently drops the
    /// catalog -- the bug fixed in commit 12956e6).
    pub seen_tools: RefCell<Vec<Vec<ToolAdvertise>>>,
}

/// One scripted dispatch outcome. The text-only queue path lives
/// behind `enqueue(text)`; native-tool-call testing pushes
/// `(text, tool_calls)` pairs via `enqueue_with_tool_calls`.
#[derive(Debug, Clone, Default)]
struct MockResponse {
    text: String,
    tool_calls: Vec<AdvertisedToolCall>,
}

impl MockAgent {
    pub fn new() -> Self {
        Self {
            label: "mock".into(),
            responses: RefCell::new(VecDeque::new()),
            seen: RefCell::new(Vec::new()),
            seen_tools: RefCell::new(Vec::new()),
        }
    }

    pub fn enqueue(&self, response: impl Into<String>) -> &Self {
        self.responses.borrow_mut().push_back(MockResponse {
            text: response.into(),
            tool_calls: Vec::new(),
        });
        self
    }

    /// Enqueue a turn that carries native tool calls (and optionally
    /// some text). Mirrors what a real OpenAI / Anthropic backend
    /// returns when the model picked a function call: `text` is the
    /// non-tool prose (commonly empty), and each entry in
    /// `tool_calls` becomes one `AdvertisedToolCall` the orchestrator
    /// dispatches. The empty-response regression test against the
    /// "Your previous response was empty" retry path uses
    /// `(text: "", tool_calls: [...])` to simulate the live
    /// shape that surfaced the bug.
    pub fn enqueue_with_tool_calls(
        &self,
        text: impl Into<String>,
        tool_calls: Vec<AdvertisedToolCall>,
    ) -> &Self {
        self.responses.borrow_mut().push_back(MockResponse {
            text: text.into(),
            tool_calls,
        });
        self
    }
}

impl Default for MockAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl CliAgent for MockAgent {
    fn name(&self) -> &str {
        &self.label
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        self.seen.borrow_mut().push(messages.to_vec());
        let resp = self.responses.borrow_mut().pop_front().unwrap_or_default();
        // Mock agent: no real LLM call; emit zeroed metrics so
        // tests that aggregate per-session totals stay deterministic.
        Ok((resp.text, LlmCallMetrics::default()))
    }

    /// Override the trait default so anomaly-repro tests can verify
    /// the orchestrator actually went through the native-tool-call
    /// path (vs silently falling back to fenced extraction). The
    /// supplied catalog is recorded in `seen_tools` for assertions,
    /// and the scripted `MockResponse.tool_calls` is returned
    /// verbatim alongside the text.
    fn dispatch_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolAdvertise],
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        self.seen.borrow_mut().push(messages.to_vec());
        self.seen_tools.borrow_mut().push(tools.to_vec());
        let resp = self.responses.borrow_mut().pop_front().unwrap_or_default();
        Ok((resp.text, resp.tool_calls, LlmCallMetrics::default()))
    }
}

// `MockAgent` uses `RefCell` so it isn't `Sync`, but the trait only
// requires `Send` - and `RefCell<T>: Send` when `T: Send`, which
// `VecDeque<String>` and `Vec<Vec<LlmMessage>>` both are.
