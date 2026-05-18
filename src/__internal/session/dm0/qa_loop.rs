//! No-source DM0 Q&A loop, built on top of the `ask_user` tool.
//!
//! When [`super::detect_mode`] returns
//! [`super::Dm0Mode::NoSource`], the agent's LLM turn drives this
//! loop: it iterates over `SpecMd::missing_required_fields()`, opens
//! an `ask_user` thread per field, validates the reply against the
//! field's `kind`, and closes the thread with `record_as =
//! "auto-decision"` (or chains a clarification on ambiguous input,
//! or records a TBD on cancellation).
//!
//! The loop does NOT implement its own user-prompting machinery;
//! every question goes through the `ask_user` suspend/resume
//! protocol the orchestrator's dispatch loop already understands
//! via `ToolResult::suspend`. The orchestrator is responsible for
//! the `RequestUserInput` event + thread-chaining; this module just
//! sequences the field walk and the validation logic. Owned by
//! Phase 6 Stream B.

use crate::__internal::session::llm_adapter::LlmAdapter;
use crate::__internal::session::spec_md::SpecMd;
use crate::Result;

/// Drive the Q&A loop for every MissingField in `spec`. Each
/// iteration opens an `ask_user` thread, validates the user's reply,
/// either closes the thread with the resolved value or chains a
/// clarification, and advances. Returns when
/// `spec.missing_required_fields()` is empty or every remaining
/// field has been cancelled (and recorded as a TBD).
///
/// The function takes `&mut dyn LlmAdapter` because the
/// normalization passes for free-form sections (e.g. Worked
/// Examples) and for ambiguity detection on user replies require an
/// LLM call. Reply validation that is purely syntactic (regex /
/// yes-no / choice) runs locally.
#[allow(dead_code)]
pub fn drive_qa_loop(_spec: &mut SpecMd, _llm: &mut dyn LlmAdapter) -> Result<()> {
    todo!("Phase 6 milestone 6.8 — iterate MissingFields and chain ask_user threads")
}

/// SectionApplicability fast-path: for OPTIONAL sections, ask the
/// user via an `ask_user` call with `kind = "choice"` and branch on
/// the reply. Called from [`drive_qa_loop`] before drilling into a
/// section's MissingFields.
#[allow(dead_code)]
pub fn ask_section_applicability(
    _section: &str,
    _spec: &mut SpecMd,
    _llm: &mut dyn LlmAdapter,
) -> Result<SectionApplicability> {
    todo!("Phase 6 milestone 6.9 — yes/no/skip applicability prompt for optional sections")
}

/// Result of [`ask_section_applicability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionApplicability {
    Applicable,
    NotApplicable,
    Deferred,
}
