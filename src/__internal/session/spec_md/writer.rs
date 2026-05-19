//! [`SpecMd`] -> markdown writer.
//!
//! The writer is the inverse of [`crate::session::spec_md::parser`].
//! Round-trip identity holds at the **typed-struct** level: the
//! pipeline
//!
//! ```text
//! parse(write(parse(input))) == parse(input)
//! ```
//!
//! holds byte-equal on every fixture and on the new template (see
//! `tests/spec_md_round_trip.rs`). Byte-equal markdown output is NOT
//! a contract -- whitespace, column widths, and blank-line policies
//! are implementation detail.
//!
//! Sections appear in the canonical Chapter 2 §2.2 order. Empty
//! sections are omitted unless they're REQUIRED (in which case we
//! emit the heading with no body so missing-required-section
//! validation downstream can fire predictably).

use std::fmt::Write;

use super::types::{
    AssumptionsAndConstraints, AutoDecision, Block, Connectivity, CycleAccurateScenario, Encoding,
    ExternalInterface, FigureEntry, FunctionalBehavior, Metadata, OpenQuestion, Parameter,
    PipelineAndHierarchy, QuantitativeRow, ResetInitFlushDrain, SourceDocumentRole, SpecMd,
    StateMachine, TimingAndThroughput,
};

impl SpecMd {
    /// Render this [`SpecMd`] back to markdown. Always ends with a
    /// trailing newline.
    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        if !self.title.is_empty() {
            writeln!(s, "# {}", self.title).unwrap();
            writeln!(s).unwrap();
        }
        write_metadata(&mut s, &self.metadata);
        write_prose_section(&mut s, "Purpose", &self.purpose);
        write_prose_section(&mut s, "Scope", &self.scope);
        write_prose_section(&mut s, "Non-goals", &self.non_goals);
        write_assumptions(&mut s, &self.assumptions);
        if !self.external_interfaces.is_empty() {
            write_external_interfaces(&mut s, &self.external_interfaces);
        }
        write_blocks(&mut s, &self.blocks);
        if !self.parameters.is_empty() {
            write_parameters(&mut s, &self.parameters);
        }
        if !self.state_machines.is_empty() {
            write_state_machines(&mut s, &self.state_machines);
        }
        if !self.encodings.is_empty() {
            write_encodings(&mut s, &self.encodings);
        }
        if !self.memory_map.is_empty() {
            write_memory_map(&mut s, &self.memory_map);
        }
        if let Some(c) = &self.connectivity {
            write_connectivity(&mut s, c);
        }
        if !self.error_handling.is_empty() {
            write_error_handling(&mut s, &self.error_handling);
        }
        write_functional_behavior(&mut s, &self.functional_behavior);
        write_timing(&mut s, &self.timing);
        write_pipeline_and_hierarchy(&mut s, &self.pipeline_and_hierarchy);
        write_reset(&mut s, &self.reset_init_flush_drain);
        if !self.cycle_accurate.is_empty() {
            write_cycle_accurate(&mut s, &self.cycle_accurate);
        }
        if !self.figures.is_empty() {
            write_figures(&mut s, &self.figures);
        }
        write_worked_examples(&mut s, &self.worked_examples);
        write_anchors_section(&mut s, &self.source_spec_anchors);
        write_open_questions(&mut s, &self.open_questions);
        write_auto_decisions(&mut s, &self.auto_decisions);
        s
    }
}

fn write_h2(s: &mut String, heading: &str) {
    writeln!(s, "## {heading}").unwrap();
    writeln!(s).unwrap();
}

fn write_prose_section(s: &mut String, heading: &str, body: &str) {
    write_h2(s, heading);
    if !body.is_empty() {
        writeln!(s, "{body}").unwrap();
        writeln!(s).unwrap();
    }
}

fn write_table(s: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    s.push_str("| ");
    s.push_str(&headers.join(" | "));
    s.push_str(" |\n");
    s.push_str("| ");
    s.push_str(
        &headers
            .iter()
            .map(|_| "---".to_string())
            .collect::<Vec<_>>()
            .join(" | "),
    );
    s.push_str(" |\n");
    for row in rows {
        s.push_str("| ");
        s.push_str(&row.join(" | "));
        s.push_str(" |\n");
    }
    writeln!(s).unwrap();
}

