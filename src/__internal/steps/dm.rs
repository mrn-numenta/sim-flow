//! DM step registration with detailed gate checks per
//! `docs/architecture/ai-flow/02-direct-modeling-flow.md`.
//!
//! Gate checks are structural (file exists, regex matches, shell command
//! succeeds, critique scan). Semantic cross-file checks (e.g. every
//! operation name from `decomposition.md` appears in `pipeline-mapping.md`)
//! live in the corresponding critique prompt, not here.

use std::path::PathBuf;

use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::{StepDescriptor, StepRegistry};

pub fn register(reg: &mut StepRegistry) {
    reg.register(dm0());
    reg.register(dm1());
    reg.register(dm2a());
    reg.register(dm2b());
    reg.register(dm2c());
    reg.register(dm2d());
    reg.register(dm3a());
    reg.register(dm3b());
    reg.register(dm3c());
    reg.register(dm4a());
    reg.register(dm4b());
}

fn critique_clean(step: &str) -> GateCheck {
    GateCheck::CritiqueClean {
        path: PathBuf::from(format!("docs/critiques/{step}-critique.md")),
        description: format!("{step} critique has no blockers"),
    }
}

fn file_exists(path: &str, description: &str) -> GateCheck {
    GateCheck::FileExists {
        path: PathBuf::from(path),
        description: description.to_string(),
    }
}

fn file_matches(path: &str, pattern: &str, description: &str) -> GateCheck {
    GateCheck::FileMatches {
        path: PathBuf::from(path),
        pattern: pattern.to_string(),
        description: description.to_string(),
    }
}

fn shell(cmd: &str, args: &[&str], description: &str) -> GateCheck {
    GateCheck::Shell {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        description: description.to_string(),
    }
}

