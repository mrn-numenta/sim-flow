//! Multi-turn `ask_user` thread management.
//!
//! A "thread" is a sequence of `ask_user` calls sharing a
//! `thread_id`. The agent uses chaining to clarify ambiguous or
//! incomplete answers; the orchestrator's role is to track the
//! per-thread history, emit the per-call and per-close metric events,
//! coalesce intermediate calls so spec.md sees only the resolved
//! form, and force-close any open threads at sub-session end.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::pending::{PendingUserAsk, RecordAs};

/// How a thread was closed. Distinct from `RecordAs` because cancel
/// paths are not driven by the agent's `record_as` arg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClosedAs {
    OpenQuestion,
    AutoDecision,
    Cancelled,
    ThreadCancelled,
    /// Force-close at sub-session end with no recorded answer.
    Abandoned,
    /// Force-close at sub-session end with at least one recorded
    /// answer.
    ForceClosed,
}

impl ClosedAs {
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedAs::OpenQuestion => "open-question",
            ClosedAs::AutoDecision => "auto-decision",
            ClosedAs::Cancelled => "cancelled",
            ClosedAs::ThreadCancelled => "thread-cancelled",
            ClosedAs::Abandoned => "abandoned",
            ClosedAs::ForceClosed => "force-closed",
        }
    }
}

/// One Q+A turn within a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTurn {
    pub question: String,
    pub answer: String,
    /// 0-based.
    pub turn_index: u32,
    /// `record_as` value the agent supplied on this turn (verbatim).
    pub record_as: RecordAs,
    /// Whether the user `/cancel`led this single turn (without
    /// cancelling the whole thread).
    pub cancelled: bool,
}

/// In-memory state of an open thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadHandle {
    pub thread_id: String,
    pub step_id: String,
    pub opened_at_ms: u64,
    pub history: Vec<ThreadTurn>,
}

impl ThreadHandle {
    pub fn turn_count(&self) -> u32 {
        self.history.len() as u32
    }

    pub fn last_answer(&self) -> Option<&str> {
        self.history
            .iter()
            .rev()
            .find(|t| !t.cancelled)
            .map(|t| t.answer.as_str())
    }
}

/// Resolved-thread payload returned at close. The orchestrator's
/// persistence layer converts this into a spec.md entry (or
/// qa-buffer entry when spec.md doesn't exist yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedThread {
    pub thread_id: String,
    pub step_id: String,
    pub closed_as: ClosedAs,
    pub turn_count: u32,
    /// The question asked on turn 0 -- the canonical entry-point
    /// question whose body is written to spec.md.
    pub initial_question: String,
    /// The last non-cancelled answer (or empty when none).
    pub final_answer: String,
    /// Full history (intermediate turns survive only in metrics /
    /// chat log; this struct still carries them for completeness).
    pub history: Vec<ThreadTurn>,
}

/// Registry of open threads keyed by `thread_id`. The orchestrator
/// holds one per sub-session; force-closes any survivors at the
/// sub-session-end hook.
#[derive(Debug, Default)]
pub struct ThreadRegistry {
    open: BTreeMap<String, ThreadHandle>,
}

#[derive(Debug)]
pub enum ThreadRegistryError {
    UnknownThread(String),
    AlreadyClosed(String),
}

impl std::fmt::Display for ThreadRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadRegistryError::UnknownThread(id) => write!(f, "unknown thread_id `{id}`"),
            ThreadRegistryError::AlreadyClosed(id) => {
                write!(f, "thread `{id}` already closed")
            }
        }
    }
}

impl std::error::Error for ThreadRegistryError {}

