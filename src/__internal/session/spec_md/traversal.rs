//! Required-field traversal for the manual-mode Q&A loop
//! (Chapter 2 §2.7).
//!
//! Walks a [`SpecMd`] in template-order and emits one
//! [`MissingField`] per empty REQUIRED slot. DM0's manual loop
//! consumes the ordered list and asks the user about each missing
//! field in turn; Phase 6 builds prompts on top of this output.
//!
//! Out of scope here: actual prompts, automated-mode auto-fill,
//! optional-section "is this applicable?" semantics beyond a single
//! emitted MissingField entry. Those live in Phase 6.

use super::types::SpecMd;

/// One missing required slot in the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingField {
    /// Dotted / arrow-joined path identifying the missing slot
    /// (e.g. `Metadata > design_name`, `Blocks > Fetch > Role`).
    pub section_path: String,
    /// Prompt template the manual-mode loop should display when
    /// asking the user about this field. Phase 6 substitutes
    /// surrounding context into this.
    pub prompt_template: String,
    /// Kind of value the slot wants.
    pub kind: MissingFieldKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingFieldKind {
    /// A single scalar value (string).
    Scalar,
    /// One to three short paragraphs of prose.
    Prose,
    /// A scalar that must match a regex (e.g. clock frequency).
    ConstrainedScalar { regex: String },
    /// A table row with one column per name.
    TableRow { column_names: Vec<&'static str> },
    /// "Does this optional section apply to your design?"
    SectionApplicability,
}

impl SpecMd {
    /// Walk the spec in template-order and return one
    /// [`MissingField`] per empty REQUIRED slot. A fully-populated
    /// spec returns the empty vector.
    pub fn missing_required_fields(&self) -> Vec<MissingField> {
        let mut out: Vec<MissingField> = Vec::new();
        push_metadata(self, &mut out);
        push_prose(
            &self.purpose,
            "Purpose",
            "Describe what the design does in one or two paragraphs.",
            &mut out,
        );
        push_prose(
            &self.scope,
            "Scope",
            "Describe what this model must include.",
            &mut out,
        );
        push_prose(
            &self.non_goals,
            "Non-goals",
            "Describe what is explicitly out of scope.",
            &mut out,
        );
        push_assumptions(self, &mut out);
        // External Interfaces -- REQUIRED if any, but a design with
        // no external boundary is allowed (per Chapter 2 §2.2);
        // emit a SectionApplicability hint instead.
        if self.external_interfaces.is_empty() {
            out.push(MissingField {
                section_path: "External Interfaces".into(),
                prompt_template: "Does this design expose any external interfaces? If so, declare each with `### Interface: <name>`.".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.blocks.is_empty() {
            out.push(MissingField {
                section_path: "Blocks".into(),
                prompt_template: "Add at least one `### Block: <name>` entry covering the top-level architectural unit.".into(),
                kind: MissingFieldKind::Scalar,
            });
        } else {
            for b in &self.blocks {
                push_block_required(b, &mut out);
            }
        }
        // Parameters -- REQUIRED if any.
        if self.parameters.is_empty() {
            out.push(MissingField {
                section_path: "Parameters".into(),
                prompt_template: "Does this design expose configuration parameters? If so, add one row per parameter.".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        // Optional sections: emit applicability prompts.
        if self.state_machines.is_empty() {
            out.push(MissingField {
                section_path: "State Machines".into(),
                prompt_template: "Does this design have any state machines?".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.encodings.is_empty() {
            out.push(MissingField {
                section_path: "Encodings".into(),
                prompt_template: "Does this design define any field-level encodings?".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.memory_map.is_empty() {
            out.push(MissingField {
                section_path: "Memory Map".into(),
                prompt_template: "Does this design have an addressable memory map?".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.connectivity.is_none() {
            out.push(MissingField {
                section_path: "Connectivity".into(),
                prompt_template:
                    "Does this design have a mesh / NoC / topology connectivity story?".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.error_handling.is_empty() {
            out.push(MissingField {
                section_path: "Error Handling".into(),
                prompt_template: "Does this design surface any error / exception conditions?"
                    .into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        push_prose(
            &self.functional_behavior.end_to_end,
            "Functional Behavior > End-to-end behavior",
            "Describe the top-level transformation from inputs to outputs.",
            &mut out,
        );
        // Required by §2.3.12.
        if self.timing.throughput.is_empty()
            && self.timing.stall_and_backpressure.is_empty()
            && self.timing.latency.is_empty()
        {
            out.push(MissingField {
                section_path: "Timing, Latency, and Throughput".into(),
                prompt_template: "Describe latency, throughput, and stall / backpressure behavior."
                    .into(),
                kind: MissingFieldKind::Prose,
            });
        }
        push_prose(
            &self.pipeline_and_hierarchy.prose,
            "Pipeline and Hierarchy",
            "Short prose summary that points at the Blocks section for detail.",
            &mut out,
        );
        if self.reset_init_flush_drain.reset.is_empty()
            && self.reset_init_flush_drain.initialization.is_empty()
            && self.reset_init_flush_drain.flush_and_drain.is_empty()
        {
            out.push(MissingField {
                section_path: "Reset, Initialization, Flush, Drain".into(),
                prompt_template: "Describe reset, initialization, and flush / drain behavior."
                    .into(),
                kind: MissingFieldKind::Prose,
            });
        }
        // Cycle-Accurate and Figures are OPTIONAL.
        if self.cycle_accurate.is_empty() {
            out.push(MissingField {
                section_path: "Cycle-Accurate Behavior".into(),
                prompt_template: "Does this design need a cycle-accurate trace scenario?".into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.figures.is_empty() {
            out.push(MissingField {
                section_path: "Figures".into(),
                prompt_template: "Does the source spec carry figures worth registering here?"
                    .into(),
                kind: MissingFieldKind::SectionApplicability,
            });
        }
        if self.worked_examples.is_empty() {
            out.push(MissingField {
                section_path: "Worked Examples".into(),
                prompt_template:
                    "Provide at least one concrete worked example tracing input to output.".into(),
                kind: MissingFieldKind::Scalar,
            });
        }
        if self.source_spec_anchors.is_empty() {
            out.push(MissingField {
                section_path: "Source-Spec Anchors".into(),
                prompt_template: "Populate the source-anchor index for every spec.md section that maps to a source-spec chunk.".into(),
                kind: MissingFieldKind::TableRow {
                    column_names: vec!["spec.md section", "Source", "Chunk id", "Page range"],
                },
            });
        }
        if self.open_questions.is_empty() {
            out.push(MissingField {
                section_path: "Open Questions".into(),
                prompt_template:
                    "List ambiguities the source spec leaves unresolved (or write `None.`).".into(),
                kind: MissingFieldKind::Scalar,
            });
        }
        if self.auto_decisions.is_empty() {
            out.push(MissingField {
                section_path: "Auto-decisions".into(),
                prompt_template:
                    "Record any non-trivial inference made in automated mode (or leave empty)."
                        .into(),
                kind: MissingFieldKind::Scalar,
            });
        }
        // ---- Phase 9 §7.7 conditional requirements ----
        push_phase9_required(self, &mut out);
        out
    }
}

fn push_prose(value: &str, path: &str, prompt: &str, out: &mut Vec<MissingField>) {
    if value.trim().is_empty() {
        out.push(MissingField {
            section_path: path.to_string(),
            prompt_template: prompt.to_string(),
            kind: MissingFieldKind::Prose,
        });
    }
}

fn push_scalar(value: &str, path: &str, prompt: &str, out: &mut Vec<MissingField>) {
    if value.trim().is_empty() {
        out.push(MissingField {
            section_path: path.to_string(),
            prompt_template: prompt.to_string(),
            kind: MissingFieldKind::Scalar,
        });
    }
}

fn push_metadata(spec: &SpecMd, out: &mut Vec<MissingField>) {
    let md = &spec.metadata;
    push_scalar(
        &md.design_name,
        "Metadata > design_name",
        "What is the design name?",
        out,
    );
    push_scalar(
        &md.version,
        "Metadata > version",
        "What is the design version / revision?",
        out,
    );
    push_scalar(
        &md.status,
        "Metadata > status",
        "Spec status (draft / reviewed / approved)?",
        out,
    );
    if md.authors.is_empty() {
        out.push(MissingField {
            section_path: "Metadata > authors".into(),
            prompt_template: "Who authored this spec?".into(),
            kind: MissingFieldKind::Scalar,
        });
    }
    push_scalar(
        &md.last_updated,
        "Metadata > last_updated",
        "When was the spec last updated (YYYY-MM-DD)?",
        out,
    );
}

fn push_assumptions(spec: &SpecMd, out: &mut Vec<MissingField>) {
    let q = &spec.assumptions.quantitative;
    let has_clock = q
        .iter()
        .any(|r| r.constraint.eq_ignore_ascii_case("Clock frequency") && !r.value.is_empty());
    let has_gate = q
        .iter()
        .any(|r| r.constraint.eq_ignore_ascii_case("Gate budget per cycle") && !r.value.is_empty());
    if !has_clock {
        out.push(MissingField {
            section_path: "Assumptions > Quantitative > Clock frequency".into(),
            prompt_template: "What is the target clock frequency? (must match `\\d+\\s*(MHz|GHz)`)"
                .into(),
            kind: MissingFieldKind::ConstrainedScalar {
                regex: r"\d+\s*(MHz|GHz)".into(),
            },
        });
    }
    if !has_gate {
        out.push(MissingField {
            section_path: "Assumptions > Quantitative > Gate budget per cycle".into(),
            prompt_template: "What is the per-cycle gate budget? (must match `\\d+`)".into(),
            kind: MissingFieldKind::ConstrainedScalar {
                regex: r"\d+".into(),
            },
        });
    }
}

fn push_block_required(b: &super::types::Block, out: &mut Vec<MissingField>) {
    let prefix = format!("Blocks > {}", b.name);
    push_scalar(
        &b.role,
        &format!("{prefix} > Role"),
        "What is this block's role?",
        out,
    );
    push_scalar(
        &b.parent,
        &format!("{prefix} > Parent"),
        "Which block is this block's parent? Use `(none -- top-level)` for the top.",
        out,
    );
}

/// Phase 9 §7.7 conditional requirements. Empty sections produce
/// no MissingField (the relevant ingest may not have discovered
/// any tables); populated sections must have their structurally-
/// required fields. Later phases will tighten this to "this
/// section is REQUIRED when format.json::tables[].kind == ..."
/// per the plan's conditional-requirement contract.
fn push_phase9_required(spec: &SpecMd, out: &mut Vec<MissingField>) {
    for (i, csr) in spec.csrs.iter().enumerate() {
        let label = if csr.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            csr.name.clone()
        };
        let prefix = format!("CSRs > {label}");
        push_scalar(
            &csr.address,
            &format!("{prefix} > Address"),
            "What is this CSR's architectural address?",
            out,
        );
        push_scalar(
            &csr.name,
            &format!("{prefix} > Name"),
            "What is this CSR's name?",
            out,
        );
    }
    for (i, g) in spec.glossary.iter().enumerate() {
        let label = if g.term.is_empty() {
            format!("#{}", i + 1)
        } else {
            g.term.clone()
        };
        let prefix = format!("Glossary > {label}");
        push_scalar(
            &g.term,
            &format!("{prefix} > Term"),
            "What is the acronym / term?",
            out,
        );
        push_scalar(
            &g.expansion,
            &format!("{prefix} > Expansion"),
            "What is the expanded / spelled-out form?",
            out,
        );
    }
    for (i, d) in spec.clock_domains.iter().enumerate() {
        let label = if d.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            d.name.clone()
        };
        push_scalar(
            &d.name,
            &format!("Clock Domains > {label} > Name"),
            "Clock-domain name?",
            out,
        );
    }
    for (i, d) in spec.power_domains.iter().enumerate() {
        let label = if d.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            d.name.clone()
        };
        push_scalar(
            &d.name,
            &format!("Power Domains > {label} > Name"),
            "Power-domain name?",
            out,
        );
    }
    for (i, d) in spec.reset_domains.iter().enumerate() {
        let label = if d.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            d.name.clone()
        };
        push_scalar(
            &d.name,
            &format!("Reset Domains > {label} > Name"),
            "Reset-domain name?",
            out,
        );
    }
    for (i, level) in spec.security_boundaries.iter().enumerate() {
        let label = if level.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            level.name.clone()
        };
        let prefix = format!("Security Boundaries > {label}");
        push_scalar(
            &level.id,
            &format!("{prefix} > Id"),
            "Stable id for this privilege level (e.g. `M`, `S`, `U`)?",
            out,
        );
        push_scalar(
            &level.name,
            &format!("{prefix} > Name"),
            "Human-readable name for this privilege level?",
            out,
        );
    }
    for (i, conv) in spec.numerical_conventions.iter().enumerate() {
        let label = if conv.name.is_empty() {
            format!("#{}", i + 1)
        } else {
            conv.name.clone()
        };
        push_scalar(
            &conv.name,
            &format!("Numerical Conventions > {label} > Name"),
            "Numerical-convention name?",
            out,
        );
    }
    for (i, ev) in spec.performance_counters.iter().enumerate() {
        let label = if ev.id.is_empty() {
            format!("#{}", i + 1)
        } else {
            ev.id.clone()
        };
        let prefix = format!("Performance Counters > {label}");
        push_scalar(
            &ev.id,
            &format!("{prefix} > Id"),
            "Stable id for this PMU event?",
            out,
        );
        push_scalar(
            &ev.name,
            &format!("{prefix} > Name"),
            "Human-readable name for this PMU event?",
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::session::spec_md::types::{
        AssumptionsAndConstraints, Block, Encoding, EncodingValue, ExternalInterface,
        FunctionalBehavior, Metadata, Parameter, PipelineAndHierarchy, QuantitativeRow,
        ResetInitFlushDrain, SpecMd, StateMachine, TimingAndThroughput, WorkedExample,
    };

    #[test]
    fn default_spec_produces_full_traversal() {
        let spec = SpecMd::default();
        let missing = spec.missing_required_fields();
        // Spot-check ordering: Metadata fields come first.
        assert_eq!(missing[0].section_path, "Metadata > design_name");
        // The traversal should include every REQUIRED slot plus
        // optional-section applicability prompts.
        let paths: Vec<&str> = missing.iter().map(|m| m.section_path.as_str()).collect();
        assert!(paths.contains(&"Metadata > version"));
        assert!(paths.contains(&"Purpose"));
        assert!(paths.contains(&"Scope"));
        assert!(paths.contains(&"Non-goals"));
        assert!(paths.contains(&"Assumptions > Quantitative > Clock frequency"));
        assert!(paths.contains(&"Assumptions > Quantitative > Gate budget per cycle"));
        assert!(paths.contains(&"Blocks"));
        assert!(paths.contains(&"Functional Behavior > End-to-end behavior"));
        assert!(paths.contains(&"Pipeline and Hierarchy"));
        assert!(paths.contains(&"Worked Examples"));
        assert!(paths.contains(&"Source-Spec Anchors"));
    }

    #[test]
    fn fully_populated_spec_returns_empty() {
        let spec = SpecMd {
            title: "Doc".into(),
            metadata: Metadata {
                design_name: "X".into(),
                version: "1.0".into(),
                status: "draft".into(),
                authors: vec!["Author".into()],
                last_updated: "2026-05-17".into(),
                ..Default::default()
            },
            purpose: "p".into(),
            scope: "s".into(),
            non_goals: "n".into(),
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "primary:p1".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "primary:p1".into(),
                    },
                ],
                environmental: "env".into(),
                architectural: "arch".into(),
            },
            external_interfaces: vec![ExternalInterface {
                name: "Bus".into(),
                ..Default::default()
            }],
            blocks: vec![Block {
                name: "Top".into(),
                role: "the only".into(),
                parent: "(none -- top-level)".into(),
                ..Default::default()
            }],
            parameters: vec![Parameter {
                name: "XLEN".into(),
                ty: "int".into(),
                default: "32".into(),
                valid_range: String::new(),
                behavioral_impact: String::new(),
                source_anchor: String::new(),
            }],
            state_machines: vec![StateMachine {
                name: "Boot".into(),
                ..Default::default()
            }],
            encodings: vec![Encoding {
                field: "Priv".into(),
                values: vec![EncodingValue {
                    value: "0".into(),
                    name: "U".into(),
                    abbreviation: "U".into(),
                }],
                ..Default::default()
            }],
            memory_map: vec![super::super::types::MemoryRegion {
                start: "0x0".into(),
                end: "0xF".into(),
                name: "R".into(),
                purpose: "p".into(),
                access: "RW".into(),
                source_anchor: "primary:p1".into(),
                ..Default::default()
            }],
            connectivity: Some(Default::default()),
            error_handling: vec![super::super::types::ErrorEntry {
                error_type: "x".into(),
                detecting_component: String::new(),
                detection_behavior: String::new(),
                bus_response: String::new(),
                master_behavior: String::new(),
                software_response: String::new(),
                source_anchor: String::new(),
            }],
            functional_behavior: FunctionalBehavior {
                end_to_end: "e2e".into(),
                operations: Vec::new(),
                data_movement: String::new(),
            },
            timing: TimingAndThroughput {
                latency: Vec::new(),
                throughput: "tp".into(),
                stall_and_backpressure: String::new(),
            },
            pipeline_and_hierarchy: PipelineAndHierarchy { prose: "p".into() },
            reset_init_flush_drain: ResetInitFlushDrain {
                reset: "r".into(),
                initialization: String::new(),
                flush_and_drain: String::new(),
            },
            cycle_accurate: vec![super::super::types::CycleAccurateScenario {
                name: "s".into(),
                ..Default::default()
            }],
            figures: vec![super::super::types::FigureEntry {
                name: "f".into(),
                ..Default::default()
            }],
            worked_examples: vec![WorkedExample {
                name: "x".into(),
                ..Default::default()
            }],
            source_spec_anchors: vec![super::super::types::AnchorIndexEntry {
                section_path: "Blocks > Top".into(),
                source: "primary".into(),
                chunk_id: "chunk-0001".into(),
                page_range: "1".into(),
            }],
            open_questions: vec![super::super::types::OpenQuestion {
                text: "None.".into(),
            }],
            auto_decisions: vec![super::super::types::AutoDecision {
                decision: "none".into(),
                rationale: "none".into(),
            }],
            ..Default::default()
        };
        let missing = spec.missing_required_fields();
        assert!(
            missing.is_empty(),
            "expected no missing fields, got: {missing:#?}"
        );
    }

    #[test]
    fn empty_phase9_sections_produce_no_missing_fields() {
        // Per the plan, empty Phase 9 §7.7 sections do NOT surface
        // as MissingField (the relevant ingest may not have
        // discovered any tables). Confirm the default spec yields
        // no Phase 9 paths.
        let spec = SpecMd::default();
        let missing = spec.missing_required_fields();
        let paths: Vec<&str> = missing.iter().map(|m| m.section_path.as_str()).collect();
        for p in &paths {
            assert!(
                !p.starts_with("CSRs > "),
                "unexpected CSRs MissingField: {p}"
            );
            assert!(
                !p.starts_with("Glossary > "),
                "unexpected Glossary MissingField: {p}"
            );
            assert!(
                !p.starts_with("Clock Domains > "),
                "unexpected Clock Domains MissingField: {p}"
            );
            assert!(
                !p.starts_with("Power Domains > "),
                "unexpected Power Domains MissingField: {p}"
            );
            assert!(
                !p.starts_with("Reset Domains > "),
                "unexpected Reset Domains MissingField: {p}"
            );
            assert!(
                !p.starts_with("Security Boundaries > "),
                "unexpected Security Boundaries MissingField: {p}"
            );
            assert!(
                !p.starts_with("Numerical Conventions > "),
                "unexpected Numerical Conventions MissingField: {p}"
            );
            assert!(
                !p.starts_with("Performance Counters > "),
                "unexpected Performance Counters MissingField: {p}"
            );
        }
    }

    #[test]
    fn populated_phase9_section_with_empty_required_field_fires_missing() {
        let mut spec = SpecMd::default();
        spec.csrs.push(super::super::types::Csr {
            address: String::new(),
            name: "mstatus".into(),
            ..Default::default()
        });
        spec.glossary.push(super::super::types::GlossaryEntry {
            term: "IF".into(),
            expansion: String::new(),
            ..Default::default()
        });
        spec.clock_domains
            .push(super::super::types::ClockDomain::default());
        let missing = spec.missing_required_fields();
        let paths: Vec<&str> = missing.iter().map(|m| m.section_path.as_str()).collect();
        assert!(
            paths.contains(&"CSRs > mstatus > Address"),
            "expected MissingField for empty CSR address; got {paths:?}"
        );
        assert!(
            paths.contains(&"Glossary > IF > Expansion"),
            "expected MissingField for empty glossary expansion; got {paths:?}"
        );
        assert!(
            paths.contains(&"Clock Domains > #1 > Name"),
            "expected MissingField for empty clock-domain name; got {paths:?}"
        );
    }
}
