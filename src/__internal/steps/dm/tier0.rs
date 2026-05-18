//! DM0 (spec) and DM1 (decomposition).
//!
//! See `super` for the shared GateCheck helper set.

use crate::state::Flow;
use crate::steps::StepDescriptor;

use super::helpers::*;

pub(super) fn dm0() -> StepDescriptor {
    // Phase 6 (Stream C) rewires the DM0 gate around the structured
    // spec.md parser + validator. The single
    // `GateCheck::SpecMdStructured` entry replaces the legacy
    // regex pipeline (file-exists + clock-frequency-regex +
    // gates-per-cycle-regex) with a parser-driven dispatch that
    // checks REQUIRED sections, the Quantitative-row regexes,
    // anchor resolution against the ingest manifest, and (in
    // automated mode) the presence of Auto-decisions. The
    // implementation lives at
    // `crate::__internal::session::dm0::gate::check_dm0_gate`; the
    // evaluator in `crate::__internal::gate::evaluators` converts
    // its `Dm0GateOutcome` into the existing `GateReport`.
    //
    // `docs/spec.md` is the canonical artifact path. The paginated
    // `docs/spec/<NN>-<slug>.md` layout the legacy regex gate
    // tolerated is no longer accepted -- the structured parser is
    // strict about single-file `docs/spec.md`.
    StepDescriptor {
        id: "DM0",
        flow: Flow::DirectModeling,
        prerequisite: None,
        instruction_slug: "dm0-specification",
        per_candidate: false,
        gate_checks: vec![
            spec_md_structured(
                "docs/spec.md",
                Some(".sim-flow/spec-ingest/manifest.toml"),
                "docs/spec.md parses, validates, and its source-anchors resolve",
            ),
            critique_clean("DM0"),
        ],
        walk_gate_checks: vec![],
        work_artifacts: &["docs/spec.md"],
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