impl ThreadRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    pub fn contains(&self, thread_id: &str) -> bool {
        self.open.contains_key(thread_id)
    }

    pub fn get(&self, thread_id: &str) -> Option<&ThreadHandle> {
        self.open.get(thread_id)
    }

    /// Open a fresh thread or look up an existing one. If
    /// `ask.thread_id` is empty / unknown the caller must decide
    /// whether to error or generate a new id -- this function does
    /// the lookup-or-error itself.
    pub fn open_or_continue(
        &mut self,
        ask: &PendingUserAsk,
    ) -> Result<&mut ThreadHandle, ThreadRegistryError> {
        if !self.open.contains_key(&ask.thread_id) {
            // Fresh thread: insert.
            self.open.insert(
                ask.thread_id.clone(),
                ThreadHandle {
                    thread_id: ask.thread_id.clone(),
                    step_id: ask.step_id.clone(),
                    opened_at_ms: ask.triggered_at_ms,
                    history: Vec::new(),
                },
            );
        }
        Ok(self.open.get_mut(&ask.thread_id).expect("just inserted"))
    }

    /// Look up an existing thread; error if absent.
    pub fn require(&mut self, thread_id: &str) -> Result<&mut ThreadHandle, ThreadRegistryError> {
        self.open
            .get_mut(thread_id)
            .ok_or_else(|| ThreadRegistryError::UnknownThread(thread_id.to_string()))
    }

    /// Append a Q+A turn to a thread's history. Emits the
    /// `ask_user_call` metric event.
    pub fn record_turn(
        &mut self,
        thread_id: &str,
        question: String,
        answer: String,
        record_as: RecordAs,
        cancelled: bool,
    ) -> Result<u32, ThreadRegistryError> {
        let thread = self.require(thread_id)?;
        let turn_index = thread.history.len() as u32;
        thread.history.push(ThreadTurn {
            question,
            answer,
            turn_index,
            record_as,
            cancelled,
        });
        tracing::info!(
            target: "sim_flow::metrics",
            event = "ask_user_call",
            thread_id = thread_id,
            step = thread.step_id.as_str(),
            thread_turn_index = turn_index,
            record_as = record_as.as_str(),
            cancelled = cancelled,
        );
        Ok(turn_index)
    }

    /// Close a thread and emit the `ask_user_thread_closed` metric
    /// event. Removes the entry from the registry.
    pub fn close_thread(
        &mut self,
        thread_id: &str,
        closed_as: ClosedAs,
    ) -> Result<ResolvedThread, ThreadRegistryError> {
        let handle = self
            .open
            .remove(thread_id)
            .ok_or_else(|| ThreadRegistryError::UnknownThread(thread_id.to_string()))?;
        let turn_count = handle.history.len() as u32;
        let initial_question = handle
            .history
            .first()
            .map(|t| t.question.clone())
            .unwrap_or_default();
        let final_answer = handle.last_answer().unwrap_or("").to_string();
        tracing::info!(
            target: "sim_flow::metrics",
            event = "ask_user_thread_closed",
            thread_id = thread_id,
            step = handle.step_id.as_str(),
            turn_count = turn_count,
            closed_as = closed_as.as_str(),
        );
        Ok(ResolvedThread {
            thread_id: handle.thread_id.clone(),
            step_id: handle.step_id.clone(),
            closed_as,
            turn_count,
            initial_question,
            final_answer,
            history: handle.history,
        })
    }

    /// Force-close every open thread per Architecture §6.5.5. Returns
    /// the resolved-thread payloads for the threads that recorded at
    /// least one answer; threads with no answers are dropped silently
    /// (the caller emits a metric for those via the registry's own
    /// instrumentation).
    pub fn force_close_all_on_subsession_end(&mut self) -> Vec<ResolvedThread> {
        let ids: Vec<String> = self.open.keys().cloned().collect();
        let mut resolved = Vec::new();
        for id in ids {
            let handle = self.open.remove(&id).expect("just enumerated");
            let has_answer = handle
                .history
                .iter()
                .any(|t| !t.cancelled && !t.answer.is_empty());
            if has_answer {
                let closed = ResolvedThread {
                    thread_id: handle.thread_id.clone(),
                    step_id: handle.step_id.clone(),
                    closed_as: ClosedAs::ForceClosed,
                    turn_count: handle.history.len() as u32,
                    initial_question: handle
                        .history
                        .first()
                        .map(|t| t.question.clone())
                        .unwrap_or_default(),
                    final_answer: handle.last_answer().unwrap_or("").to_string(),
                    history: handle.history,
                };
                tracing::info!(
                    target: "sim_flow::metrics",
                    event = "ask_user_thread_closed",
                    thread_id = id.as_str(),
                    step = closed.step_id.as_str(),
                    turn_count = closed.turn_count,
                    closed_as = closed.closed_as.as_str(),
                );
                resolved.push(closed);
            } else {
                tracing::info!(
                    target: "sim_flow::metrics",
                    event = "ask_user_thread_closed",
                    thread_id = id.as_str(),
                    step = handle.step_id.as_str(),
                    turn_count = 0,
                    closed_as = "abandoned",
                );
            }
        }
        resolved
    }

    /// Persist every currently-open thread's state to
    /// `.sim-flow/<step>/ask-threads/<thread_id>.toml`. Idempotent:
    /// writes the same file on every call.
    pub fn persist_open_threads(&self, project_dir: &Path) -> std::io::Result<()> {
        for handle in self.open.values() {
            let path = thread_path(project_dir, &handle.step_id, &handle.thread_id);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let body = toml::to_string_pretty(handle)
                .map_err(|e| std::io::Error::other(format!("serialize thread: {e}")))?;
            std::fs::write(path, body)?;
        }
        Ok(())
    }

    /// Load all on-disk thread state for `step_id` back into the
    /// registry. Used by reload recovery (Architecture §6.5.4).
    /// Already-loaded threads are kept; on-disk entries with the
    /// same id override.
    pub fn load_open_threads(
        &mut self,
        project_dir: &Path,
        step_id: &str,
    ) -> std::io::Result<usize> {
        let dir = project_dir
            .join(".sim-flow")
            .join(step_id)
            .join("ask-threads");
        if !dir.is_dir() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let body = std::fs::read_to_string(&path)?;
            let handle: ThreadHandle = toml::from_str(&body)
                .map_err(|e| std::io::Error::other(format!("parse thread: {e}")))?;
            self.open.insert(handle.thread_id.clone(), handle);
            count += 1;
        }
        Ok(count)
    }

    /// Remove the on-disk thread file (after close).
    pub fn clear_persisted_thread(
        project_dir: &Path,
        step_id: &str,
        thread_id: &str,
    ) -> std::io::Result<()> {
        let path = thread_path(project_dir, step_id, thread_id);
        if path.is_file() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn thread_path(project_dir: &Path, step_id: &str, thread_id: &str) -> PathBuf {
    project_dir
        .join(".sim-flow")
        .join(step_id)
        .join("ask-threads")
        .join(format!("{thread_id}.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::ask_user::pending::AskUserKind;
    use crate::__internal::session::protocol::StepMode;

    fn make_ask(thread_id: &str, turn: u32, step: &str) -> PendingUserAsk {
        PendingUserAsk {
            question: format!("q{turn}"),
            context: String::new(),
            kind: AskUserKind::FreeForm,
            choices: Vec::new(),
            default: None,
            record_as: if turn == 0 {
                RecordAs::OpenQuestion
            } else {
                RecordAs::None
            },
            tool_call_id: format!("call-{turn}"),
            triggered_at_ms: 1000 + turn as u64,
            step_mode_before: StepMode::Manual,
            thread_id: thread_id.to_string(),
            thread_turn_index: turn,
            step_id: step.to_string(),
        }
    }

    #[test]
    fn open_or_continue_creates_fresh_thread() {
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        assert!(reg.contains("t1"));
        assert_eq!(reg.open_count(), 1);
    }

    #[test]
    fn open_or_continue_returns_existing_thread() {
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        let ask2 = make_ask("t1", 1, "DM2d");
        reg.open_or_continue(&ask2).unwrap();
        assert_eq!(reg.open_count(), 1);
    }

    #[test]
    fn record_turn_appends_to_history() {
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        reg.record_turn("t1", "q0".into(), "a0".into(), RecordAs::None, false)
            .unwrap();
        reg.record_turn(
            "t1",
            "q1".into(),
            "a1".into(),
            RecordAs::OpenQuestion,
            false,
        )
        .unwrap();
        let handle = reg.get("t1").unwrap();
        assert_eq!(handle.history.len(), 2);
        assert_eq!(handle.history[0].turn_index, 0);
        assert_eq!(handle.history[1].turn_index, 1);
    }

    #[test]
    fn close_thread_returns_resolved_payload_and_removes_from_registry() {
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        reg.record_turn(
            "t1",
            "q0".into(),
            "a0".into(),
            RecordAs::OpenQuestion,
            false,
        )
        .unwrap();
        let resolved = reg.close_thread("t1", ClosedAs::OpenQuestion).unwrap();
        assert_eq!(resolved.thread_id, "t1");
        assert_eq!(resolved.turn_count, 1);
        assert_eq!(resolved.initial_question, "q0");
        assert_eq!(resolved.final_answer, "a0");
        assert!(!reg.contains("t1"));
    }

    #[test]
    fn close_unknown_thread_errors() {
        let mut reg = ThreadRegistry::new();
        let err = reg.close_thread("nope", ClosedAs::OpenQuestion);
        assert!(matches!(err, Err(ThreadRegistryError::UnknownThread(_))));
    }

    #[test]
    fn force_close_returns_threads_with_answers_drops_empty() {
        let mut reg = ThreadRegistry::new();
        let ask1 = make_ask("with-answer", 0, "DM2d");
        let ask2 = make_ask("empty", 0, "DM2d");
        reg.open_or_continue(&ask1).unwrap();
        reg.open_or_continue(&ask2).unwrap();
        reg.record_turn(
            "with-answer",
            "q".into(),
            "yes".into(),
            RecordAs::None,
            false,
        )
        .unwrap();
        let resolved = reg.force_close_all_on_subsession_end();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].thread_id, "with-answer");
        assert!(matches!(resolved[0].closed_as, ClosedAs::ForceClosed));
        assert_eq!(reg.open_count(), 0);
    }

    #[test]
    fn persist_and_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        reg.record_turn("t1", "q0".into(), "a0".into(), RecordAs::None, false)
            .unwrap();
        reg.persist_open_threads(tmp.path()).unwrap();

        // Fresh registry recovers the thread.
        let mut recovered = ThreadRegistry::new();
        let n = recovered.load_open_threads(tmp.path(), "DM2d").unwrap();
        assert_eq!(n, 1);
        let handle = recovered.get("t1").expect("recovered");
        assert_eq!(handle.history.len(), 1);
        assert_eq!(handle.history[0].answer, "a0");
    }

    #[test]
    fn clear_persisted_thread_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ThreadRegistry::new();
        let ask = make_ask("t1", 0, "DM2d");
        reg.open_or_continue(&ask).unwrap();
        reg.persist_open_threads(tmp.path()).unwrap();
        ThreadRegistry::clear_persisted_thread(tmp.path(), "DM2d", "t1").unwrap();
        let mut recovered = ThreadRegistry::new();
        let n = recovered.load_open_threads(tmp.path(), "DM2d").unwrap();
        assert_eq!(n, 0);
    }
}
