//! SystemVerilog conversion flow.
//!
//! Translates a DirectModeling-completed project (Rust model under
//! `src/`, UVM-lite testbench under `tests/`) into synthesizable
//! SystemVerilog RTL plus a UVM testbench under `generated/`. The
//! flow mirrors DM3a → DM3ad → DM3b → DM3c structure so the LLM
//! processes the conversion in small, critiqued slices instead of
//! emitting the entire SV tree in a single response:
//!
//! - **SV0** (plan): classify each Foundation module into a hardware
//!   pattern, draft `generated/plan.md` + per-area milestone stubs
//!   under `generated/plan/`. Mirrors DM3a (outline + stubs).
//! - **SV0d** (plan detail, planning-detail walk): replace each
//!   milestone stub's `<!-- detail-pending -->` with a concrete file
//!   list and per-file tasks. Mirrors DM3ad (`placeholder_marker =
//!   Some(...)`).
//! - **SV1** (RTL emission, execution walk): for each `rtl-milestone-NN`
//!   file, write the listed `generated/rtl/*.sv` files and mark each
//!   task `- [x]`. One Foundation module per emission slice.
//! - **SV2** (UVM emission, execution walk): for each `uvm-milestone-NN`
//!   file, write the listed `generated/test/*.sv` files (types,
//!   interfaces, sequences, driver, monitor, scoreboard, env, base
//!   test, per-test, top, sim.f, Makefile).
//! - **SV3** (build + validate): run `verilator --binary` against the
//!   compile list, smoke-run `./obj_dir/V<top>`, iterate on failures
//!   until clean.
//!
//! Opt-in: the project switches into this flow after the
//! `DirectModeling` flow's DM4b passes (via a future `sim-flow flow
//! switch` operation or `sim-flow convert-sv`).

use std::path::PathBuf;

use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor, StepRegistry};

pub fn register(reg: &mut StepRegistry) {
    reg.register(sv0());
    reg.register(sv0d());
    reg.register(sv1());
    reg.register(sv2());
    reg.register(sv3());
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

fn milestones_all_implemented(dir: &str, file_prefix: &str, description: &str) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: vec![file_prefix.to_string()],
        placeholder_marker: None,
        description: description.to_string(),
        forbid_deferred: true,
    }
}

fn milestones_all_detailed(
    dir: &str,
    file_prefixes: &[&str],
    placeholder_marker: &str,
    description: &str,
) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: file_prefixes.iter().map(|s| (*s).to_string()).collect(),
        placeholder_marker: Some(placeholder_marker.to_string()),
        description: description.to_string(),
        forbid_deferred: false,
    }
}