fn write_metadata(s: &mut String, md: &Metadata) {
    write_h2(s, "Metadata");
    if !md.design_name.is_empty() {
        writeln!(s, "- Design name: {}", md.design_name).unwrap();
    }
    if !md.version.is_empty() {
        writeln!(s, "- Version: {}", md.version).unwrap();
    }
    if !md.status.is_empty() {
        writeln!(s, "- Status: {}", md.status).unwrap();
    }
    if !md.authors.is_empty() {
        writeln!(s, "- Authors: {}", md.authors.join(", ")).unwrap();
    }
    if !md.source_documents.is_empty() {
        writeln!(s, "- Source documents:").unwrap();
        for doc in &md.source_documents {
            match doc.role {
                SourceDocumentRole::Primary => {
                    writeln!(s, "  - primary: {}", doc.path).unwrap();
                }
                SourceDocumentRole::Peer => match &doc.peer_id {
                    Some(id) => writeln!(s, "  - peer: {} -> {}", id, doc.path).unwrap(),
                    None => writeln!(s, "  - peer: {}", doc.path).unwrap(),
                },
            }
        }
    }
    if !md.last_updated.is_empty() {
        writeln!(s, "- Last updated: {}", md.last_updated).unwrap();
    }
    writeln!(s).unwrap();
}

