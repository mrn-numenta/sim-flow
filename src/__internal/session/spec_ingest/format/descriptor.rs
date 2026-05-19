//! Rust types mirroring the `format.json` schema (Chapter 7 §7.3).
//!
//! Every field carries serde attributes so JSON values produced by
//! the discovery pipeline (or hand-edited by the operator) round-trip
//! through these types without an intermediate untyped value. The
//! tagged-enum variants for `spec_md_role`, table targets, and figure
//! targets follow the externally-tagged shape from §7.3 — `{"kind":
//! "...", ...extra-fields}` — driven by `#[serde(tag = "kind",
//! rename_all = "snake_case")]`.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Top-level `format.json` descriptor (Chapter 7 §7.3.1).
///
/// `schema_version` is pinned to `1` in the v1 schema; callers detect
/// a version skew with [`FormatJson::current_schema_version`] after
/// loading. The remaining metadata fields (`model`, `prompt_version`,
/// `source_sha256`, `discovered_at`) make the descriptor
/// content-addressable so callers can skip re-discovery on cache hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatJson {
    pub schema_version: u32,
    pub model: String,
    pub prompt_version: String,
    pub source_sha256: String,
    pub discovered_at: DateTime<Utc>,
    #[serde(default)]
    pub section_roles: Vec<SectionRoleEntry>,
    #[serde(default)]
    pub tables: Vec<TableEntry>,
    #[serde(default)]
    pub figures: Vec<FigureEntry>,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    pub chrome: Vec<ChromeEntry>,
    #[serde(default)]
    pub validation: ValidationBlock,
}

impl FormatJson {
    /// Schema version this build of sim-flow understands. Persisted
    /// in every descriptor written by [`FormatJson::write`]; callers
    /// loading a descriptor compare against this to detect skew.
    pub const fn current_schema_version() -> u32 {
        1
    }

    /// Read + deserialise a `format.json` from disk. Wraps the
    /// underlying I/O and JSON parse errors in the crate's
    /// [`Error::State`] variant so callers can surface a single
    /// failure mode.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| Error::State(format!("format.json: read {}: {e}", path.display())))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::State(format!("format.json: parse {}: {e}", path.display())))
    }

    /// Serialise + write a `format.json` to disk, pretty-printed for
    /// human review.
    pub fn write(&self, path: &Path) -> Result<()> {
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::State(format!("format.json: serialize {}: {e}", path.display())))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::State(format!("format.json: mkdir {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(path, body)
            .map_err(|e| Error::State(format!("format.json: write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Tuple identifying the descriptor's content origin. The
    /// downstream ingest pipeline keys its cache on this so a
    /// `(source_sha256, model, prompt_version)` match short-circuits
    /// re-discovery.
    pub fn content_key(&self) -> ContentKey {
        ContentKey {
            source_sha256: self.source_sha256.clone(),
            model: self.model.clone(),
            prompt_version: self.prompt_version.clone(),
        }
    }
}

/// Cache key tying a `format.json` to the LLM + prompt that produced
/// it and the SHA-256 of the source document. Compared field-wise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentKey {
    pub source_sha256: String,
    pub model: String,
    pub prompt_version: String,
}

/// One detected section heading (Chapter 7 §7.3.2). Pairs the PDF
/// origin (`page`, `line`, `font_size`, `font_weight`) with the
/// classifier's role + layer assignment plus a rationale string the
/// LLM critique pass populates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionRoleEntry {
    pub heading: String,
    pub page: u32,
    pub line: u32,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub level: u8,
    pub spec_md_role: SpecMdRole,
    pub layer: Layer,
    #[serde(default)]
    pub rationale: String,
}

/// Font weight as reported by pdf_oxide's span metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontWeight {
    Normal,
    Bold,
}

/// Architectural layer a section describes (Chapter 7 §7.3.2).
/// `architectural` covers software-visible behavior; `micro` covers
/// implementation. `mixed` is for sections that span both; `unknown`
/// means neither pass committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Architectural,
    Micro,
    Mixed,
    Unknown,
}

