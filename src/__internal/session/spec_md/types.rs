//! Rust type definitions for the structured `spec.md` schema.
//!
//! These mirror the section layout defined in Chapter 2 of the
//! architecture (`docs/architecture/02-spec-md-schema.md`). Each
//! section in the markdown maps to a typed struct or a `Vec` of
//! typed rows. The top-level [`SpecMd`] holds every section in the
//! fixed order specified in §2.2.
//!
//! Every type derives `Serialize` / `Deserialize` so the parser and
//! the lance build path can move `SpecMd` values across the
//! orchestrator boundary as JSON / TOML without further conversion.

use serde::{Deserialize, Serialize};

/// Top-level structured spec.md document. Fields appear in the
/// canonical section order from Chapter 2 §2.2.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpecMd {
    /// H1 document title (e.g. "RV12 RISC-V CPU Core Design
    /// Specification"). Empty when the source omits the title.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub non_goals: String,
    #[serde(default)]
    pub assumptions: AssumptionsAndConstraints,
    #[serde(default)]
    pub external_interfaces: Vec<ExternalInterface>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub state_machines: Vec<StateMachine>,
    #[serde(default)]
    pub encodings: Vec<Encoding>,
    #[serde(default)]
    pub memory_map: Vec<MemoryRegion>,
    #[serde(default)]
    pub connectivity: Option<Connectivity>,
    #[serde(default)]
    pub error_handling: Vec<ErrorEntry>,
    #[serde(default)]
    pub functional_behavior: FunctionalBehavior,
    #[serde(default)]
    pub timing: TimingAndThroughput,
    #[serde(default)]
    pub pipeline_and_hierarchy: PipelineAndHierarchy,
    #[serde(default)]
    pub reset_init_flush_drain: ResetInitFlushDrain,
    #[serde(default)]
    pub cycle_accurate: Vec<CycleAccurateScenario>,
    #[serde(default)]
    pub figures: Vec<FigureEntry>,
    #[serde(default)]
    pub worked_examples: Vec<WorkedExample>,
    #[serde(default)]
    pub source_spec_anchors: Vec<AnchorIndexEntry>,
    #[serde(default)]
    pub open_questions: Vec<OpenQuestion>,
    #[serde(default)]
    pub auto_decisions: Vec<AutoDecision>,
}

/// `## Metadata` section. Key/value pairs from the definition-list
/// shape in §2.3.1.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub design_name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub source_documents: Vec<SourceDocument>,
    #[serde(default)]
    pub last_updated: String,
}

/// One entry in the metadata Source-documents list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDocument {
    pub role: SourceDocumentRole,
    /// Peer ID — present only for `role = peer`; matches an entry in
    /// `manifest.toml.peers[].id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    pub path: String,
}

/// Role of a Source-document entry: the design's primary spec or a
/// peer reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceDocumentRole {
    Primary,
    Peer,
}

/// `## Assumptions and Constraints` — quantitative table + two prose
/// subsections.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssumptionsAndConstraints {
    #[serde(default)]
    pub quantitative: Vec<QuantitativeRow>,
    #[serde(default)]
    pub environmental: String,
    #[serde(default)]
    pub architectural: String,
}

/// One row of the `### Quantitative` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantitativeRow {
    pub constraint: String,
    pub value: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// One entry under `## External Interfaces`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalInterface {
    pub name: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub clock_domain: String,
    #[serde(default)]
    pub peer: String,
    #[serde(default)]
    pub signals: Vec<ExternalSignalRow>,
    #[serde(default)]
    pub transaction_semantics: String,
    #[serde(default)]
    pub timing_and_flow_control: String,
    #[serde(default)]
    pub error_behavior: String,
    #[serde(default)]
    pub source_anchors: Vec<String>,
}

/// One row of an External Interface signal table (six-column form).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalSignalRow {
    pub name: String,
    pub direction: String,
    pub width: String,
    #[serde(rename = "type", default)]
    pub ty: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

/// One entry under `## Blocks`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// Parent block name, or the literal `(none -- top-level)` for
    /// the top of the hierarchy.
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub clock_domain: String,
    #[serde(default)]
    pub parameterized_by: Vec<String>,
    #[serde(default)]
    pub signals: Vec<BlockSignalRow>,
    #[serde(default)]
    pub state: Vec<BlockState>,
    #[serde(default)]
    pub behavior_summary: String,
    #[serde(default)]
    pub source_anchors: Vec<String>,
    #[serde(default)]
    pub figures: Vec<String>,
    #[serde(default)]
    pub sub_blocks: Vec<String>,
}

/// One row of a Block I/O signal table (four-column form).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockSignalRow {
    pub name: String,
    pub direction: String,
    #[serde(default)]
    pub peer: String,
    #[serde(default)]
    pub description: String,
}