fn dm0() -> StepDescriptor {
    StepDescriptor {
        id: "DM0",
        flow: Flow::DirectModeling,
        prerequisite: None,
        instruction_slug: "dm0-specification",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/spec.md", "docs/spec.md exists and is non-empty"),
            file_matches(
                "docs/spec.md",
                r"\d+\s*(MHz|GHz)",
                "docs/spec.md declares a clock frequency",
            ),
            file_matches(
                "docs/spec.md",
                r"\d+\s*nm",
                "docs/spec.md declares a technology node",
            ),
            critique_clean("DM0"),
        ],
        work_artifacts: &["docs/spec.md"],
        predecessor_inputs: &[],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

fn dm1() -> StepDescriptor {
    StepDescriptor {
        id: "DM1",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM0"),
        instruction_slug: "dm1-modeling-setup",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/targets.md", "docs/targets.md exists"),
            file_matches(
                "docs/targets.md",
                r"(?i)\d+\s*(cycles?|ns|MHz|GHz|items|bits|gates)",
                "docs/targets.md contains at least one quantitative target",
            ),
            file_exists("docs/testbench.md", "docs/testbench.md exists"),
            file_matches(
                "docs/testbench.md",
                r"(Sequencer|Driver|Monitor|Scoreboard)",
                "docs/testbench.md names at least one UVM-lite component",
            ),
            critique_clean("DM1"),
        ],
        work_artifacts: &["docs/targets.md", "docs/testbench.md"],
        predecessor_inputs: &["docs/spec.md"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

fn dm2a() -> StepDescriptor {
    StepDescriptor {
        id: "DM2a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM1"),
        instruction_slug: "dm2a-decomposition",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/analysis/decomposition.md",
                "docs/analysis/decomposition.md exists",
            ),
            file_matches(
                "docs/analysis/decomposition.md",
                r"(?m)^##\s*Operation:\s*\S+",
                "docs/analysis/decomposition.md declares at least one ## Operation: <name> heading",
            ),
            file_exists(
                "docs/analysis/data-movement.md",
                "docs/analysis/data-movement.md exists",
            ),
            critique_clean("DM2a"),
        ],
        work_artifacts: &[
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
        predecessor_inputs: &["docs/spec.md", "docs/targets.md"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

fn dm2b() -> StepDescriptor {
    StepDescriptor {
        id: "DM2b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2a"),
        instruction_slug: "dm2b-pipeline-mapping",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/analysis/pipeline-mapping.md",
                "docs/analysis/pipeline-mapping.md exists",
            ),
            file_matches(
                "docs/analysis/pipeline-mapping.md",
                r"(?i)stage",
                "docs/analysis/pipeline-mapping.md mentions pipeline stages",
            ),
            critique_clean("DM2b"),
        ],
        work_artifacts: &["docs/analysis/pipeline-mapping.md"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

/// DM2c (Implementation Plan) — produces the milestone-driven plan
/// at `docs/plan/`. No code is written here; that's DM2d's job.
/// The gate verifies that `plan.md` exists, references at least one
/// numbered milestone heading, and that there's at least one
/// `milestone-NN-<name>.md` file alongside it.
fn dm2c() -> StepDescriptor {
    StepDescriptor {
        id: "DM2c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2b"),
        instruction_slug: "dm2c-model-impl-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/plan/plan.md", "docs/plan/plan.md exists"),
            file_matches(
                "docs/plan/plan.md",
                r"(?i)milestone\s+\d+",
                "docs/plan/plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &["-c", "ls docs/plan/milestone-*.md >/dev/null 2>&1"],
                "docs/plan/ contains at least one milestone-NN-*.md file",
            ),
            critique_clean("DM2c"),
        ],
        work_artifacts: &["docs/plan/plan.md"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
        ],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

/// DM2d (Model Implementation) — executes the DM2c plan to land
/// `src/model/`, tests, a working `cargo build` + `cargo test`. The
/// gates here are the heavyweight code checks that used to live on
/// DM2c (Cargo, build, elaboration smoke test, ConnectivityPlan and
/// HasLogic grep) so the split is purely an internal partition --
/// downstream steps still rely on the same artifacts being present
/// once DM2d's gate passes.
fn dm2d() -> StepDescriptor {
    StepDescriptor {
        id: "DM2d",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2c"),
        instruction_slug: "dm2d-model-implementation",
        per_candidate: false,
        gate_checks: vec![
            file_exists("Cargo.toml", "Cargo.toml exists"),
            file_matches(
                "Cargo.toml",
                "foundation-framework",
                "Cargo.toml depends on foundation-framework",
            ),
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            shell(
                "cargo",
                &["test", "--quiet", "--test", "elaboration"],
                "elaboration smoke test passes",
            ),
            shell(
                "grep",
                &["-r", "--include=*.rs", "-q", "ConnectivityPlan", "src"],
                "src/ references ConnectivityPlan",
            ),
            shell(
                "grep",
                &["-r", "--include=*.rs", "-q", "impl HasLogic", "src"],
                "src/ implements HasLogic",
            ),
            critique_clean("DM2d"),
        ],
        work_artifacts: &["src/", "tests/", "Cargo.toml"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/plan/plan.md",
        ],
        work_phases: &["author", "build", "test"],
        critique_phases: &["chat"],
    }
}

/// DM3a (Test Plan) — produces the formal verification plan at
/// `docs/plan/test-plan.md`. The plan covers testbench design
/// (UVM-lite components + `SimEnvBuilder` wiring), four enumerated
/// test categories (smoke / edge / stress / random) with `[ ]`
/// checklists, a `cargo-tarpaulin` coverage strategy, and a
/// traceability table back to spec / targets / decomposition. No
/// test code is written here; that's DM3b (scaffolding) and DM3c
/// (test cases).
fn dm3a() -> StepDescriptor {
    StepDescriptor {
        id: "DM3a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2d"),
        instruction_slug: "dm3a-test-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/plan/test-plan.md", "docs/plan/test-plan.md exists"),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?i)Sequencer|Driver|Monitor|Scoreboard",
                "docs/plan/test-plan.md describes UVM-lite components",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?im)^##\s+Smoke",
                "docs/plan/test-plan.md has a Smoke section",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?im)^##\s+Edge",
                "docs/plan/test-plan.md has an Edge section",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?im)^##\s+Stress",
                "docs/plan/test-plan.md has a Stress section",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?im)^##\s+Random",
                "docs/plan/test-plan.md has a Random section",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?m)^-\s+\[\s?[x ]?\s?\]\s+",
                "docs/plan/test-plan.md has markdown checklist entries",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"(?i)tarpaulin",
                "docs/plan/test-plan.md describes coverage via cargo-tarpaulin",
            ),
            file_matches(
                "docs/plan/test-plan.md",
                r"spec\.md|targets\.md",
                "docs/plan/test-plan.md traces entries back to docs/spec.md or docs/targets.md",
            ),
            critique_clean("DM3a"),
        ],
        work_artifacts: &["docs/plan/test-plan.md"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/data-movement.md",
            "docs/plan/plan.md",
            "src/",
        ],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

/// DM3b (Testbench Implementation) — implements the UVM-lite
/// testbench scaffolding (Sequencers, Drivers, Monitors,
/// Scoreboards, `SimEnvBuilder` wiring) named in the test plan,
/// plus the basic data-flow smoke test. Edge / stress / random
/// tests are DM3c's responsibility.
fn dm3b() -> StepDescriptor {
    StepDescriptor {
        id: "DM3b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3a"),
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
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            critique_clean("DM3b"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/plan/test-plan.md", "src/"],
        work_phases: &["author", "build"],
        critique_phases: &["chat"],
    }
}