/// spec_md role assigned to a detected section heading (Chapter 7
/// §7.3.2). Externally tagged on `kind` with optional context fields
/// per variant so JSON looks like `{"kind": "block", "block_name":
/// "..."}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpecMdRole {
    Metadata,
    Assumptions,
    ExternalInterfaces,
    Block { block_name: String },
    Parameters,
    Csrs,
    CsrFields { csr_name: String },
    RegisterFiles,
    MemoryMap,
    StateMachines,
    Encodings,
    Connectivity,
    Errors,
    FunctionalBehavior,
    TimingAndThroughput,
    PipelineAndHierarchy,
    ResetInitFlushDrain,
    WorkedExamples,
    Glossary,
    ClockDomains,
    PowerDomains,
    ResetDomains,
    SecurityBoundaries,
    NumericalConventions,
    PerformanceCounters,
    Prose,
    Unknown,
}

/// One detected table (Chapter 7 §7.3.3). pdf_oxide reports `(page,
/// first_line, row_count, col_count)` deterministically; `kind`,
/// `spec_md_target`, `column_map`, and `wrap_strategy` are assigned
/// by the first-cut classifier and refined by the LLM critique pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableEntry {
    pub id: String,
    pub page: u32,
    pub first_line: u32,
    pub row_count: u32,
    pub col_count: u32,
    pub kind: TableKind,
    pub spec_md_target: TableTarget,
    #[serde(default)]
    pub column_map: Vec<ColumnMapping>,
    pub wrap_strategy: WrapStrategy,
    #[serde(default)]
    pub rationale: String,
}

/// Recognised table kinds (Chapter 7 §7.3.3). `Unknown` is the
/// fall-through for tables neither the first-cut classifier nor the
/// LLM committed on; DM0 surfaces those via `ask_user`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    SignalTable,
    ExternalSignalTable,
    ParameterTable,
    CsrTable,
    CsrFieldTable,
    RegisterFileTable,
    MemoryMapTable,
    EncodingTable,
    ErrorTable,
    FsmStateTable,
    FsmTransitionTable,
    LatencyTable,
    ConnectivityTable,
    PmuEventTable,
    Unknown,
}

/// spec_md target a classified table feeds into. Externally tagged
/// on `kind`; variants carrying a parent (block, CSR, FSM, encoding)
/// name it inline so classify.rs can route rows without re-inferring
/// ownership from headings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TableTarget {
    BlockSignals { block_name: String },
    ExternalSignals,
    Parameters,
    Csrs,
    CsrFields { csr_name: String },
    MemoryMap,
    StateMachineStates { fsm_name: String },
    StateMachineTransitions { fsm_name: String },
    Encoding { encoding_name: String },
    Errors,
    ConnectivityNodes,
    ConnectivityEdges,
    TimingLatency,
    PmuEvents,
    Unknown,
}

/// One column projection from a PDF table's source header text to a
/// canonical spec_md row field name. The deterministic post-pass
/// validates `canonical` against the target's row schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnMapping {
    pub source: String,
    pub canonical: String,
}

/// Strategy for coalescing pdf_oxide's per-cell rows when a logical
/// row's text wraps across multiple `TableRow`s. Chapter 7 §7.3.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapStrategy {
    SingleRow,
    MergeContinuationRows,
    JoinOnBlankFirstCol,
}

/// One detected figure (Chapter 7 §7.3.4). Every detected figure is
/// rasterised regardless of classification; `kind` and
/// `spec_md_target` drive retrieval + DM0 auto-populate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FigureEntry {
    pub id: String,
    pub page: u32,
    pub kind: FigureKind,
    pub rasterized_to: String,
    pub spec_md_target: FigureTarget,
    #[serde(default)]
    pub referenced_acronyms: Vec<String>,
    #[serde(default)]
    pub rationale: String,
}

/// Recognised figure kinds (Chapter 7 §7.3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FigureKind {
    BlockDiagram,
    StateDiagram,
    TimingDiagram,
    MemoryMapDiagram,
    ConnectivityTopology,
    PipelineDiagram,
    Generic,
}

/// spec_md target a classified figure attaches to. Mirrors the
/// figure-kind enumeration but carries optional parent context for
/// the variants that need it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FigureTarget {
    BlockDiagram { block_name: String },
    StateDiagram { fsm_name: String },
    TimingDiagram,
    MemoryMapDiagram,
    ConnectivityTopology,
    PipelineDiagram,
    Generic,
}

/// One glossary entry (Chapter 7 §7.3.5). `source` records where the
/// detector found the expansion so the LLM critique pass can prefer
/// `glossary_section` entries over parenthesised first-mentions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub acronym: String,
    pub expansion: String,
    pub first_page: u32,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub used_in_blocks: Vec<String>,
    pub source: GlossarySource,
}

