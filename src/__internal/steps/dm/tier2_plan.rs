//! DM2 planning steps (analysis + impl-plan outline + detail).
//!
//! See `super` for the shared GateCheck helper set.

use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor};

use super::helpers::*;

pub(super) fn dm2a() -> StepDescriptor {
    StepDescriptor {
        id: "DM2a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM1"),
        instruction_slug: "dm2a-decomposition",
        per_candidate: false,
        gate_checks: vec![
            // decomposition supports dual layout: single-file
            // `docs/analysis/decomposition.md` OR paginated
            // `docs/analysis/decomposition/<NN>-<slug>.md` section
            // files. data-movement stays single-file (typically a
            // single table even for large designs).
            any_exists(
                &[
                    "docs/analysis/decomposition.md",
                    "docs/analysis/decomposition/",
                ],
                "decomposition.md or decomposition/ exists and is non-empty",
            ),
            any_matches(
                &[
                    "docs/analysis/decomposition.md",
                    "docs/analysis/decomposition/",
                ],
                r"(?m)^##\s*Operation:\s*\S+",
                "decomposition declares at least one ## Operation: <name> heading",
            ),
            file_exists(
                "docs/analysis/data-movement.md",
                "docs/analysis/data-movement.md exists",
            ),
            critique_clean("DM2a"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &[
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
        ],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/targets.md",
            "docs/targets/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

pub(super) fn dm2b() -> StepDescriptor {
    StepDescriptor {
        id: "DM2b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2a"),
        instruction_slug: "dm2b-pipeline-mapping",
        per_candidate: false,
        gate_checks: vec![
            // pipeline-mapping supports dual layout: single-file
            // `docs/analysis/pipeline-mapping.md` OR paginated
            // `docs/analysis/pipeline-mapping/<NN>-<slug>.md`.
            any_exists(
                &[
                    "docs/analysis/pipeline-mapping.md",
                    "docs/analysis/pipeline-mapping/",
                ],
                "pipeline-mapping.md or pipeline-mapping/ exists and is non-empty",
            ),
            any_matches(
                &[
                    "docs/analysis/pipeline-mapping.md",
                    "docs/analysis/pipeline-mapping/",
                ],
                r"(?i)stage",
                "pipeline-mapping mentions pipeline stages",
            ),
            critique_clean("DM2b"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &[
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
        ],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM2c (Implementation Plan, OUTLINE) — produces the plan index
/// at `docs/impl-plan/plan.md` plus a milestone-NN-*.md STUB per
/// milestone. Each stub carries its scope, predecessor / dependency
/// notes, and a `<!-- detail-pending` placeholder that DM2cd
/// later replaces with the full task list. Splitting outline from
/// detail bounds each session's context: a large design (e.g.
/// RISC-V core with 25+ milestones) can name its milestones in one
/// session even when the per-milestone task lists are too large to
/// fit alongside the spec + decomposition + targets in one prompt.
pub(super) fn dm2c() -> StepDescriptor {
    StepDescriptor {
        id: "DM2c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2b"),
        instruction_slug: "dm2c-model-impl-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/impl-plan/plan.md", "docs/impl-plan/plan.md exists"),
            file_matches(
                "docs/impl-plan/plan.md",
                r"(?i)milestone\s+\d+",
                "docs/impl-plan/plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &["-c", "ls docs/impl-plan/milestone-*.md >/dev/null 2>&1"],
                "docs/impl-plan/ contains at least one milestone-NN-*.md stub",
            ),
            critique_clean("DM2c"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/impl-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM2cd (Implementation Plan, DETAIL) — walks each
/// `docs/impl-plan/milestone-NN-*.md` stub and replaces the
/// `<!-- detail-pending` placeholder with the full task list per
/// `docs/plan-management.md`'s task format. One milestone
/// per work + critique session, so the per-milestone task list gets
/// a focused review and a critique can flag e.g. "milestone 03
/// task list is too coarse" without blocking the rest of the plan.
pub(super) fn dm2cd() -> StepDescriptor {
    StepDescriptor {
        id: "DM2cd",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2c"),
        instruction_slug: "dm2cd-impl-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/impl-plan/",
                &["milestone-"],
                "<!-- detail-pending",
                "every docs/impl-plan/milestone-NN-*.md stub has been detailed",
            ),
            critique_clean("DM2cd"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/impl-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/impl-plan/plan.md",
            "docs/plan-management.md",
        ],
        work_write_paths: &["docs/impl-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        }),
    }
}