/// DM3c (Test Execution and Coverage) — implements every test in
/// the plan's smoke / edge / stress / random categories using
/// DM3b's testbench, runs the full suite, then runs
/// `cargo-tarpaulin` to verify the coverage threshold the plan
/// declared.
fn dm3c() -> StepDescriptor {
    StepDescriptor {
        id: "DM3c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3b"),
        instruction_slug: "dm3c-test-execution",
        per_candidate: false,
        gate_checks: vec![
            shell(
                "cargo",
                &["test", "--quiet"],
                "full cargo test suite passes",
            ),
            file_exists(
                "docs/plan/test-plan.md",
                "docs/plan/test-plan.md still present",
            ),
            // Coverage threshold is validated by the critique (the
            // AI inspects the cargo-tarpaulin report referenced from
            // the plan and flags sub-threshold runs). A structural
            // gate that runs tarpaulin from this process would
            // double the test time; rely on the agent reporting the
            // measured percentage in the plan's `## Coverage`
            // section.
            critique_clean("DM3c"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/plan/test-plan.md", "tests/", "src/"],
        work_phases: &["author", "test"],
        critique_phases: &["chat"],
    }
}

/// DM4a (Performance Analysis Plan) — produces a milestone-driven
/// plan at `docs/plan/perf-plan.md` + per-milestone files. The
/// plan covers baseline measurement, parameter sweeps, bottleneck
/// analysis, target verification, and reporting -- and tells DM4b
/// to stop for user review at each milestone boundary. No
/// simulations or report writing happen here.
fn dm4a() -> StepDescriptor {
    StepDescriptor {
        id: "DM4a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3c"),
        instruction_slug: "dm4a-performance-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/plan/perf-plan.md", "docs/plan/perf-plan.md exists"),
            file_matches(
                "docs/plan/perf-plan.md",
                r"(?i)milestone\s+\d+",
                "docs/plan/perf-plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &["-c", "ls docs/plan/perf-milestone-*.md >/dev/null 2>&1"],
                "docs/plan/ contains at least one perf-milestone-NN-*.md file",
            ),
            critique_clean("DM4a"),
        ],
        work_artifacts: &["docs/plan/perf-plan.md"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/plan/test-plan.md",
        ],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

/// DM4b (Performance Analysis) — executes the DM4a plan: runs
/// experiments + sweeps, identifies bottlenecks, verifies targets,
/// and writes per-topic reports under `docs/analysis/`. Depends on
/// experiment tracking (Phase 4); the critique surfaces a BLOCKER
/// if `.sim-flow/experiments.db` is unreachable.
fn dm4b() -> StepDescriptor {
    StepDescriptor {
        id: "DM4b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM4a"),
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
                "docs/plan/perf-plan.md",
                "docs/plan/perf-plan.md still present",
            ),
            critique_clean("DM4b"),
        ],
        work_artifacts: &["docs/analysis/"],
        predecessor_inputs: &[
            "docs/targets.md",
            "docs/plan/perf-plan.md",
            ".sim-flow/experiments.db",
        ],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_every_dm_step_in_order() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let order = reg.order_for(Flow::DirectModeling);
        assert_eq!(
            order,
            vec![
                "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2d", "DM3a", "DM3b", "DM3c", "DM4a",
                "DM4b",
            ]
        );
    }

    #[test]
    fn every_dm_step_has_a_critique_check() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        for step in reg.steps() {
            assert!(
                step.gate_checks
                    .iter()
                    .any(|c| matches!(c, GateCheck::CritiqueClean { .. })),
                "{} is missing a critique clean check",
                step.id
            );
        }
    }

    #[test]
    fn prerequisites_chain_as_expected() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let pairs: Vec<_> = reg.steps().iter().map(|s| (s.id, s.prerequisite)).collect();
        assert_eq!(
            pairs,
            vec![
                ("DM0", None),
                ("DM1", Some("DM0")),
                ("DM2a", Some("DM1")),
                ("DM2b", Some("DM2a")),
                ("DM2c", Some("DM2b")),
                ("DM2d", Some("DM2c")),
                ("DM3a", Some("DM2d")),
                ("DM3b", Some("DM3a")),
                ("DM3c", Some("DM3b")),
                ("DM4a", Some("DM3c")),
                ("DM4b", Some("DM4a")),
            ]
        );
    }
}
