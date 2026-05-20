//! DM3 family: test plan + testbench + tests.
//!
//! See `super` for the shared GateCheck helper set.

use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor};

use super::helpers::*;

/// DM3a (Test Plan) — produces the formal verification plan as a
/// directory at `docs/test-plan/`. Top-level `test-plan.md` is the
/// index (testbench architecture + traceability table back to spec
/// / targets / decomposition); two parallel milestone sequences
/// (`tb-milestone-NN-*.md` for DM3b's testbench-impl slices and
/// `test-milestone-NN-*.md` for DM3c's test-execution slices); and
/// `coverage.md` for the `cargo-llvm-cov` strategy. No test code
/// is written here; that's DM3b (scaffolding) and DM3c (test
/// cases). The milestone structure mirrors `docs/impl-plan/` and
/// `docs/perf-plan/` so DM3b/DM3c walk small reviewable chunks
/// with a critique after each milestone (10-task cap per
/// `plan-management.md`).
pub(super) fn dm3a() -> StepDescriptor {
    StepDescriptor {
        id: "DM3a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2d"),
        instruction_slug: "dm3a-test-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/test-plan/test-plan.md",
                "docs/test-plan/test-plan.md exists (index file)",
            ),
            file_exists(
                "docs/test-plan/coverage.md",
                "docs/test-plan/coverage.md exists",
            ),
            shell(
                "sh",
                &["-c", "ls docs/test-plan/tb-milestone-*.md >/dev/null 2>&1"],
                "docs/test-plan/ contains at least one tb-milestone-NN-*.md stub (DM3b's slices)",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "ls docs/test-plan/test-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/test-plan/ contains at least one test-milestone-NN-*.md stub (DM3c's slices)",
            ),
            file_matches(
                "docs/test-plan/test-plan.md",
                r"(?i)Sequencer|Driver|Monitor|Scoreboard",
                "test-plan.md index describes UVM-lite testbench components",
            ),
            file_matches(
                "docs/test-plan/test-plan.md",
                r"spec\.md|targets\.md",
                "test-plan.md index traces entries back to docs/spec.md or docs/targets.md",
            ),
            file_matches(
                "docs/test-plan/coverage.md",
                r"(?i)llvm-cov",
                "docs/test-plan/coverage.md describes coverage via cargo-llvm-cov",
            ),
            critique_clean("DM3a"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/test-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/targets.md",
            "docs/targets/",
            "docs/testbench.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/analysis/data-movement.md",
            "docs/impl-plan/plan.md",
            "src/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM3ad (Test Plan, DETAIL) — walks each tb-milestone-NN-*.md and
/// test-milestone-NN-*.md stub left by DM3a and replaces the
/// `<!-- detail-pending` placeholder with the full task list.
/// Both file types live in `docs/test-plan/`; the milestone-walk
/// machinery walks lexicographically across both prefixes so a
/// large project's tb-* tasks land before the test-* tasks that
/// depend on them. One milestone per session = focused critiques.
pub(super) fn dm3ad() -> StepDescriptor {
    StepDescriptor {
        id: "DM3ad",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3a"),
        instruction_slug: "dm3ad-test-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/test-plan/",
                &["tb-milestone-", "test-milestone-"],
                "<!-- detail-pending",
                "every tb-milestone-NN-*.md and test-milestone-NN-*.md stub has been detailed",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "grep -lE '^-[[:space:]]+\\[[ x]\\][[:space:]]' docs/test-plan/tb-milestone-*.md docs/test-plan/test-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/test-plan/ milestone files contain markdown checklist entries",
            ),
            critique_dir_clean("DM3ad"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/test-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/targets.md",
            "docs/targets/",
            "docs/testbench.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/test-plan/test-plan.md",
            "docs/test-plan/coverage.md",
            "docs/plan-management.md",
        ],
        work_write_paths: &["docs/test-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-", "test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        }),
    }
}

/// DM3b (Testbench Implementation) — implements the UVM-lite
/// testbench scaffolding (Sequencers, Drivers, Monitors,
/// Scoreboards, `SimEnvBuilder` wiring) named in the test plan,
/// plus the basic data-flow smoke test. Edge / stress / random
/// tests are DM3c's responsibility.
pub(super) fn dm3b() -> StepDescriptor {
    StepDescriptor {
        id: "DM3b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3ad"),
        instruction_slug: "dm3b-testbench-impl",
        per_candidate: false,
        gate_checks: vec![
            shell(
                "grep",
                &[
                    "-r",
                    "--include=*.rs",
                    "-qE",
                    "SimEnv|Sequencer|Driver|Monitor|Scoreboard",
                    "tests",
                ],
                "tests/ references UVM-lite testbench components",
            ),
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
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            milestones_all_resolved(
                "docs/test-plan/",
                "tb-milestone-",
                "every docs/test-plan/tb-milestone-NN-*.md row resolved",
            ),
            critique_clean("DM3b"),
        ],
        // Per-milestone gate: cheap quality checks. Reserves the
        // cross-module `grep -r 'SimEnv|Sequencer|Driver|Monitor|Scoreboard'`
        // check + `milestones_all_resolved` for the step gate -- those
        // only become satisfiable once the testbench's component
        // milestones all land.
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
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            critique_clean("DM3b"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/testbench.md", "docs/test-plan/", "src/"],
        // `docs/test-plan/` included so the agent can flip
        // checkboxes on `tb-milestone-NN-*.md` files as tasks
        // complete. The DM3b critique catches out-of-scope plan
        // rewrites (silent restructuring of milestone files).
        work_write_paths: &["tests/", "src/", "docs/test-plan/"],
        work_phases: &["author", "build"],
        critique_phases: &["chat"],
        // DM3b walks `docs/test-plan/tb-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
            forbid_deferred: false,
        }),
    }
}

