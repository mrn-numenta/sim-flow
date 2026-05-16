use super::*;
use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::StepRegistry;

#[test]
fn registers_every_dm_step_in_order() {
    let mut reg = StepRegistry::new();
    register(&mut reg);
    let order = reg.order_for(Flow::DirectModeling);
    assert_eq!(
        order,
        vec![
            "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd", "DM2d", "DM3a", "DM3ad", "DM3b", "DM3c",
            "DM4a", "DM4ad", "DM4b",
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
            ("DM2cd", Some("DM2c")),
            ("DM2d", Some("DM2cd")),
            ("DM3a", Some("DM2d")),
            ("DM3ad", Some("DM3a")),
            ("DM3b", Some("DM3ad")),
            ("DM3c", Some("DM3b")),
            ("DM4a", Some("DM3c")),
            ("DM4ad", Some("DM4a")),
            ("DM4b", Some("DM4ad")),
        ]
    );
}
