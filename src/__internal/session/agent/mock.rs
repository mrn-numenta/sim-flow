//! In-memory CLI agent for tests. Drains a scripted queue of
//! responses; the TerminalHost integration tests build one of these
//! up front and then run a session against it.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::{AdvertisedToolCall, CliAgent, LlmCallMetrics, ToolAdvertise};
use crate::Result;
use crate::session::protocol::LlmMessage;

/// Test agent that returns canned responses in FIFO order.
///
/// `Mutex` (not `RefCell`) so the agent satisfies `LlmAdapter`'s
/// `Send + Sync` bound -- the parallel plan-detail walk dispatcher
/// shares one adapter across worker threads, and MockAgent has to
/// fit that contract too so the parallel path is testable.
///
/// # Threading caveat for `seen` and `seen_tools`
///
/// Both vectors record dispatches in the order the mutex grants
/// them. Under the parallel-walk dispatcher, multiple worker
/// threads call `dispatch` concurrently and **the recording order
/// is non-deterministic** -- whichever worker wins the lock first
/// occupies the next slot. Tests that drive parallel sessions
/// MUST NOT assert positional equality (e.g. `seen[0] ==
/// "milestone-01 prompt"`); use a set-based assertion or tag the
/// prompt content with the milestone name and assert the SET of
/// recorded prompts matches. Single-threaded tests retain FIFO
/// ordering and may keep positional asserts.
pub struct MockAgent {
    label: String,
    responses: Mutex<VecDeque<MockResponse>>,
    /// Records every messages-vector passed in so tests can assert
    /// what the orchestrator sent. **Non-deterministic order under
    /// parallel dispatch** -- see the type-level docs.
    pub seen: Mutex<Vec<Vec<LlmMessage>>>,
    /// Records every tool catalog passed to dispatch_with_tools so
    /// tests can assert native-mode dispatch was actually used (vs
    /// the trait's default fall-through that silently drops the
    /// catalog -- the bug fixed in commit 12956e6).
    /// **Non-deterministic order under parallel dispatch** -- see
    /// the type-level docs.
    pub seen_tools: Mutex<Vec<Vec<ToolAdvertise>>>,
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
            responses: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
            seen_tools: Mutex::new(Vec::new()),
        }
    }

    pub fn enqueue(&self, response: impl Into<String>) -> &Self {
        self.responses.lock().unwrap().push_back(MockResponse {
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
        self.responses.lock().unwrap().push_back(MockResponse {
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
        self.seen.lock().unwrap().push(messages.to_vec());
        let resp = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
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
        self.seen.lock().unwrap().push(messages.to_vec());
        self.seen_tools.lock().unwrap().push(tools.to_vec());
        let resp = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        Ok((resp.text, resp.tool_calls, LlmCallMetrics::default()))
    }
}
