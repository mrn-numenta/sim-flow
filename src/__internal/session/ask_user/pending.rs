//! `PendingUserAsk` and `AskUserAnswer` -- the per-call records that
//! cross the suspend/resume boundary.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::__internal::session::protocol::StepMode;

/// How the agent intends the resolved thread to be persisted in
/// spec.md. `None` is "ephemeral" and applies to intermediate calls
/// within a multi-turn chained thread; the closing call sets one of
/// the other two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordAs {
    #[default]
    OpenQuestion,
    AutoDecision,
    None,
}

impl RecordAs {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordAs::OpenQuestion => "open-question",
            RecordAs::AutoDecision => "auto-decision",
            RecordAs::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open-question" => Some(RecordAs::OpenQuestion),
            "auto-decision" => Some(RecordAs::AutoDecision),
            "none" => Some(RecordAs::None),
            _ => None,
        }
    }
}

/// Shape of the reply the agent expects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskUserKind {
    #[default]
    FreeForm,
    YesNo,
    Choice,
    Value,
}

impl AskUserKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AskUserKind::FreeForm => "free-form",
            AskUserKind::YesNo => "yes-no",
            AskUserKind::Choice => "choice",
            AskUserKind::Value => "value",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "free-form" => Some(AskUserKind::FreeForm),
            "yes-no" => Some(AskUserKind::YesNo),
            "choice" => Some(AskUserKind::Choice),
            "value" => Some(AskUserKind::Value),
            _ => None,
        }
    }
}

/// One pending `ask_user` call -- the data the orchestrator must
/// remember between the tool-call suspending the LLM turn and the
/// user's reply that resumes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUserAsk {
    pub question: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub kind: AskUserKind,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub record_as: RecordAs,
    /// The tool-call id the LLM emitted for this `ask_user`
    /// invocation. The resume path returns the answer in a tool-result
    /// frame keyed by this id so the model's working memory pairs
    /// the answer to the right call.
    pub tool_call_id: String,
    /// Unix ms when the call was suspended.
    pub triggered_at_ms: u64,
    /// Step-mode at the moment of the call (so the resume can compute
    /// `mode_changed = "auto-to-manual"` after the flip).
    pub step_mode_before: StepMode,
    /// `thread_id` for chaining. Set by the orchestrator on a fresh
    /// thread; supplied verbatim by the agent for follow-up calls.
    pub thread_id: String,
    /// Turn index within the thread. `0` for the first call; +1 per
    /// follow-up.
    pub thread_turn_index: u32,
    /// Step id at the time of the call. Used to scope persistence
    /// to `.sim-flow/<step>/...`.
    pub step_id: String,
}

impl PendingUserAsk {
    /// Path of the on-disk checkpoint, relative to
    /// `<project>/.sim-flow/<step>/pending-ask.toml`.
    pub fn checkpoint_path(project_dir: &Path, step_id: &str) -> PathBuf {
        project_dir
            .join(".sim-flow")
            .join(step_id)
            .join("pending-ask.toml")
    }

    /// Write the pending ask to the checkpoint file. Parent dirs are
    /// created as needed.
    pub fn save_checkpoint(&self, project_dir: &Path) -> std::io::Result<()> {
        let path = Self::checkpoint_path(project_dir, &self.step_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(format!("serialize pending ask: {e}")))?;
        std::fs::write(path, body)?;
        Ok(())
    }

    /// Load a pending-ask checkpoint if it exists. Returns `Ok(None)`
    /// when the file is absent (no recovery needed).
    pub fn load_checkpoint(
        project_dir: &Path,
        step_id: &str,
    ) -> std::io::Result<Option<PendingUserAsk>> {
        let path = Self::checkpoint_path(project_dir, step_id);
        if !path.is_file() {
            return Ok(None);
        }
        let body = std::fs::read_to_string(&path)?;
        let pending: PendingUserAsk = toml::from_str(&body)
            .map_err(|e| std::io::Error::other(format!("parse pending ask: {e}")))?;
        Ok(Some(pending))
    }

    /// Delete the checkpoint (after the answer has been consumed).
    pub fn clear_checkpoint(project_dir: &Path, step_id: &str) -> std::io::Result<()> {
        let path = Self::checkpoint_path(project_dir, step_id);
        if path.is_file() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// The result the agent receives when the user's reply lands.
/// Shaped per Architecture §4.5 "Return shape".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AskUserAnswer {
    /// Verbatim user reply text.
    pub answer: String,
    /// Echo of the call's `kind`.
    pub kind: String,
    /// Thread handle (generated for fresh threads, echoed for
    /// follow-ups).
    pub thread_id: String,
    /// 0-based turn index within the thread.
    pub thread_turn_index: u32,
    /// Anchor where the resolved thread was persisted (empty when
    /// `record_as = "none"` or this call was intermediate).
    pub recorded_at: String,
    /// Set when the call flipped step-mode (`"auto-to-manual"` is
    /// the only value v1 emits). Empty otherwise.
    pub mode_changed: String,
    /// Wall-clock ms the agent was paused for.
    pub elapsed_ms: u64,
    /// `true` when the user issued `/cancel` (this call) or
    /// `/cancel-thread` (the whole thread).
    pub cancelled: bool,
    /// `true` only on `/cancel-thread`.
    pub thread_cancelled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_as_round_trips_through_str() {
        for v in [
            RecordAs::OpenQuestion,
            RecordAs::AutoDecision,
            RecordAs::None,
        ] {
            assert_eq!(RecordAs::parse(v.as_str()), Some(v));
        }
        assert_eq!(RecordAs::parse("unknown"), None);
    }

    #[test]
    fn ask_user_kind_round_trips() {
        for v in [
            AskUserKind::FreeForm,
            AskUserKind::YesNo,
            AskUserKind::Choice,
            AskUserKind::Value,
        ] {
            assert_eq!(AskUserKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn pending_ask_round_trips_through_checkpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let ask = PendingUserAsk {
            question: "Pick a number".into(),
            context: "We need a width".into(),
            kind: AskUserKind::Choice,
            choices: vec!["4".into(), "8".into()],
            default: Some("4".into()),
            record_as: RecordAs::AutoDecision,
            tool_call_id: "call-1".into(),
            triggered_at_ms: 1234567890,
            step_mode_before: StepMode::Auto,
            thread_id: "ask-DM2d-1".into(),
            thread_turn_index: 0,
            step_id: "DM2d".into(),
        };
        ask.save_checkpoint(tmp.path()).unwrap();
        let recovered = PendingUserAsk::load_checkpoint(tmp.path(), "DM2d")
            .unwrap()
            .expect("present");
        assert_eq!(recovered, ask);
        PendingUserAsk::clear_checkpoint(tmp.path(), "DM2d").unwrap();
        let after_clear = PendingUserAsk::load_checkpoint(tmp.path(), "DM2d").unwrap();
        assert!(after_clear.is_none());
    }
}
