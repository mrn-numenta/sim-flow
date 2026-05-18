//! DM0 work + critique prompt loaders.
//!
//! Phase 6 Stream C owns the prompt rewrites at
//! `prompts/dm0-specification.md` and
//! `prompts/dm0-specification-critique.md`. This module exposes
//! load helpers so the orchestrator's existing `prompts.rs` can
//! call into DM0-specific assembly (e.g. inlining the
//! auto-populate report into the work prompt) without growing the
//! generic loader.

use crate::Result;

#[allow(dead_code)]
pub fn load_work_prompt() -> Result<String> {
    todo!("Phase 6 milestone 6.7 — load rewritten dm0-specification.md")
}

#[allow(dead_code)]
pub fn load_critique_prompt() -> Result<String> {
    todo!("Phase 6 milestone 6.10 — load rewritten dm0-specification-critique.md")
}