/// One bullet under a Block's `#### State` subsection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockState {
    pub name: String,
    #[serde(default)]
    pub width: String,
    #[serde(default)]
    pub reset_value: String,
    #[serde(default)]
    pub description: String,
}

/// One row of the `## Parameters` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type", default)]
    pub ty: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub valid_range: String,
    #[serde(default)]
    pub behavioral_impact: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// One entry under `## State Machines`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateMachine {
    pub name: String,
    #[serde(default)]
    pub reset_state: String,
    #[serde(default)]
    pub source_anchor: String,
    #[serde(default)]
    pub states: Vec<FsmState>,
    #[serde(default)]
    pub transitions: Vec<FsmTransition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsmState {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsmTransition {
    pub from: String,
    pub input: String,
    pub to: String,
    #[serde(default)]
    pub output: String,
}

/// One entry under `## Encodings`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Encoding {
    pub field: String,
    #[serde(default)]
    pub bit_width: String,
    #[serde(default)]
    pub source_anchor: String,
    #[serde(default)]
    pub values: Vec<EncodingValue>,
    /// Free-form text describing reserved / illegal encodings (often
    /// just `none`).
    #[serde(default)]
    pub reserved: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodingValue {
    pub value: String,
    pub name: String,
    #[serde(default)]
    pub abbreviation: String,
}

/// One row of the `## Memory Map` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRegion {
    pub start: String,
    pub end: String,
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub access: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// `## Connectivity` section: nodes + edges + routing-rules prose.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Connectivity {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub routing_rules: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type", default)]
    pub ty: String,
    #[serde(default)]
    pub coordinate: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// One row of the `## Error Handling` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub error_type: String,
    #[serde(default)]
    pub detecting_component: String,
    #[serde(default)]
    pub detection_behavior: String,
    #[serde(default)]
    pub bus_response: String,
    #[serde(default)]
    pub master_behavior: String,
    #[serde(default)]
    pub software_response: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// `## Functional Behavior` section: three prose / list subsections.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionalBehavior {
    #[serde(default)]
    pub end_to_end: String,
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default)]
    pub data_movement: String,
}

/// One numbered entry in the Operation flow list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub source_anchor: String,
}

/// `## Timing, Latency, and Throughput` section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimingAndThroughput {
    #[serde(default)]
    pub latency: Vec<LatencyRow>,
    #[serde(default)]
    pub throughput: String,
    #[serde(default)]
    pub stall_and_backpressure: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyRow {
    pub operation: String,
    pub best_case: String,
    pub worst_case: String,
    #[serde(default)]
    pub notes: String,
}

/// `## Pipeline and Hierarchy` — single short prose summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PipelineAndHierarchy {
    #[serde(default)]
    pub prose: String,
}

/// `## Reset, Initialization, Flush, Drain` — three short prose
/// subsections.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResetInitFlushDrain {
    #[serde(default)]
    pub reset: String,
    #[serde(default)]
    pub initialization: String,
    #[serde(default)]
    pub flush_and_drain: String,
}

/// One scenario under `## Cycle-Accurate Behavior`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CycleAccurateScenario {
    pub name: String,
    /// The cycle-by-cycle table's column headers (e.g.
    /// `["Cycle", "IF", "PD", ...]`).
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<CycleAccurateRow>,
    #[serde(default)]
    pub source_anchor: String,
}

/// One row of a cycle-accurate scenario table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleAccurateRow {
    /// Cell values, one per column declared on the scenario.
    pub cells: Vec<String>,
}

/// One entry under `## Figures`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FigureEntry {
    pub name: String,
    #[serde(default)]
    pub source_page: String,
    #[serde(default)]
    pub raster: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub referenced_blocks: Vec<String>,
    #[serde(default)]
    pub caption: String,
    #[serde(default)]
    pub elements: Vec<FigureElement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FigureElement {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub notes: String,
}

/// One entry under `## Worked Examples`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkedExample {
    pub name: String,
    #[serde(default)]
    pub inputs: String,
    #[serde(default)]
    pub expected_flow: String,
    #[serde(default)]
    pub expected_outputs: String,
}

/// One row of the `## Source-Spec Anchors` index table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnchorIndexEntry {
    pub section_path: String,
    pub source: String,
    pub chunk_id: String,
    #[serde(default)]
    pub page_range: String,
}

/// Parsed form of a source-spec anchor (see §2.4). Three forms:
/// page, page-range, chunk. `source` is `primary` or a peer ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceSpecAnchor {
    Page {
        source: String,
        page: u32,
    },
    PageRange {
        source: String,
        start: u32,
        end: u32,
    },
    Chunk {
        source: String,
        chunk: String,
    },
}

/// One bullet under `## Open Questions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub text: String,
}

/// One bullet under `## Auto-decisions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoDecision {
    pub decision: String,
    pub rationale: String,
}