/// SV0 (Plan) -- classify modules into HW patterns, draft
/// `generated/plan.md` + per-area milestone stubs.
fn sv0() -> StepDescriptor {
    StepDescriptor {
        id: "SV0",
        flow: Flow::SystemVerilogConvert,
        prerequisite: None,
        instruction_slug: "sv0-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("generated/plan.md", "generated/plan.md exists (index)"),
            shell(
                "sh",
                &["-c", "ls generated/plan/rtl-milestone-*.md >/dev/null 2>&1"],
                "generated/plan/ contains at least one rtl-milestone-NN-*.md stub",
            ),
            shell(
                "sh",
                &["-c", "ls generated/plan/uvm-milestone-*.md >/dev/null 2>&1"],
                "generated/plan/ contains at least one uvm-milestone-NN-*.md stub",
            ),
            file_matches(
                "generated/plan.md",
                r"(?i)pattern|simple pipeline|stateful|fifo|cdc",
                "plan.md classifies each module into a hardware pattern",
            ),
            critique_clean("SV0"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["generated/plan.md", "generated/plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/data-movement.md",
            "docs/test-plan/test-plan.md",
            "src/",
            "tests/",
        ],
        work_write_paths: &["generated/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// SV0d (Plan detail, planning-detail walk) -- fill in each
/// milestone stub's task list.
fn sv0d() -> StepDescriptor {
    StepDescriptor {
        id: "SV0d",
        flow: Flow::SystemVerilogConvert,
        prerequisite: Some("SV0"),
        instruction_slug: "sv0d-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "generated/plan/",
                &["rtl-milestone-", "uvm-milestone-"],
                "<!-- detail-pending",
                "every milestone stub under generated/plan/ has been detailed (placeholder removed)",
            ),
            critique_clean("SV0d"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["generated/plan/"],
        predecessor_inputs: &["generated/plan.md", "src/", "tests/"],
        work_write_paths: &["generated/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "generated/plan/",
            file_prefixes: &["rtl-milestone-", "uvm-milestone-"],
            index_file: "generated/plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        }),
    }
}

/// SV1 (RTL emission, execution walk) -- one Foundation module per
/// slice, emit `generated/rtl/*.sv`.
fn sv1() -> StepDescriptor {
    StepDescriptor {
        id: "SV1",
        flow: Flow::SystemVerilogConvert,
        prerequisite: Some("SV0d"),
        instruction_slug: "sv1-rtl",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "generated/rtl/payloads.sv",
                "generated/rtl/payloads.sv exists (shared packed structs / typedefs)",
            ),
            file_exists(
                "generated/rtl/top.sv",
                "generated/rtl/top.sv exists (top-level DUT wiring)",
            ),
            milestones_all_implemented(
                "generated/plan/",
                "rtl-milestone-",
                "every rtl-milestone task is resolved",
            ),
            critique_clean("SV1"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["generated/rtl/"],
        predecessor_inputs: &[
            "generated/plan.md",
            "generated/plan/",
            "src/",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
        ],
        work_write_paths: &["generated/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "generated/plan/",
            file_prefixes: &["rtl-milestone-"],
            index_file: "generated/plan.md",
            placeholder_marker: None,
            forbid_deferred: true,
        }),
    }
}

/// SV2 (UVM emission, execution walk) -- emit testbench components
/// + sequences + per-test files.
fn sv2() -> StepDescriptor {
    StepDescriptor {
        id: "SV2",
        flow: Flow::SystemVerilogConvert,
        prerequisite: Some("SV1"),
        instruction_slug: "sv2-uvm",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "generated/test/uvm_types_pkg.sv",
                "generated/test/uvm_types_pkg.sv exists (sequence items + common typedefs)",
            ),
            file_exists(
                "generated/test/uvm_env.sv",
                "generated/test/uvm_env.sv exists (UVM env wiring)",
            ),
            file_exists(
                "generated/test/tb_top.sv",
                "generated/test/tb_top.sv exists (testbench top module)",
            ),
            milestones_all_implemented(
                "generated/plan/",
                "uvm-milestone-",
                "every uvm-milestone task is resolved",
            ),
            critique_clean("SV2"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["generated/test/"],
        predecessor_inputs: &[
            "generated/plan.md",
            "generated/plan/",
            "generated/rtl/",
            "tests/",
            "docs/test-plan/test-plan.md",
        ],
        work_write_paths: &["generated/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "generated/plan/",
            file_prefixes: &["uvm-milestone-"],
            index_file: "generated/plan.md",
            placeholder_marker: None,
            forbid_deferred: true,
        }),
    }
}

/// SV3 (Build + validate) -- emit sim.f / Makefile, run verilator,
/// iterate on failures.
fn sv3() -> StepDescriptor {
    StepDescriptor {
        id: "SV3",
        flow: Flow::SystemVerilogConvert,
        prerequisite: Some("SV2"),
        instruction_slug: "sv3-build",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "generated/test/sim.f",
                "generated/test/sim.f exists (flat compile-order file list)",
            ),
            file_exists(
                "generated/test/Makefile",
                "generated/test/Makefile exists (runnable simulation flow)",
            ),
            file_exists(
                "generated/validation.md",
                "generated/validation.md exists (tool, command, results)",
            ),
            critique_clean("SV3"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &[
            "generated/test/sim.f",
            "generated/test/Makefile",
            "generated/validation.md",
            "generated/manifest.md",
        ],
        predecessor_inputs: &["generated/rtl/", "generated/test/", "generated/plan.md"],
        work_write_paths: &["generated/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sv_steps_register_in_canonical_order() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let ids: Vec<&'static str> = reg.steps().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["SV0", "SV0d", "SV1", "SV2", "SV3"]);
    }

    #[test]
    fn sv_step_prerequisites_chain_linearly() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        assert_eq!(reg.get("SV0").unwrap().prerequisite, None);
        assert_eq!(reg.get("SV0d").unwrap().prerequisite, Some("SV0"));
        assert_eq!(reg.get("SV1").unwrap().prerequisite, Some("SV0d"));
        assert_eq!(reg.get("SV2").unwrap().prerequisite, Some("SV1"));
        assert_eq!(reg.get("SV3").unwrap().prerequisite, Some("SV2"));
    }

    #[test]
    fn sv0d_is_planning_detail_walk_with_placeholder_marker() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let walk = reg.get("SV0d").unwrap().milestone_walk.unwrap();
        assert_eq!(walk.dir, "generated/plan/");
        assert!(
            walk.placeholder_marker.is_some(),
            "SV0d is a planning-detail walk"
        );
        assert!(!walk.forbid_deferred);
    }

    #[test]
    fn sv1_and_sv2_are_execution_walks_forbidding_deferrals() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let sv1 = reg.get("SV1").unwrap().milestone_walk.unwrap();
        assert!(sv1.placeholder_marker.is_none());
        assert!(sv1.forbid_deferred);
        let sv2 = reg.get("SV2").unwrap().milestone_walk.unwrap();
        assert!(sv2.placeholder_marker.is_none());
        assert!(sv2.forbid_deferred);
    }
}
