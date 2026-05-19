//! DM4 family: perf plan + perf walk.
//!
//! See `super` for the shared GateCheck helper set.

use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor};

use super::helpers::*;

/// DM4a (Performance Analysis Plan, OUTLINE) — produces the index
/// at `docs/perf-plan/perf-plan.md` plus stub
/// `perf-milestone-NN-*.md` files. Each stub names its workload
/// scope and traceability hooks; DM4ad fills in the per-milestone
/// task list. No simulations or report writing happen here.
pub(super) fn dm4a() -> StepDescriptor {
    StepDescriptor {
        id: "DM4a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3c"),
        instruction_slug: "dm4a-performance-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/perf-plan/perf-plan.md",
                "docs/perf-plan/perf-plan.md exists",
            ),
            file_matches(
                "docs/perf-plan/perf-plan.md",
                r"(?i)milestone\s+\d+",
                "docs/perf-plan/perf-plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "ls docs/perf-plan/perf-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/perf-plan/ contains at least one perf-milestone-NN-*.md stub",
            ),
            critique_clean("DM4a"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/perf-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/targets.md",
            "docs/targets/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/test-plan/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM4ad (Performance Analysis Plan, DETAIL) — walks each
/// `docs/perf-plan/perf-milestone-NN-*.md` stub and replaces its
/// `<!-- detail-pending` placeholder with the full task list per
/// `docs/plan-management.md`. Tasks reference workload
/// configs, target rows, and run-id schemes that DM4b later
/// executes.
pub(super) fn dm4ad() -> StepDescriptor {
    StepDescriptor {
        id: "DM4ad",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM4a"),
        instruction_slug: "dm4ad-performance-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/perf-plan/",
                &["perf-milestone-"],
                "<!-- detail-pending",
                "every docs/perf-plan/perf-milestone-NN-*.md stub has been detailed",
            ),
            critique_clean("DM4ad"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/perf-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/targets.md",
            "docs/targets/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/perf-plan/perf-plan.md",
            "docs/plan-management.md",
        ],
        work_write_paths: &["docs/perf-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/perf-plan/",
            file_prefixes: &["perf-milestone-"],
            index_file: "docs/perf-plan/perf-plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        }),
    }
}

/// DM4b (Performance Analysis) — executes the DM4a plan: runs
/// experiments + sweeps, identifies bottlenecks, verifies targets,
/// and writes per-topic reports under `docs/analysis/`. Depends on
/// experiment tracking (Phase 4); the critique surfaces a BLOCKER
/// if `.sim-flow/experiments.db` is unreachable.
pub(super) fn dm4b() -> StepDescriptor {
    StepDescriptor {
        id: "DM4b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM4ad"),
        instruction_slug: "dm4b-performance-analysis",
        per_candidate: false,
        gate_checks: vec![
            GateCheck::ExperimentsRecorded {
                description: "experiments.db has at least one recorded run".to_string(),
            },
            shell(
                "sh",
                &["-c", "ls docs/analysis/*.md >/dev/null 2>&1"],
                "docs/analysis/ contains at least one report",
            ),
            shell(
                "sh",
                &["-c", "grep -rqiE 'throughput|latency' docs/analysis/"],
                "analysis report covers throughput and latency metrics",
            ),
            file_exists(
                "docs/perf-plan/perf-plan.md",
                "docs/perf-plan/perf-plan.md still present",
            ),
            // DM4b mostly writes markdown reports, but any Rust
            // helpers / sweep glue / scratch binaries it lands
            // must still meet the same fmt + clippy bar as
            // earlier code-generating steps. `cargo build` is
            // already implied by `cargo clippy` so we skip a
            // separate build check here.
            shell(
                "cargo",
                &["fmt", "--all"],
                "cargo fmt --all succeeds (auto-formats)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            // DM4b's gate forbids `- [-]` deferrals on its own
            // perf-plan. The impl-plan and test-plan dirs are
            // already enforced clean by DM2d's and DM3c's gates,
            // so the agent can't have introduced new deferrals
            // into them here.
            milestones_all_implemented(
                "docs/perf-plan/",
                "perf-milestone-",
                "every docs/perf-plan/perf-milestone-NN-*.md row implemented (no deferrals at gate exit)",
            ),
            critique_clean("DM4b"),
        ],
        // Per-milestone gate: cheap quality checks. Reserves
        // `ExperimentsRecorded`, the `docs/analysis/` report checks,
        // and `milestones_all_implemented` for the step gate.
        walk_gate_checks: vec![
            shell(
                "cargo",
                &["fmt", "--all"],
                "cargo fmt --all succeeds (auto-formats)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            critique_clean("DM4b"),
        ],
        work_artifacts: &["docs/analysis/"],
        predecessor_inputs: &[
            "docs/targets.md",
            "docs/targets/",
            "docs/perf-plan/perf-plan.md",
            ".sim-flow/experiments.db",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        // DM4b walks `docs/perf-plan/perf-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/perf-plan/",
            file_prefixes: &["perf-milestone-"],
            index_file: "docs/perf-plan/perf-plan.md",
            placeholder_marker: None,
            // DM4b's gate forbids `- [-]`; the walker must agree.
            forbid_deferred: true,
        }),
    }
}
