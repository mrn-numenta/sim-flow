//! Session orchestrator: drives a single Work or Critique session
//! from Hello through SessionEnd.
//!
//! The orchestrator was originally one 5.4k-line file; it's split
//! here by concern so each submodule fits in a single editor pane:
//!
//! - `options` -- `OrchestratorOptions` + shared constants and the
//!   `unix_seconds_now` clock helper.
//! - `dispatch` -- `run_session` + the `run_session_inner` turn
//!   loop. Owns the per-turn state machine, control-socket cancel
//!   handling, runaway-guard caps, and the wind-down branches.
//! - `messages` -- initial prompt + per-session input assembly
//!   (`MessageBundle`, `build_initial_messages`,
//!   `step_descriptor_for_protocol`, `SessionInputs`, the TOC
//!   helpers).
//! - `tools_dispatch` -- tool invocation, argument parsing,
//!   per-call rendering, `resolve_native_tool_mode`,
//!   `run_phase_validator`, and the `base64_encode` shim.
//! - `gates` -- structural-gate evaluation,
//!   `salvage_critique_json`, the markdown finding parsers, and
//!   `retry_gate_finding_blocks`.
//! - `artifacts` -- fenced ``` <path>``` block extraction +
//!   `write_artifact`, plus the library / framework / framework-
//!   docs root detectors.
//! - `progress` -- `ProgressClass` + `classify_progress` + the
//!   stuck-loop response-hash normalizer.
//!
//! External callers see the same public surface they did before
//! the split: `OrchestratorOptions`, `run_session`, `MessageBundle`,
//! `build_initial_messages`, `step_descriptor_for_protocol`. Those
//! are re-exported here so existing import paths keep working.

mod artifacts;
mod dispatch;
mod gates;
mod messages;
mod options;
mod progress;
mod tools_dispatch;

pub use artifacts::ExtractedArtifact;
pub use dispatch::run_session;
pub use messages::{MessageBundle, build_initial_messages};
pub use options::OrchestratorOptions;

pub(crate) use messages::step_descriptor_for_protocol;
