//! In-memory CLI agent for tests. Drains a scripted queue of
//! responses; the TerminalHost integration tests build one of these
//! up front and then run a session against it.

use std::cell::RefCell;
use std::collections::VecDeque;

use super::CliAgent;
use crate::Result;
use crate::session::protocol::LlmMessage;

/// Test agent that returns canned responses in FIFO order.
///
/// `RefCell` is fine here because all uses live inside a single
/// thread (the test runner). For production agents the trait is
/// `Send`, so anything stateful uses `Arc<Mutex<...>>`.
pub struct MockAgent {
    label: String,
    responses: RefCell<VecDeque<String>>,
    /// Records every messages-vector passed in so tests can assert
    /// what the orchestrator sent.
    pub seen: RefCell<Vec<Vec<LlmMessage>>>,
}

impl MockAgent {
    pub fn new() -> Self {
        Self {
            label: "mock".into(),
            responses: RefCell::new(VecDeque::new()),
            seen: RefCell::new(Vec::new()),
        }
    }

    pub fn enqueue(&self, response: impl Into<String>) -> &Self {
        self.responses.borrow_mut().push_back(response.into());
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

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<String> {
        self.seen.borrow_mut().push(messages.to_vec());
        match self.responses.borrow_mut().pop_front() {
            Some(text) => Ok(text),
            None => Ok(String::new()),
        }
    }
}

// `MockAgent` uses `RefCell` so it isn't `Sync`, but the trait only
// requires `Send` - and `RefCell<T>: Send` when `T: Send`, which
// `VecDeque<String>` and `Vec<Vec<LlmMessage>>` both are.