/// Provenance of a glossary entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossarySource {
    ParenthesisedFirstMention,
    GlossarySection,
    UserAdded,
}

/// One chrome (running header / footer / page number / footer link /
/// watermark) regex (Chapter 7 §7.3.6). `y_band_pt` is the optional
/// positional band the detector pinned the line to; chrome-strip
/// applies positional + regex filters together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromeEntry {
    pub regex: String,
    pub kind: ChromeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_band_pt: Option<[f32; 2]>,
    pub match_count: u32,
}

/// Recognised chrome kinds (Chapter 7 §7.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeKind {
    RunningHeader,
    RunningFooter,
    PageNumber,
    FooterLink,
    Watermark,
}

/// Validation block filled by the deterministic post-pass (Chapter 7
/// §7.3.7). `tables_classified` is a kind-name → count map so the
/// JSON shape matches §7.3.7 verbatim (`{"signal_table": 6, ...}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ValidationBlock {
    #[serde(default)]
    pub section_roles_assigned: u32,
    #[serde(default)]
    pub tables_classified: BTreeMap<String, u32>,
    #[serde(default)]
    pub tables_unknown: u32,
    #[serde(default)]
    pub glossary_entries: u32,
    #[serde(default)]
    pub chrome_lines_stripped: u32,
    #[serde(default)]
    pub warnings: Vec<ValidationWarning>,
}