fn write_assumptions(s: &mut String, a: &AssumptionsAndConstraints) {
    write_h2(s, "Assumptions and Constraints");
    if !a.quantitative.is_empty() {
        writeln!(s, "### Quantitative").unwrap();
        writeln!(s).unwrap();
        let rows: Vec<Vec<String>> = a
            .quantitative
            .iter()
            .map(|r: &QuantitativeRow| {
                vec![
                    r.constraint.clone(),
                    r.value.clone(),
                    r.source_anchor.clone(),
                ]
            })
            .collect();
        write_table(s, &["Constraint", "Value", "Source-anchor"], &rows);
    }
    if !a.environmental.is_empty() {
        writeln!(s, "### Environmental").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", a.environmental).unwrap();
        writeln!(s).unwrap();
    }
    if !a.architectural.is_empty() {
        writeln!(s, "### Architectural").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", a.architectural).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_external_interfaces(s: &mut String, ifaces: &[ExternalInterface]) {
    write_h2(s, "External Interfaces");
    for iface in ifaces {
        writeln!(s, "### Interface: {}", iface.name).unwrap();
        writeln!(s).unwrap();
        write_property(s, "Direction", &iface.direction);
        write_property(s, "Protocol", &iface.protocol);
        write_property(s, "Clock domain", &iface.clock_domain);
        write_property(s, "Connected peer", &iface.peer);
        writeln!(s).unwrap();
        if !iface.signals.is_empty() {
            writeln!(s, "#### Signals").unwrap();
            writeln!(s).unwrap();
            let rows: Vec<Vec<String>> = iface
                .signals
                .iter()
                .map(|r| {
                    vec![
                        format!("`{}`", r.name),
                        r.direction.clone(),
                        r.width.clone(),
                        r.ty.clone(),
                        if r.required { "yes" } else { "no" }.to_string(),
                        r.description.clone(),
                    ]
                })
                .collect();
            write_table(
                s,
                &[
                    "Signal",
                    "Direction",
                    "Width",
                    "Type",
                    "Required",
                    "Description",
                ],
                &rows,
            );
        }
        if !iface.transaction_semantics.is_empty() {
            writeln!(s, "#### Transaction semantics").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", iface.transaction_semantics).unwrap();
            writeln!(s).unwrap();
        }
        if !iface.timing_and_flow_control.is_empty() {
            writeln!(s, "#### Timing and flow control").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", iface.timing_and_flow_control).unwrap();
            writeln!(s).unwrap();
        }
        if !iface.error_behavior.is_empty() {
            writeln!(s, "#### Error and exceptional behavior").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", iface.error_behavior).unwrap();
            writeln!(s).unwrap();
        }
        if !iface.source_anchors.is_empty() {
            writeln!(s, "#### Source-spec anchors").unwrap();
            writeln!(s).unwrap();
            for a in &iface.source_anchors {
                writeln!(s, "- {a}").unwrap();
            }
            writeln!(s).unwrap();
        }
    }
}

fn write_property(s: &mut String, key: &str, value: &str) {
    if !value.is_empty() {
        writeln!(s, "**{key}:** {value}").unwrap();
    }
}

fn write_blocks(s: &mut String, blocks: &[Block]) {
    write_h2(s, "Blocks");
    for block in blocks {
        writeln!(s, "### Block: {}", block.name).unwrap();
        writeln!(s).unwrap();
        write_property(s, "Role", &block.role);
        write_property(s, "Parent", &block.parent);
        write_property(s, "Clock domain", &block.clock_domain);
        if !block.parameterized_by.is_empty() {
            let params = block
                .parameterized_by
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(s, "**Parameterized by:** {params}").unwrap();
        }
        writeln!(s).unwrap();
        if !block.signals.is_empty() {
            writeln!(s, "#### I/O Signals").unwrap();
            writeln!(s).unwrap();
            let rows: Vec<Vec<String>> = block
                .signals
                .iter()
                .map(|r| {
                    vec![
                        format!("`{}`", r.name),
                        r.direction.clone(),
                        r.peer.clone(),
                        r.description.clone(),
                    ]
                })
                .collect();
            write_table(s, &["Signal", "Direction", "Peer", "Description"], &rows);
        }
        if !block.state.is_empty() {
            writeln!(s, "#### State").unwrap();
            writeln!(s).unwrap();
            for st in &block.state {
                if !st.description.is_empty() {
                    writeln!(s, "- `{}` ({})", st.name, st.description).unwrap();
                } else if !st.width.is_empty() || !st.reset_value.is_empty() {
                    let mut bits = Vec::new();
                    if !st.width.is_empty() {
                        bits.push(format!("{}-wide register", st.width));
                    }
                    if !st.reset_value.is_empty() {
                        bits.push(format!("reset to {}", st.reset_value));
                    }
                    writeln!(s, "- `{}` ({})", st.name, bits.join(", ")).unwrap();
                } else {
                    writeln!(s, "- `{}`", st.name).unwrap();
                }
            }
            writeln!(s).unwrap();
        }
        if !block.behavior_summary.is_empty() {
            writeln!(s, "#### Behavior summary").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", block.behavior_summary).unwrap();
            writeln!(s).unwrap();
        }
        if !block.source_anchors.is_empty() {
            writeln!(s, "#### Source-spec anchors").unwrap();
            writeln!(s).unwrap();
            for a in &block.source_anchors {
                writeln!(s, "- {a}").unwrap();
            }
            writeln!(s).unwrap();
        }
        if !block.figures.is_empty() {
            writeln!(s, "#### Figures").unwrap();
            writeln!(s).unwrap();
            for f in &block.figures {
                writeln!(s, "- {f}").unwrap();
            }
            writeln!(s).unwrap();
        }
        if !block.sub_blocks.is_empty() {
            writeln!(s, "#### Sub-blocks").unwrap();
            writeln!(s).unwrap();
            for sb in &block.sub_blocks {
                writeln!(s, "- {sb}").unwrap();
            }
            writeln!(s).unwrap();
        }
    }
}

fn write_parameters(s: &mut String, params: &[Parameter]) {
    write_h2(s, "Parameters");
    let rows: Vec<Vec<String>> = params
        .iter()
        .map(|p| {
            vec![
                format!("`{}`", p.name),
                p.ty.clone(),
                p.default.clone(),
                p.valid_range.clone(),
                p.behavioral_impact.clone(),
                p.source_anchor.clone(),
            ]
        })
        .collect();
    write_table(
        s,
        &[
            "Name",
            "Type",
            "Default",
            "Valid range",
            "Behavioral impact",
            "Source-anchor",
        ],
        &rows,
    );
}

fn write_state_machines(s: &mut String, fsms: &[StateMachine]) {
    write_h2(s, "State Machines");
    for fsm in fsms {
        writeln!(s, "### FSM: {}", fsm.name).unwrap();
        writeln!(s).unwrap();
        write_property(s, "Reset state", &fsm.reset_state);
        write_property(s, "Source-spec anchor", &fsm.source_anchor);
        writeln!(s).unwrap();
        if !fsm.states.is_empty() {
            writeln!(s, "#### States").unwrap();
            writeln!(s).unwrap();
            for st in &fsm.states {
                if st.description.is_empty() {
                    writeln!(s, "- `{}`", st.name).unwrap();
                } else {
                    writeln!(s, "- `{}` - {}", st.name, st.description).unwrap();
                }
            }
            writeln!(s).unwrap();
        }
        if !fsm.transitions.is_empty() {
            writeln!(s, "#### Transitions").unwrap();
            writeln!(s).unwrap();
            let rows: Vec<Vec<String>> = fsm
                .transitions
                .iter()
                .map(|t| {
                    vec![
                        format!("`{}`", t.from),
                        t.input.clone(),
                        format!("`{}`", t.to),
                        t.output.clone(),
                    ]
                })
                .collect();
            write_table(s, &["From", "Input/Event", "To", "Output/Action"], &rows);
        }
    }
}

fn write_encodings(s: &mut String, encs: &[Encoding]) {
    write_h2(s, "Encodings");
    for enc in encs {
        writeln!(s, "### Encoding: {}", enc.field).unwrap();
        writeln!(s).unwrap();
        write_property(s, "Bit width", &enc.bit_width);
        write_property(s, "Source-anchor", &enc.source_anchor);
        writeln!(s).unwrap();
        if !enc.values.is_empty() {
            let rows: Vec<Vec<String>> = enc
                .values
                .iter()
                .map(|v| {
                    vec![
                        format!("`{}`", v.value),
                        v.name.clone(),
                        v.abbreviation.clone(),
                    ]
                })
                .collect();
            write_table(s, &["Value", "Name", "Abbreviation"], &rows);
        }
        if !enc.reserved.is_empty() {
            writeln!(s, "Reserved / illegal: {}.", enc.reserved).unwrap();
            writeln!(s).unwrap();
        }
    }
}

fn write_memory_map(s: &mut String, regions: &[super::types::MemoryRegion]) {
    write_h2(s, "Memory Map");
    let rows: Vec<Vec<String>> = regions
        .iter()
        .map(|m| {
            vec![
                format!("`{}`", m.start),
                format!("`{}`", m.end),
                m.name.clone(),
                m.purpose.clone(),
                m.access.clone(),
                m.source_anchor.clone(),
            ]
        })
        .collect();
    write_table(
        s,
        &["Start", "End", "Name", "Purpose", "Access", "Source-anchor"],
        &rows,
    );
}

fn write_connectivity(s: &mut String, c: &Connectivity) {
    write_h2(s, "Connectivity");
    if !c.nodes.is_empty() {
        writeln!(s, "### Nodes").unwrap();
        writeln!(s).unwrap();
        let rows: Vec<Vec<String>> = c
            .nodes
            .iter()
            .map(|n| {
                vec![
                    format!("`{}`", n.id),
                    n.ty.clone(),
                    format!("`{}`", n.coordinate),
                    n.role.clone(),
                ]
            })
            .collect();
        write_table(s, &["Id", "Type", "Coordinate", "Role"], &rows);
    }
    if !c.edges.is_empty() {
        writeln!(s, "### Edges").unwrap();
        writeln!(s).unwrap();
        let rows: Vec<Vec<String>> = c
            .edges
            .iter()
            .map(|e| {
                vec![
                    format!("`{}`", e.from),
                    format!("`{}`", e.to),
                    e.channel.clone(),
                    e.source_anchor.clone(),
                ]
            })
            .collect();
        write_table(s, &["From", "To", "Channel", "Source-anchor"], &rows);
    }
    if !c.routing_rules.is_empty() {
        writeln!(s, "### Routing rules").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", c.routing_rules).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_error_handling(s: &mut String, errs: &[super::types::ErrorEntry]) {
    write_h2(s, "Error Handling");
    let rows: Vec<Vec<String>> = errs
        .iter()
        .map(|e| {
            vec![
                e.error_type.clone(),
                e.detecting_component.clone(),
                e.detection_behavior.clone(),
                e.bus_response.clone(),
                e.master_behavior.clone(),
                e.software_response.clone(),
                e.source_anchor.clone(),
            ]
        })
        .collect();
    write_table(
        s,
        &[
            "Error type",
            "Detecting component",
            "Detection behavior",
            "Bus response",
            "Master behavior",
            "Software response",
            "Source-anchor",
        ],
        &rows,
    );
}

fn write_functional_behavior(s: &mut String, fb: &FunctionalBehavior) {
    write_h2(s, "Functional Behavior");
    if !fb.end_to_end.is_empty() {
        writeln!(s, "### End-to-end behavior").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", fb.end_to_end).unwrap();
        writeln!(s).unwrap();
    }
    if !fb.operations.is_empty() {
        writeln!(s, "### Operation flow").unwrap();
        writeln!(s).unwrap();
        for (i, op) in fb.operations.iter().enumerate() {
            let mut line = format!("{}. `{}`", i + 1, op.id);
            if !op.purpose.is_empty() {
                line.push_str(&format!(" - {}", op.purpose));
            }
            if !op.source_anchor.is_empty() {
                line.push_str(&format!(" (anchor: {})", op.source_anchor));
            }
            writeln!(s, "{line}").unwrap();
        }
        writeln!(s).unwrap();
    }
    if !fb.data_movement.is_empty() {
        writeln!(s, "### Data movement").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", fb.data_movement).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_timing(s: &mut String, t: &TimingAndThroughput) {
    write_h2(s, "Timing, Latency, and Throughput");
    if !t.latency.is_empty() {
        writeln!(s, "### Latency").unwrap();
        writeln!(s).unwrap();
        let rows: Vec<Vec<String>> = t
            .latency
            .iter()
            .map(|r| {
                vec![
                    r.operation.clone(),
                    r.best_case.clone(),
                    r.worst_case.clone(),
                    r.notes.clone(),
                ]
            })
            .collect();
        write_table(s, &["Operation", "Best-case", "Worst-case", "Notes"], &rows);
    }
    if !t.throughput.is_empty() {
        writeln!(s, "### Throughput").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", t.throughput).unwrap();
        writeln!(s).unwrap();
    }
    if !t.stall_and_backpressure.is_empty() {
        writeln!(s, "### Stall and backpressure").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", t.stall_and_backpressure).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_pipeline_and_hierarchy(s: &mut String, p: &PipelineAndHierarchy) {
    write_h2(s, "Pipeline and Hierarchy");
    if !p.prose.is_empty() {
        writeln!(s, "{}", p.prose).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_reset(s: &mut String, r: &ResetInitFlushDrain) {
    write_h2(s, "Reset, Initialization, Flush, Drain");
    if !r.reset.is_empty() {
        writeln!(s, "### Reset").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", r.reset).unwrap();
        writeln!(s).unwrap();
    }
    if !r.initialization.is_empty() {
        writeln!(s, "### Initialization").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", r.initialization).unwrap();
        writeln!(s).unwrap();
    }
    if !r.flush_and_drain.is_empty() {
        writeln!(s, "### Flush and drain").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "{}", r.flush_and_drain).unwrap();
        writeln!(s).unwrap();
    }
}

fn write_cycle_accurate(s: &mut String, scenes: &[CycleAccurateScenario]) {
    write_h2(s, "Cycle-Accurate Behavior");
    for sc in scenes {
        writeln!(s, "### Scenario: {}", sc.name).unwrap();
        writeln!(s).unwrap();
        if !sc.columns.is_empty() {
            let headers: Vec<&str> = sc.columns.iter().map(String::as_str).collect();
            let rows: Vec<Vec<String>> = sc.rows.iter().map(|r| r.cells.clone()).collect();
            write_table(s, &headers, &rows);
        }
        write_property(s, "Source-anchor", &sc.source_anchor);
        if !sc.source_anchor.is_empty() {
            writeln!(s).unwrap();
        }
    }
}

fn write_figures(s: &mut String, figs: &[FigureEntry]) {
    write_h2(s, "Figures");
    for f in figs {
        writeln!(s, "### Figure: {}", f.name).unwrap();
        writeln!(s).unwrap();
        write_property(s, "Source page", &f.source_page);
        if !f.raster.is_empty() {
            writeln!(s, "**Raster:** [{}]({})", f.raster, f.raster).unwrap();
        }
        write_property(s, "Role", &f.role);
        if !f.referenced_blocks.is_empty() {
            writeln!(
                s,
                "**Referenced blocks:** {}",
                f.referenced_blocks.join(", ")
            )
            .unwrap();
        }
        writeln!(s).unwrap();
        if !f.caption.is_empty() {
            writeln!(s, "#### Caption").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", f.caption).unwrap();
            writeln!(s).unwrap();
        }
        if !f.elements.is_empty() {
            writeln!(s, "#### Elements depicted").unwrap();
            writeln!(s).unwrap();
            let rows: Vec<Vec<String>> = f
                .elements
                .iter()
                .map(|e| vec![format!("`{}`", e.name), e.kind.clone(), e.notes.clone()])
                .collect();
            write_table(s, &["Element", "Kind", "Notes"], &rows);
        }
    }
}

fn write_worked_examples(s: &mut String, exs: &[super::types::WorkedExample]) {
    write_h2(s, "Worked Examples");
    for (i, ex) in exs.iter().enumerate() {
        writeln!(s, "### Example {}: {}", i + 1, ex.name).unwrap();
        writeln!(s).unwrap();
        if !ex.inputs.is_empty() {
            writeln!(s, "#### Inputs").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", ex.inputs).unwrap();
            writeln!(s).unwrap();
        }
        if !ex.expected_flow.is_empty() {
            writeln!(s, "#### Expected flow").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", ex.expected_flow).unwrap();
            writeln!(s).unwrap();
        }
        if !ex.expected_outputs.is_empty() {
            writeln!(s, "#### Expected outputs").unwrap();
            writeln!(s).unwrap();
            writeln!(s, "{}", ex.expected_outputs).unwrap();
            writeln!(s).unwrap();
        }
    }
}

fn write_anchors_section(s: &mut String, xs: &[super::types::AnchorIndexEntry]) {
    write_h2(s, "Source-Spec Anchors");
    if !xs.is_empty() {
        let rows: Vec<Vec<String>> = xs
            .iter()
            .map(|x| {
                vec![
                    x.section_path.clone(),
                    x.source.clone(),
                    x.chunk_id.clone(),
                    x.page_range.clone(),
                ]
            })
            .collect();
        write_table(
            s,
            &["spec.md section", "Source", "Chunk id", "Page range"],
            &rows,
        );
    }
}

fn write_open_questions(s: &mut String, qs: &[OpenQuestion]) {
    write_h2(s, "Open Questions");
    for q in qs {
        writeln!(s, "- {}", q.text).unwrap();
    }
    if !qs.is_empty() {
        writeln!(s).unwrap();
    }
}

fn write_auto_decisions(s: &mut String, ds: &[AutoDecision]) {
    write_h2(s, "Auto-decisions");
    for d in ds {
        if d.rationale.is_empty() {
            writeln!(s, "- Decided {}.", d.decision).unwrap();
        } else {
            writeln!(s, "- Decided {}; rationale: {}.", d.decision, d.rationale).unwrap();
        }
    }
    if !ds.is_empty() {
        writeln!(s).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_metadata_section() {
        let md = Metadata {
            design_name: "RV12".into(),
            version: "1.0".into(),
            status: "draft".into(),
            authors: vec!["Mike".into()],
            last_updated: "2026-05-17".into(),
            ..Default::default()
        };
        let mut s = String::new();
        write_metadata(&mut s, &md);
        assert!(s.starts_with("## Metadata\n"));
        assert!(s.contains("- Design name: RV12"));
        assert!(s.contains("- Status: draft"));
    }

    #[test]
    fn writes_blocks_with_state_and_signals() {
        let mut spec = SpecMd::default();
        spec.blocks.push(Block {
            name: "Fetch".into(),
            role: "fetch parcels".into(),
            parent: "(none -- top-level)".into(),
            clock_domain: "core".into(),
            signals: vec![super::super::types::BlockSignalRow {
                name: "addr".into(),
                direction: "out".into(),
                peer: "Bus".into(),
                description: "addr line".into(),
                ..Default::default()
            }],
            state: vec![super::super::types::BlockState {
                name: "pc".into(),
                width: "XLEN".into(),
                reset_value: "RESET".into(),
                description: "XLEN-wide register, reset to RESET".into(),
            }],
            ..Default::default()
        });
        let s = spec.to_markdown();
        assert!(s.contains("### Block: Fetch"));
        assert!(s.contains("| `addr` | out | Bus | addr line |"));
        assert!(s.contains("- `pc` (XLEN-wide register, reset to RESET)"));
    }

    #[test]
    fn writes_parameters_table_with_required_headers() {
        let mut spec = SpecMd::default();
        spec.parameters.push(Parameter {
            name: "XLEN".into(),
            ty: "int".into(),
            default: "32".into(),
            valid_range: "32 | 64".into(),
            behavioral_impact: "width".into(),
            source_anchor: "primary:p3".into(),
        });
        let s = spec.to_markdown();
        assert!(s.contains("## Parameters"));
        assert!(s.contains(
            "| Name | Type | Default | Valid range | Behavioral impact | Source-anchor |"
        ));
        assert!(s.contains("| `XLEN` | int | 32 | 32 | 64 | width | primary:p3 |"));
    }

    #[test]
    fn writes_open_questions_bullets() {
        let mut spec = SpecMd::default();
        spec.open_questions.push(OpenQuestion {
            text: "BPU size?".into(),
        });
        let s = spec.to_markdown();
        assert!(s.contains("## Open Questions"));
        assert!(s.contains("- BPU size?"));
    }

    #[test]
    fn auto_decisions_round_trip_token() {
        let mut spec = SpecMd::default();
        spec.auto_decisions.push(AutoDecision {
            decision: "XLEN = 32".into(),
            rationale: "default".into(),
        });
        let s = spec.to_markdown();
        assert!(s.contains("- Decided XLEN = 32; rationale: default."));
    }
}
