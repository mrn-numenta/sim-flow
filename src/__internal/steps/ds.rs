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
            walk_gate_checks: vec![],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::StepRegistry;

    #[test]
    fn register_populates_all_ds_steps_in_order() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let ids: Vec<&str> = reg.order_for(Flow::DesignStudy);
        assert_eq!(
            ids,
            vec![
                "DS0", "DS1", "DS2", "DS3", "DS4", "DS5a", "DS5b", "DS6", "DS7", "DS8", "DS9",
            ]
        );
    }

    #[test]
    fn register_chains_prerequisites_into_ordered_walk() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        // Every step except DS0 must point at its immediate
        // predecessor; DS0 is the head of the flow.
        let ds0 = reg.get("DS0").expect("DS0 registered");
        assert_eq!(ds0.prerequisite, None);
        let chain = [
            ("DS1", "DS0"),
            ("DS2", "DS1"),
            ("DS3", "DS2"),
            ("DS4", "DS3"),
            ("DS5a", "DS4"),
            ("DS5b", "DS5a"),
            ("DS6", "DS5b"),
            ("DS7", "DS6"),
            ("DS8", "DS7"),
            ("DS9", "DS8"),
        ];
        for (id, expected_prereq) in chain {
            let step = reg.get(id).unwrap_or_else(|| panic!("{id} registered"));
            assert_eq!(step.prerequisite, Some(expected_prereq), "{id} prereq");
        }
    }

    #[test]
    fn register_marks_only_the_candidate_pair_per_candidate() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        // DS5a + DS5b are the per-candidate prototype/validation
        // pair; every other DS step is flat.
        for step in reg.steps() {
            let expected = matches!(step.id, "DS5a" | "DS5b");
            assert_eq!(
                step.per_candidate, expected,
                "{} per_candidate mismatch",
                step.id
            );
        }
    }

    #[test]
    fn register_attaches_default_critique_clean_gate_to_each_step() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        for step in reg.steps() {
            assert_eq!(
                step.gate_checks.len(),
                1,
                "{} should have exactly one gate check (CritiqueClean)",
                step.id,
            );
            match &step.gate_checks[0] {
                GateCheck::CritiqueClean { path, description } => {
                    assert!(
                        path.to_string_lossy()
                            .ends_with(&format!("{}-critique.md", step.id)),
                        "{} critique path mismatch: {:?}",
                        step.id,
                        path,
                    );
                    assert!(
                        description.contains(step.id),
                        "{} description should mention step id; got {description}",
                        step.id,
                    );
                }
                other => panic!("{} unexpected gate-check variant: {:?}", step.id, other),
            }
        }
    }

    #[test]
    fn register_uses_design_study_flow_for_every_step() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        for step in reg.steps() {
            assert_eq!(
                step.flow,
                Flow::DesignStudy,
                "{} should be tagged DesignStudy",
                step.id,
            );
        }
    }

    #[test]
    fn register_uses_phase1_phase_pipeline_placeholders() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        for step in reg.steps() {
            assert_eq!(step.work_phases, &["chat"], "{} work_phases", step.id);
            assert_eq!(
                step.critique_phases,
                &["chat"],
                "{} critique_phases",
                step.id
            );
            assert_eq!(step.work_write_paths, &["docs/"], "{} write_paths", step.id);
            assert!(step.work_artifacts.is_empty(), "{} work_artifacts", step.id);
            assert!(
                step.predecessor_inputs.is_empty(),
                "{} predecessor_inputs",
                step.id
            );
            assert!(step.milestone_walk.is_none(), "{} milestone_walk", step.id);
            assert!(
                step.walk_gate_checks.is_empty(),
                "{} walk_gate_checks",
                step.id
            );
        }
        // Concrete slug spot-check: the slug is `<id-lower>-<topic>`
        // where topic is registry-defined free-form text.
        assert_eq!(
            reg.get("DS0").unwrap().instruction_slug,
            "ds0-specification"
        );
        assert_eq!(
            reg.get("DS5a").unwrap().instruction_slug,
            "ds5a-candidate-prototyping"
        );
        assert_eq!(reg.get("DS9").unwrap().instruction_slug, "ds9-formalize");
    }
}
