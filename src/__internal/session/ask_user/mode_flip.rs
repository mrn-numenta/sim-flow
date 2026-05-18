//! `flip_step_mode_for_ask_user` -- the auto→manual step-mode flip
//! triggered on the first `ask_user` call of a thread when the
//! current mode is `auto` (Architecture §6.5.2).
//!
//! State is kept in `<project>/.sim-flow/state.toml`'s
//! `current_step_mode` field. This module owns the read/modify/write
//! semantics and emits the `StepModeChanged` event plus the
//! `Diagnostic::Info` "ask_user invoked during auto run; flipping to
//! manual" message via the supplied callback. The callback indirection
//! keeps this module free of the orchestrator's host/presenter
//! plumbing -- unit tests pass a noop sink, the real orchestrator
//! passes a thunk that emits onto the live presenter.

use std::path::Path;

use crate::__internal::session::protocol::StepMode;

/// Outcome of a flip attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeFlipOutcome {
    /// Already manual; no state change emitted.
    NoChange,
    /// auto→manual transition occurred; `state.toml` was updated and
    /// the supplied emitter saw `StepModeChanged` + `Diagnostic::Info`.
    AutoToManual,
}

/// Path of the state file relative to the project root.
fn state_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join(".sim-flow").join("state.toml")
}

/// Read the current step mode from `state.toml`. Returns `manual` as
/// the safe default when the file is missing or malformed.
pub fn read_current_step_mode(project_dir: &Path) -> StepMode {
    let path = state_path(project_dir);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return StepMode::Manual;
    };
    // Tolerate other keys in state.toml -- only pull our field.
    let value: toml::Value = match toml::from_str(&body) {
        Ok(v) => v,
        Err(_) => return StepMode::Manual,
    };
    value
        .get("current_step_mode")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "auto" => Some(StepMode::Auto),
            "manual" => Some(StepMode::Manual),
            _ => None,
        })
        .unwrap_or(StepMode::Manual)
}

/// Write `current_step_mode = "<mode>"` into `state.toml`, preserving
/// every other key. Creates the file (and parent dir) if absent.
pub fn write_current_step_mode(project_dir: &Path, mode: StepMode) -> std::io::Result<()> {
    let path = state_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml::Value = if body.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&body)
            .map_err(|e| std::io::Error::other(format!("parse state.toml: {e}")))?
    };
    if let toml::Value::Table(table) = &mut doc {
        let val = match mode {
            StepMode::Auto => "auto",
            StepMode::Manual => "manual",
        };
        table.insert(
            "current_step_mode".into(),
            toml::Value::String(val.to_string()),
        );
    }
    let serialized = toml::to_string_pretty(&doc)
        .map_err(|e| std::io::Error::other(format!("serialize state.toml: {e}")))?;
    std::fs::write(&path, serialized)?;
    Ok(())
}

/// Sink for the mode-flip side-effects (StepModeChanged event +
/// Diagnostic::Info). The orchestrator passes a thunk that drives its
/// host/presenter; unit tests pass a counting fake.
pub trait FlipEventSink {
    fn step_mode_changed(&mut self, new_mode: StepMode);
    fn info(&mut self, message: &str);
}

/// No-op sink for the in-memory flip helpers used by the tool when the
/// orchestrator's emitter isn't wired in.
pub struct NoopSink;

impl FlipEventSink for NoopSink {
    fn step_mode_changed(&mut self, _new_mode: StepMode) {}
    fn info(&mut self, _message: &str) {}
}

/// Recording sink for unit tests.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub mode_changes: Vec<StepMode>,
    pub infos: Vec<String>,
}

impl FlipEventSink for RecordingSink {
    fn step_mode_changed(&mut self, new_mode: StepMode) {
        self.mode_changes.push(new_mode);
    }
    fn info(&mut self, message: &str) {
        self.infos.push(message.to_string());
    }
}

const FLIP_MESSAGE: &str = "ask_user invoked during auto run; flipping to manual mode. \
Re-enable auto via the chat panel toggle when ready.";

/// Idempotent step-mode flip. Reads current_step_mode from
/// `state.toml`, flips it to manual when auto, persists, and emits
/// the side effects via the supplied sink.
pub fn flip_step_mode_for_ask_user(
    project_dir: &Path,
    sink: &mut dyn FlipEventSink,
) -> std::io::Result<ModeFlipOutcome> {
    let current = read_current_step_mode(project_dir);
    match current {
        StepMode::Manual => Ok(ModeFlipOutcome::NoChange),
        StepMode::Auto => {
            write_current_step_mode(project_dir, StepMode::Manual)?;
            sink.step_mode_changed(StepMode::Manual);
            sink.info(FLIP_MESSAGE);
            Ok(ModeFlipOutcome::AutoToManual)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_from_auto_writes_manual_and_emits_events() {
        let tmp = tempfile::tempdir().unwrap();
        write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
        let mut sink = RecordingSink::default();
        let outcome = flip_step_mode_for_ask_user(tmp.path(), &mut sink).unwrap();
        assert_eq!(outcome, ModeFlipOutcome::AutoToManual);
        assert_eq!(read_current_step_mode(tmp.path()), StepMode::Manual);
        assert_eq!(sink.mode_changes, vec![StepMode::Manual]);
        assert_eq!(sink.infos.len(), 1);
        assert!(sink.infos[0].contains("flipping to manual"));
    }

    #[test]
    fn flip_from_manual_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        write_current_step_mode(tmp.path(), StepMode::Manual).unwrap();
        let mut sink = RecordingSink::default();
        let outcome = flip_step_mode_for_ask_user(tmp.path(), &mut sink).unwrap();
        assert_eq!(outcome, ModeFlipOutcome::NoChange);
        assert!(sink.mode_changes.is_empty());
        assert!(sink.infos.is_empty());
    }

    #[test]
    fn flip_when_no_state_file_treats_as_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = RecordingSink::default();
        let outcome = flip_step_mode_for_ask_user(tmp.path(), &mut sink).unwrap();
        assert_eq!(outcome, ModeFlipOutcome::NoChange);
    }

    #[test]
    fn flip_preserves_other_keys_in_state_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = state_path(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "current_step_mode = \"auto\"\nother_key = \"keep me\"\n",
        )
        .unwrap();
        let mut sink = RecordingSink::default();
        flip_step_mode_for_ask_user(tmp.path(), &mut sink).unwrap();
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("other_key"));
        assert!(body.contains("manual"));
    }

    #[test]
    fn reload_after_flip_sees_manual() {
        let tmp = tempfile::tempdir().unwrap();
        write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
        let mut sink = NoopSink;
        flip_step_mode_for_ask_user(tmp.path(), &mut sink).unwrap();
        assert_eq!(read_current_step_mode(tmp.path()), StepMode::Manual);
    }
}
