//! Cross-reference and required-row validation for a parsed
//! [`SpecMd`] (Chapter 2 §2.6).
//!
//! This module runs cheap structural checks that don't require any
//! external context (no manifest.toml lookup, no figure-file
//! existence check -- those live in Phase 4 / the gate engine).

use std::collections::HashSet;

use regex::Regex;

use super::types::{SourceSpecAnchor, SpecMd};

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub location: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    Error,
    Warning,
}

impl SpecMd {
    /// Run every cross-reference / required-row check. Returns an
    /// empty vector when the spec is clean.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut out: Vec<ValidationIssue> = Vec::new();
        validate_block_parents(self, &mut out);
        validate_signal_peers(self, &mut out);
        validate_anchors(self, &mut out);
        validate_quantitative_rows(self, &mut out);
        out
    }
}

fn validate_block_parents(spec: &SpecMd, out: &mut Vec<ValidationIssue>) {
    let names: HashSet<&str> = spec.blocks.iter().map(|b| b.name.as_str()).collect();
    for block in &spec.blocks {
        let parent = block.parent.trim();
        if parent.is_empty() || is_top_level_parent(parent) {
            continue;
        }
        if !names.contains(parent) {
            out.push(ValidationIssue {
                severity: IssueSeverity::Error,
                location: format!("Blocks > {}", block.name),
                message: format!(
                    "parent `{parent}` does not match any declared block (and is not the literal `(none -- top-level)`)"
                ),
            });
        }
    }
}

fn is_top_level_parent(s: &str) -> bool {
    let n = s
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .to_ascii_lowercase();
    n.contains("none") && n.contains("top-level")
}

fn validate_signal_peers(spec: &SpecMd, out: &mut Vec<ValidationIssue>) {
    let mut valid: HashSet<String> = HashSet::new();
    for b in &spec.blocks {
        valid.insert(b.name.clone());
    }
    for i in &spec.external_interfaces {
        valid.insert(i.name.clone());
    }
    for block in &spec.blocks {
        for sig in &block.signals {
            let peer = sig.peer.trim();
            if peer.is_empty() {
                continue;
            }
            // Some block signals reference internal sub-blocks (e.g.
            // `EX/State`) or external concepts. We treat a `/`-
            // containing token as a path that must resolve along its
            // leading component.
            let head = peer.split('/').next().unwrap_or(peer).trim();
            if !valid.contains(head) && !valid.contains(peer) {
                out.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    location: format!("Blocks > {} > {}", block.name, sig.name),
                    message: format!(
                        "peer `{peer}` does not match any declared block or external interface"
                    ),
                });
            }
        }
    }
}

fn validate_anchors(spec: &SpecMd, out: &mut Vec<ValidationIssue>) {
    let mut check = |loc: String, anchor: &str| {
        let s = anchor.trim();
        if s.is_empty() {
            return;
        }
        if let Err(e) = SourceSpecAnchor::parse(s) {
            out.push(ValidationIssue {
                severity: IssueSeverity::Error,
                location: loc,
                message: format!("malformed source anchor `{s}`: {e}"),
            });
        }
    };
    for row in &spec.assumptions.quantitative {
        check(
            format!("Assumptions > Quantitative > {}", row.constraint),
            &row.source_anchor,
        );
    }
    for iface in &spec.external_interfaces {
        for a in &iface.source_anchors {
            check(format!("External Interfaces > {}", iface.name), a);
        }
    }
    for block in &spec.blocks {
        for a in &block.source_anchors {
            check(format!("Blocks > {}", block.name), a);
        }
    }
    for p in &spec.parameters {
        check(format!("Parameters > {}", p.name), &p.source_anchor);
    }
    for fsm in &spec.state_machines {
        check(format!("State Machines > {}", fsm.name), &fsm.source_anchor);
    }
    for e in &spec.encodings {
        check(format!("Encodings > {}", e.field), &e.source_anchor);
    }
    for m in &spec.memory_map {
        check(format!("Memory Map > {}", m.name), &m.source_anchor);
    }
    if let Some(c) = &spec.connectivity {
        for e in &c.edges {
            check(
                format!("Connectivity > Edges > {} -> {}", e.from, e.to),
                &e.source_anchor,
            );
        }
    }
    for e in &spec.error_handling {
        check(
            format!("Error Handling > {}", e.error_type),
            &e.source_anchor,
        );
    }
    for op in &spec.functional_behavior.operations {
        check(
            format!("Functional Behavior > Operation > {}", op.id),
            &op.source_anchor,
        );
    }
    for sc in &spec.cycle_accurate {
        check(
            format!("Cycle-Accurate Behavior > {}", sc.name),
            &sc.source_anchor,
        );
    }
}

