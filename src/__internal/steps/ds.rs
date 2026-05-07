//! DS step registration. Phase 1 provides the skeleton registration;
//! Phase 6 fills in real gate checks.

use std::path::PathBuf;

use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::{StepDescriptor, StepRegistry};

pub fn register(reg: &mut StepRegistry) {
    let steps: &[(&'static str, Option<&'static str>, &'static str, bool)] = &[
        ("DS0", None, "ds0-specification", false),
        ("DS1", Some("DS0"), "ds1-study-setup", false),
        ("DS2", Some("DS1"), "ds2-decomposition", false),
        ("DS3", Some("DS2"), "ds3-pipeline-mapping", false),
        ("DS4", Some("DS3"), "ds4-analytical-screening", false),
        ("DS5a", Some("DS4"), "ds5a-candidate-prototyping", true),
        ("DS5b", Some("DS5a"), "ds5b-candidate-validation", true),
        ("DS6", Some("DS5b"), "ds6-comparison", false),
        ("DS7", Some("DS6"), "ds7-deep-analysis", false),
        ("DS8", Some("DS7"), "ds8-decision", false),
        ("DS9", Some("DS8"), "ds9-formalize", false),
    ];
    for (id, prereq, slug, per_candidate) in steps {
        reg.register(StepDescriptor {
            id,
            flow: Flow::DesignStudy,
            prerequisite: *prereq,
            instruction_slug: slug,
            per_candidate: *per_candidate,
            gate_checks: vec![GateCheck::CritiqueClean {
                path: PathBuf::from(format!("docs/critiques/{id}-critique.md")),
                description: format!("{id} critique has no blockers"),
            }],
            // DSF artifact paths, tool catalogs, and phase pipelines
            // are filled in by Phase 6 alongside the real gate
            // checks. Empty for now so `describe` returns
            // structurally-valid but content-empty descriptors.
            work_artifacts: &[],
            predecessor_inputs: &[],
            work_write_paths: &["docs/"],
            work_phases: &["chat"],
            critique_phases: &["chat"],
            milestone_walk: None,
        });
    }
}