/// One validation warning (Chapter 7 §7.9). `code` is a stable string
/// identifier (e.g. `wrap_strategy_zero_merges`); `table_id` /
/// `section_id` localise the warning when applicable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationWarning {
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_descriptor() -> FormatJson {
        let mut tables_classified = BTreeMap::new();
        tables_classified.insert("signal_table".to_string(), 1);
        FormatJson {
            schema_version: 1,
            model: "claude-sonnet-4-6".to_string(),
            prompt_version: "2026-05-19".to_string(),
            source_sha256: "deadbeef".to_string(),
            discovered_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
            section_roles: vec![SectionRoleEntry {
                heading: "Instruction Fetch (IF)".to_string(),
                page: 11,
                line: 5,
                font_size: 14.7,
                font_weight: FontWeight::Bold,
                level: 2,
                spec_md_role: SpecMdRole::Block {
                    block_name: "Instruction Fetch (IF)".to_string(),
                },
                layer: Layer::Micro,
                rationale: "matches pipeline-stage acronym pattern".to_string(),
            }],
            tables: vec![TableEntry {
                id: "tbl_023".to_string(),
                page: 12,
                first_line: 17,
                row_count: 9,
                col_count: 4,
                kind: TableKind::SignalTable,
                spec_md_target: TableTarget::BlockSignals {
                    block_name: "Instruction Fetch (IF)".to_string(),
                },
                column_map: vec![
                    ColumnMapping {
                        source: "Signal".to_string(),
                        canonical: "name".to_string(),
                    },
                    ColumnMapping {
                        source: "Direction".to_string(),
                        canonical: "direction".to_string(),
                    },
                ],
                wrap_strategy: WrapStrategy::MergeContinuationRows,
                rationale: "column headers match signal-table convention".to_string(),
            }],
            figures: vec![FigureEntry {
                id: "fig_005".to_string(),
                page: 13,
                kind: FigureKind::BlockDiagram,
                rasterized_to: "figures/page-013.png".to_string(),
                spec_md_target: FigureTarget::BlockDiagram {
                    block_name: "Instruction Fetch (IF)".to_string(),
                },
                referenced_acronyms: vec!["IF".to_string(), "PD".to_string()],
                rationale: "vector-path page with stage labels".to_string(),
            }],
            glossary: vec![GlossaryEntry {
                acronym: "IF".to_string(),
                expansion: "Instruction Fetch".to_string(),
                first_page: 11,
                scope: "spec".to_string(),
                used_in_blocks: vec!["Instruction Fetch (IF)".to_string()],
                source: GlossarySource::ParenthesisedFirstMention,
            }],
            chrome: vec![ChromeEntry {
                regex: "^RV12 RISC-V.*$".to_string(),
                kind: ChromeKind::RunningHeader,
                y_band_pt: Some([766.0, 774.0]),
                match_count: 95,
            }],
            validation: ValidationBlock {
                section_roles_assigned: 1,
                tables_classified,
                tables_unknown: 0,
                glossary_entries: 1,
                chrome_lines_stripped: 95,
                warnings: vec![ValidationWarning {
                    code: "wrap_strategy_zero_merges".to_string(),
                    message: "wrap strategy never fired".to_string(),
                    table_id: Some("tbl_023".to_string()),
                    section_id: None,
                }],
            },
        }
    }

    /// Hand-authored descriptor serializes to JSON and deserializes
    /// back, equal.
    #[test]
    fn round_trip_value_equality() {
        let value = sample_descriptor();
        let json = serde_json::to_string(&value).expect("serialize");
        let back: FormatJson = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(value, back);
    }

    /// A hand-authored JSON string matching the §7.3.3 example
    /// deserializes into the expected typed shape.
    #[test]
    fn deserializes_signal_table_fixture() {
        let json = r#"{
            "schema_version": 1,
            "model": "claude-sonnet-4-6",
            "prompt_version": "2026-05-19",
            "source_sha256": "abc123",
            "discovered_at": "2026-05-18T12:00:00Z",
            "section_roles": [],
            "tables": [{
                "id": "tbl_023",
                "page": 12,
                "first_line": 17,
                "row_count": 9,
                "col_count": 4,
                "kind": "signal_table",
                "spec_md_target": {
                    "kind": "block_signals",
                    "block_name": "Instruction Fetch (IF)"
                },
                "column_map": [
                    { "source": "Signal", "canonical": "name" },
                    { "source": "Direction", "canonical": "direction" },
                    { "source": "To/From", "canonical": "peer" },
                    { "source": "Description", "canonical": "description" }
                ],
                "wrap_strategy": "merge_continuation_rows",
                "rationale": "column headers match signal-table convention; sits under IF block section"
            }],
            "figures": [],
            "glossary": [],
            "chrome": [],
            "validation": {}
        }"#;
        let parsed: FormatJson = serde_json::from_str(json).expect("deserialize");
        assert_eq!(parsed.tables.len(), 1);
        assert_eq!(parsed.tables[0].kind, TableKind::SignalTable);
        assert_eq!(parsed.tables[0].column_map[0].canonical, "name");
        assert_eq!(
            parsed.tables[0].spec_md_target,
            TableTarget::BlockSignals {
                block_name: "Instruction Fetch (IF)".to_string()
            }
        );
        assert_eq!(
            parsed.tables[0].wrap_strategy,
            WrapStrategy::MergeContinuationRows
        );
    }

    /// A descriptor with `schema_version != 1` still parses, but the
    /// version-skew is detectable against
    /// `FormatJson::current_schema_version()`.
    #[test]
    fn schema_version_mismatch_is_detectable() {
        let json = r#"{
            "schema_version": 99,
            "model": "m",
            "prompt_version": "p",
            "source_sha256": "s",
            "discovered_at": "2026-05-18T12:00:00Z"
        }"#;
        let parsed: FormatJson = serde_json::from_str(json).expect("deserialize");
        assert_eq!(parsed.schema_version, 99);
        assert_ne!(parsed.schema_version, FormatJson::current_schema_version());
    }

    /// `content_key()` returns the `(source_sha256, model,
    /// prompt_version)` tuple.
    #[test]
    fn content_key_returns_expected_tuple() {
        let value = sample_descriptor();
        let key = value.content_key();
        assert_eq!(key.source_sha256, "deadbeef");
        assert_eq!(key.model, "claude-sonnet-4-6");
        assert_eq!(key.prompt_version, "2026-05-19");
    }

    /// Tagged-enum JSON shape: `spec_md_role` variants with context
    /// fields serialize as `{"kind": "block", "block_name": "..."}`.
    #[test]
    fn spec_md_role_tagged_shape() {
        let role = SpecMdRole::Block {
            block_name: "IF".to_string(),
        };
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, r#"{"kind":"block","block_name":"IF"}"#);

        let bare = SpecMdRole::Glossary;
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(json, r#"{"kind":"glossary"}"#);
    }

    /// `load` + `write` round-trips through a tempfile.
    #[test]
    fn load_write_round_trip_via_tempfile() {
        let value = sample_descriptor();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("format.json");
        value.write(&path).expect("write");
        let loaded = FormatJson::load(&path).expect("load");
        assert_eq!(value, loaded);
    }
}