/// DM3c (Test Execution and Coverage) — implements every test in
/// the plan's smoke / edge / stress / random categories using
/// DM3b's testbench, runs the full suite, then runs
/// `cargo-llvm-cov` to verify the coverage threshold the plan
/// declared.
pub(super) fn dm3c() -> StepDescriptor {
    StepDescriptor {
        id: "DM3c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3b"),
        instruction_slug: "dm3c-test-execution",
        per_candidate: false,
        gate_checks: vec![
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
            shell(
                "cargo",
                &["test", "--quiet"],
                "full cargo test suite passes",
            ),
            file_exists(
                "docs/test-plan/test-plan.md",
                "docs/test-plan/test-plan.md still present",
            ),
            // Coverage threshold is validated by the critique (the
            // AI inspects the cargo-llvm-cov report referenced from
            // the plan and flags sub-threshold runs). A structural
            // gate that runs llvm-cov from this process would
            // double the test time; rely on the agent reporting the
            // measured percentage in the plan's `## Coverage`
            // section.
            // DM3c's gate forbids `- [-]` deferrals. Same
            // semantics as DM2d: defers are valid intra-step but
            // must be resolved before this gate passes; otherwise
            // a deferred test escapes into DM4 with no way to
            // rerun without resetting.
            milestones_all_implemented(
                "docs/test-plan/",
                "test-milestone-",
                "every docs/test-plan/test-milestone-NN-*.md row implemented (no deferrals at gate exit)",
            ),
            critique_clean("DM3c"),
        ],
        // Per-milestone gate: cheap quality checks. Reserves the
        // expensive `cargo test --quiet` (full suite) +
        // `milestones_all_implemented` + `file_exists test-plan.md`
        // for the step gate -- the full suite only becomes
        // satisfiable once every test milestone lands its cases.
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
            critique_clean("DM3c"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/testbench.md", "docs/test-plan/", "tests/", "src/"],
        // `docs/test-plan/` included so the agent can flip
        // checkboxes on `test-milestone-NN-*.md` rows and append
        // measured-coverage info to `test-plan.md`'s `## Coverage`
        // section. The DM3c critique catches structural plan
        // rewrites.
        work_write_paths: &["tests/", "src/", "docs/test-plan/"],
        work_phases: &["author", "test"],
        critique_phases: &["chat"],
        // DM3c walks `docs/test-plan/test-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
            // DM3c's gate forbids `- [-]`; the walker must agree.
            forbid_deferred: true,
        }),
    }
}