fn validate_quantitative_rows(spec: &SpecMd, out: &mut Vec<ValidationIssue>) {
    let clock_re = Regex::new(r"\d+\s*(MHz|GHz)").unwrap();
    let gate_re = Regex::new(r"\d+").unwrap();
    let mut have_clock = false;
    let mut have_gate = false;
    for row in &spec.assumptions.quantitative {
        let key = row.constraint.to_ascii_lowercase();
        if key.contains("clock frequency") && clock_re.is_match(&row.value) {
            have_clock = true;
        }
        if (key.contains("gate budget per cycle") || key.contains("gate budget"))
            && gate_re.is_match(&row.value)
        {
            have_gate = true;
        }
    }
    if !have_clock {
        out.push(ValidationIssue {
            severity: IssueSeverity::Error,
            location: "Assumptions > Quantitative".to_string(),
            message:
                "missing required row `Clock frequency` with a value matching `\\d+\\s*(MHz|GHz)`"
                    .to_string(),
        });
    }
    if !have_gate {
        out.push(ValidationIssue {
            severity: IssueSeverity::Error,
            location: "Assumptions > Quantitative".to_string(),
            message: "missing required row `Gate budget per cycle` with a value matching `\\d+`"
                .to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_md::types::{
        AssumptionsAndConstraints, Block, BlockSignalRow, ExternalInterface, QuantitativeRow,
    };

    fn minimal_valid_spec() -> SpecMd {
        SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "primary:p3".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50-100".into(),
                        source_anchor: "primary:p3".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn missing_block_parent_is_error() {
        let mut spec = minimal_valid_spec();
        spec.blocks.push(Block {
            name: "Fetch".into(),
            parent: "DoesNotExist".into(),
            ..Default::default()
        });
        let issues = spec.validate();
        assert!(issues.iter().any(|i| i.message.contains("DoesNotExist")));
    }

    #[test]
    fn top_level_parent_is_ok() {
        let mut spec = minimal_valid_spec();
        spec.blocks.push(Block {
            name: "Pipeline".into(),
            parent: "(none -- top-level)".into(),
            ..Default::default()
        });
        assert!(spec.validate().is_empty());
    }

    #[test]
    fn unknown_signal_peer_is_warning() {
        let mut spec = minimal_valid_spec();
        spec.blocks.push(Block {
            name: "Fetch".into(),
            parent: "(none -- top-level)".into(),
            signals: vec![BlockSignalRow {
                name: "x".into(),
                direction: "in".into(),
                peer: "MysteryPeer".into(),
                description: String::new(),
            }],
            ..Default::default()
        });
        let issues = spec.validate();
        assert!(issues.iter().any(|i| {
            i.severity == IssueSeverity::Warning && i.message.contains("MysteryPeer")
        }));
    }

    #[test]
    fn known_signal_peer_is_ok() {
        let mut spec = minimal_valid_spec();
        spec.external_interfaces.push(ExternalInterface {
            name: "Bus Interface".into(),
            ..Default::default()
        });
        spec.blocks.push(Block {
            name: "Fetch".into(),
            parent: "(none -- top-level)".into(),
            signals: vec![BlockSignalRow {
                name: "x".into(),
                direction: "in".into(),
                peer: "Bus Interface".into(),
                description: String::new(),
            }],
            ..Default::default()
        });
        assert!(spec.validate().is_empty());
    }

    #[test]
    fn missing_quantitative_row_is_error() {
        let mut spec = minimal_valid_spec();
        spec.assumptions.quantitative.clear();
        let issues = spec.validate();
        assert!(issues.iter().any(|i| i.message.contains("Clock frequency")));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("Gate budget per cycle"))
        );
    }

    #[test]
    fn bad_anchor_is_error() {
        let mut spec = minimal_valid_spec();
        spec.assumptions.quantitative[0].source_anchor = "not-an-anchor".into();
        let issues = spec.validate();
        assert!(issues.iter().any(|i| i.message.contains("not-an-anchor")));
    }

    #[test]
    fn minimal_valid_spec_passes() {
        assert!(minimal_valid_spec().validate().is_empty());
    }
}
