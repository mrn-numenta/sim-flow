//! DM0 (spec) and DM1 (decomposition).
//!
//! See `super` for the shared GateCheck helper set.

use crate::state::Flow;
use crate::steps::StepDescriptor;

use super::helpers::*;

pub(super) fn dm0() -> StepDescriptor {
    // The spec can land in either of two layouts and the gate
    // accepts both:
    //   - Single file: `docs/spec.md` (small designs).
    //   - Paginated:  `docs/spec/<NN>-<slug>.md` section files
    //     (large designs that exceed an LLM's single-response
    //     budget; mirrors the input-spec staging convention).
    // The `any_exists` / `any_matches` helpers expand the directory
    // entry to all `*.md` files inside (excluding scaffolding and
    // index files like `README.md` / `_toc.md`).
    StepDescriptor {
        id: "DM0",
        flow: Flow::DirectModeling,
        prerequisite: None,
        instruction_slug: "dm0-specification",
        per_candidate: false,
        gate_checks: vec![
            any_exists(
                &["docs/spec.md", "docs/spec/"],
                "docs/spec.md or docs/spec/ exists and is non-empty",
            ),
            any_matches(
                &["docs/spec.md", "docs/spec/"],
                r"\d+\s*(MHz|GHz)",
                "spec declares a clock frequency",
            ),
            // Gates-per-cycle is REQUIRED -- DM2 needs an explicit
            // budget number from the source material, not an
            // LLM-derived estimate from frequency + technology node.
            // Technology node is now optional (downstream context for
            // power / area discussion) and not gate-checked.
            any_matches(
                &["docs/spec.md", "docs/spec/"],
                r"(?i)gates\s+per\s+cycle.*\d+",
                "spec declares an explicit gates-per-cycle budget",
            ),
            critique_clean("DM0"),
        ],
        walk_gate_checks: vec![],
        // Both layouts listed so a Reset to DM0 (or any downstream
        // reset that cascades through DM0) clears either form.
        work_artifacts: &["docs/spec.md", "docs/spec/"],
        predecessor_inputs: &[],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

pub(super) fn dm1() -> StepDescriptor {
    StepDescriptor {
        id: "DM1",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM0"),
        instruction_slug: "dm1-modeling-setup",
        per_candidate: false,
        gate_checks: vec![
            // targets supports dual layout: single-file `docs/targets.md`
            // OR paginated `docs/targets/<NN>-<slug>.md` section files,
            // mirroring the spec layout. Big projects exceed single-file
            // context budgets and must paginate; small projects use the
            // single file.
            any_exists(
                &["docs/targets.md", "docs/targets/"],
                "docs/targets.md or docs/targets/ exists and is non-empty",
            ),
            any_matches(
                &["docs/targets.md", "docs/targets/"],
                r"(?i)\d+\s*(cycles?|ns|MHz|GHz|items|bits|gates)",
                "targets declare at least one quantitative target",
            ),
            file_exists("docs/testbench.md", "docs/testbench.md exists"),
            file_matches(
                "docs/testbench.md",
                r"(Sequencer|Driver|Monitor|Scoreboard)",
                "docs/testbench.md names at least one UVM-lite component",
            ),
            file_matches(
                "docs/testbench.md",
                r"lib:examples/\d{2}-[a-z0-9-]+/test/?",
                "docs/testbench.md names a concrete lib:examples/<NN-name>/test/ baseline DM3b will mirror",
            ),
            critique_clean("DM1"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/targets.md", "docs/targets/", "docs/testbench.md"],
        predecessor_inputs: &["docs/spec.md", "docs/spec/"],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}
