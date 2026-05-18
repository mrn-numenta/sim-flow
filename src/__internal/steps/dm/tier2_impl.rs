//! DM2d model-implementation walk step.
//!
//! See `super` for the shared GateCheck helper set.

use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor};

use super::helpers::*;

/// DM2d (Model Implementation) — executes the DM2c plan to land
/// `src/model/`, tests, a working `cargo build` + `cargo test`. The
/// gates here are the heavyweight code checks that used to live on
/// DM2c (Cargo, build, elaboration smoke test, ConnectivityPlan and
/// HasLogic grep) so the split is purely an internal partition --
/// downstream steps still rely on the same artifacts being present
/// once DM2d's gate passes.
pub(super) fn dm2d() -> StepDescriptor {
    StepDescriptor {
        id: "DM2d",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2cd"),
        instruction_slug: "dm2d-model-implementation",
        per_candidate: false,
        gate_checks: vec![
            file_exists("Cargo.toml", "Cargo.toml exists"),
            file_matches(
                "Cargo.toml",
                "foundation-framework",
                "Cargo.toml depends on foundation-framework",
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
            // Run the full test suite (unit + every `tests/*.rs`
            // integration target). The prompt asks the LLM to keep
            // smoke tests in `tests/elaboration.rs`, but models
            // periodically rename or split the file (`tests/smoke_tests.rs`,
            // `tests/elab.rs`, ...). Pinning the gate to one specific
            // test target name then fails with `no test target named
            // elaboration` even when functionally-equivalent tests
            // pass. `cargo test --quiet` is strictly stronger and
            // doesn't care about the file layout.
            shell(
                "cargo",
                &["test", "--quiet"],
                "cargo test --quiet passes (smoke + unit tests)",
            ),
            // Connectivity style: the prompt instructs inline
            // `HasInstances::instances()` + `connect()` (using
            // `InstanceBuilder` / `NetBuilder`), not the
            // `ConnectivityPlanBuilder` recipe path. Match the
            // trait impl directly so either an `impl HasInstances
            // for Top` block (the typical hand-rolled form) or a
            // derive macro that emits one satisfies the gate.
            shell(
                "grep",
                &[
                    "-rE",
                    "--include=*.rs",
                    "-q",
                    "impl[[:space:]]+HasInstances",
                    "src",
                ],
                "src/ implements HasInstances (parent module's connectivity)",
            ),
            shell(
                "grep",
                &["-r", "--include=*.rs", "-q", "impl HasLogic", "src"],
                "src/ implements HasLogic",
            ),
            // Block-diagram contract. The orchestrator auto-renders
            // the block diagram on the DM2d -> DM3a advance via
            // `crate::dump_topology(&args)`. The two greps below
            // pin the contract DM2d must keep intact: the dump
            // helper in lib.rs and the stub `Top` (or its
            // post-DM2d replacement) under model/.
            file_matches(
                "src/lib.rs",
                r"pub\s+fn\s+dump_topology",
                "src/lib.rs exports dump_topology(&TopologyDumpArgs) (template contract)",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "grep -q 'pub struct Top' src/model/top.rs && grep -q 'impl Module for Top' src/model/top.rs",
                ],
                "src/model/top.rs defines `pub struct Top` with `impl Module for Top` (block-diagram contract)",
            ),
            // DM2d's gate forbids `- [-]` deferrals. Defers ARE
            // legitimate during the step (one milestone task can
            // wait on a sibling milestone landing first), but by
            // the time the gate evaluates, every deferred row must
            // have been re-opened and resolved as `- [x]`.
            // Otherwise a deferred-and-forgotten implementation
            // task could leak into DM3 / DM4 where there's no
            // longer a chance to fix it without resetting.
            milestones_all_implemented(
                "docs/impl-plan/",
                "milestone-",
                "every docs/impl-plan/milestone-NN-*.md row implemented (no deferrals at gate exit)",
            ),
            critique_clean("DM2d"),
        ],
        // Per-milestone gate: the cheap quality checks that should
        // hold after every milestone lands. Reserves the expensive
        // integration checks (`cargo test --quiet`, the
        // HasInstances / HasLogic / dump_topology / Top struct
        // greps, `milestones_all_implemented`) for the step gate
        // above -- those only become satisfiable once the last
        // milestone closes, and running them per-walk-turn just
        // burns cargo time.
        walk_gate_checks: vec![
            file_exists("Cargo.toml", "Cargo.toml exists"),
            file_matches(
                "Cargo.toml",
                "foundation-framework",
                "Cargo.toml depends on foundation-framework",
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
            critique_clean("DM2d"),
        ],
        work_artifacts: &["src/", "tests/", "Cargo.toml"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/decomposition/",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/pipeline-mapping/",
            "docs/impl-plan/plan.md",
        ],
        // `docs/impl-plan/` included so the agent can tick `[ ]` -> `[x]`
        // on completed plan tasks. The DM2d critique gates already
        // catch out-of-scope plan rewrites (gate 11: "introduced
        // major architectural structures or boundaries not reflected
        // in DM2c's plan"), so the safety net for misuse exists.
        work_write_paths: &["src/", "tests/", "Cargo.toml", "docs/impl-plan/"],
        work_phases: &["author", "build", "test"],
        critique_phases: &["chat"],
        // DM2d walks `docs/impl-plan/milestone-NN-*.md` files one
        // at a time. The orchestrator picks the current milestone
        // (first file with `- [ ]` rows) and scopes both work +
        // critique sessions to it; the auto-driver iterates
        // work-then-critique until every milestone is resolved.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: None,
            // DM2d's gate forbids `- [-]`; the walker must agree
            // so milestones with deferred-only rows keep being
            // targeted by Work + Critique until they're `[x]`.
            forbid_deferred: true,
        }),
    }
}
