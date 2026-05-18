//! `ask_user` orchestrator-side machinery (Architecture Ch.4 §4.5
//! and Ch.6 §6.5).
//!
//! The tool itself is a thin wrapper that pushes a `PendingUserAsk`
//! into this module's state, suspends the LLM turn, and lets the
//! orchestrator's existing `RequestUserInput` + `UserMessage` event
//! loop drive the actual user round-trip. On the user's reply the
//! orchestrator builds an `AskUserAnswer` from the pending state
//! and threads it back into the next turn's tool-result stream.
//!
//! Submodules:
//!
//! - [`pending`] holds the `PendingUserAsk` / `AskUserAnswer`
//!   structs plus the on-disk pending-ask checkpoint persistence.
//! - [`threads`] holds the `ThreadRegistry`, multi-turn chaining,
//!   force-close-on-sub-session-end, and per-thread persistence.
//! - [`mode_flip`] holds the auto→manual flip helper (milestone
//!   5.6).
//! - [`persist`] holds the spec.md / qa-buffer writers (milestone
//!   5.8).
//! - [`runtime`] holds the in-memory `AskUserRuntime` that bundles
//!   the pending-ask slot with the thread registry. The orchestrator
//!   owns one instance per sub-session; the tool talks to it through
//!   the `&AskUserRuntime` handle.

pub mod mode_flip;
pub mod pending;
pub mod persist;
pub mod runtime;
pub mod threads;

pub use mode_flip::{ModeFlipOutcome, flip_step_mode_for_ask_user};
pub use pending::{AskUserAnswer, AskUserKind, PendingUserAsk, RecordAs};
pub use persist::{ResolvedThreadRecord, persist_resolved_thread};
pub use runtime::{AskUserRuntime, SuspendOutcome};
pub use threads::{ClosedAs, ResolvedThread, ThreadHandle, ThreadRegistry, ThreadTurn};
