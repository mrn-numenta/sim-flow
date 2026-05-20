//! DM0 work + critique prompt loaders.
//!
//! Phase 6 Stream C owns the prompt rewrites at
//! `prompts/dm0-specification.md` and
//! `prompts/dm0-specification-critique.md`. This module exposes
//! load helpers so the orchestrator's existing `prompts.rs` can
//! call into DM0-specific assembly (e.g. inlining the
//! auto-populate report into the work prompt) without growing the
//! generic loader.
//!
//! The bodies of the helpers are thin wrappers over
//! [`crate::prompts::load`] keyed on the DM0 slug
//! (`dm0-specification`). The orchestrator's per-session render
//! pipeline still substitutes the shared `{{ output_intro }}` /
//! `{{ critique_kinds }}` / `{{ critique_output_block }}` /
//! `{{ third_party_reviewer_note }}` placeholders later, so this
//! loader returns the raw body unchanged.

use std::path::Path;

use crate::__internal::client::SessionKind;
use crate::__internal::prompts;
use crate::Result;

/// Slug shared by both DM0 prompt files
/// (`dm0-specification.md` and `dm0-specification-critique.md`).
const DM0_SLUG: &str = "dm0-specification";

/// Load the DM0 work prompt body from disk, resolving overrides in
/// project / global scope before falling back to the foundation default.
/// Returns the raw markdown; `{{ key }}` placeholders are substituted
/// by the orchestrator's per-session render pass.
#[allow(dead_code)]
pub fn load_work_prompt(foundation_root: &Path, project_dir: &Path) -> Result<String> {
    prompts::load_for_project(foundation_root, project_dir, DM0_SLUG, SessionKind::Work)
}

/// Load the DM0 critique prompt body from disk. Mirrors
/// [`load_work_prompt`] for the critique session.
#[allow(dead_code)]
pub fn load_critique_prompt(foundation_root: &Path, project_dir: &Path) -> Result<String> {
    prompts::load_for_project(
        foundation_root,
        project_dir,
        DM0_SLUG,
        SessionKind::Critique,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        // sim-flow is now its own repo; CARGO_MANIFEST_DIR is the
        // crate (and asset) root.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn loads_new_work_prompt() {
        let foundation = workspace_root();
        let project = tempfile::tempdir().unwrap();
        let body = load_work_prompt(&foundation, project.path()).expect("work prompt loads");
        // The new prompt teaches the structured-template vocabulary and
        // references the new retrieval tools by name. These tokens
        // distinguish the new prompt from the legacy one.
        assert!(
            body.contains("structured spec.md schema"),
            "missing structured-schema vocabulary in body:\n{body}"
        );
        assert!(
            body.contains("spec_semantic_search"),
            "missing spec_semantic_search nudge in body:\n{body}"
        );
        assert!(
            body.contains("signal_table_query"),
            "missing signal_table_query nudge in body:\n{body}"
        );
        assert!(
            body.contains("ask_user"),
            "missing ask_user nudge in body:\n{body}"
        );
        assert!(
            body.contains("LAST tool call"),
            "missing turn-boundary nudge in body:\n{body}"
        );
    }

    #[test]
    fn loads_new_critique_prompt() {
        let foundation = workspace_root();
        let project = tempfile::tempdir().unwrap();
        let body =
            load_critique_prompt(&foundation, project.path()).expect("critique prompt loads");
        assert!(
            body.contains("structured Chapter 2 schema"),
            "missing schema-reference vocabulary in body:\n{body}"
        );
        assert!(
            body.contains("spec_semantic_search"),
            "missing spec_semantic_search nudge in body:\n{body}"
        );
        assert!(
            body.contains("signal_table_query"),
            "missing signal_table_query directive in body:\n{body}"
        );
    }

    #[test]
    fn legacy_prompts_still_on_disk_for_reference() {
        // Stream C moves the prior DM0 prompts under `prompts/legacy/`
        // so the rewrite can be diffed against the original. The
        // foundation-default scope ignores subdirectories, so the
        // legacy files never resolve into a live session.
        let foundation = workspace_root();
        let work = foundation.join("prompts/legacy/dm0-specification.md");
        let critique = foundation.join("prompts/legacy/dm0-specification-critique.md");
        assert!(
            work.exists(),
            "legacy work prompt missing: {}",
            work.display()
        );
        assert!(
            critique.exists(),
            "legacy critique prompt missing: {}",
            critique.display()
        );
    }
}
