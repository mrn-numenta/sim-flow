//! First-cut deterministic classifier (Phase 9 milestone 9.4).
//!
//! Walks a [`Skeleton`] (the structural map built in milestone
//! 9.3) and emits a provisional [`FormatJson`] descriptor that
//! the LLM critique pass (milestone 9.5) consumes and refines.
//! The first cut is what the LLM corrects, not what the LLM
//! produces from scratch.
//!
//! The classifier uses a heuristic library of substring patterns
//! for section roles, column-header signatures for tables, and
//! neighbouring-heading inference for figures. Architecture
//! Chapter 7 §7.4 and §7.6 specify the two-pass design; this
//! module is the first (deterministic) pass.

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use regex::Regex;

use super::descriptor::{
    ChromeEntry, ChromeKind, ColumnMapping, FigureEntry, FigureKind, FigureTarget, FontWeight,
    FormatJson, GlossaryEntry, GlossarySource, Layer, SectionRoleEntry, SpecMdRole, TableEntry,
    TableKind, TableTarget, ValidationBlock, WrapStrategy,
};
use super::skeleton::{
    AcronymCandidate, FigureEntry as SkelFigure, HeadingEntry, Skeleton, TableEntry as SkelTable,
};
use crate::session::spec_ingest::stages::loading::FontWeight as LoadingFontWeight;

/// Sentinel `model` value identifying a descriptor produced by
/// the deterministic first-cut classifier. The LLM critique pass
/// overwrites this when it lands; downstream callers can grep
/// for the sentinel to tell pre-critique drafts apart from
/// final descriptors.
pub const FIRST_CUT_MODEL: &str = "first-cut-builtin";

/// Prompt version paired with [`FIRST_CUT_MODEL`]. Bumped when
/// the heuristic library changes in a way that invalidates
/// cached output keyed on
/// `(source_sha256, model, prompt_version)`.
pub const FIRST_CUT_PROMPT_VERSION: &str = "first-cut-v1";

/// Classify a [`Skeleton`] into a provisional [`FormatJson`].
///
/// The returned descriptor's `model` is [`FIRST_CUT_MODEL`] and
/// its `prompt_version` is [`FIRST_CUT_PROMPT_VERSION`]; the LLM
/// critique pass (milestone 9.5) overwrites those when it lands.
/// `source_sha256` is left blank — the CLI fills it in when
/// caching the descriptor next to the input. `validation` is
/// left at [`ValidationBlock::default`] for the deterministic
/// validation post-pass to populate.
pub fn classify(skeleton: &Skeleton) -> FormatJson {
    let section_roles = classify_sections(&skeleton.headings);
    let tables = classify_tables(&skeleton.tables, &skeleton.headings, &section_roles);
    let figures = classify_figures(
        &skeleton.figures,
        &skeleton.headings,
        &section_roles,
        &skeleton.acronym_candidates,
    );
    let glossary = build_glossary(&skeleton.acronym_candidates);
    let chrome = build_chrome(&skeleton.chrome_repeated_lines);

    FormatJson {
        schema_version: FormatJson::current_schema_version(),
        model: FIRST_CUT_MODEL.to_string(),
        prompt_version: FIRST_CUT_PROMPT_VERSION.to_string(),
        source_sha256: String::new(),
        // Epoch zero: the first cut is deterministic and
        // content-agnostic w.r.t. wall-clock time. A fixed
        // timestamp keeps the descriptor reproducible across
        // invocations; the LLM critique pass stamps the real
        // `discovered_at` when it lands.
        discovered_at: epoch_zero(),
        section_roles,
        tables,
        figures,
        glossary,
        chrome,
        validation: ValidationBlock::default(),
    }
}

/// `1970-01-01T00:00:00Z`. Hard-coded so first-cut output is
/// bit-identical across runs.
fn epoch_zero() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(0, 0)
        .single()
        .expect("epoch 0 is a valid UTC instant")
}

/// Map a skeleton heading's `FontWeight` (the layout-stage
/// `LoadingFontWeight`) onto the descriptor's `FontWeight`. Both
/// enums collapse the PDF 100-900 weight scale to Normal vs Bold;
/// this is a structural rebinding only.
fn map_weight(is_bold: bool) -> FontWeight {
    if is_bold {
        FontWeight::Bold
    } else {
        FontWeight::Normal
    }
}

// `LoadingFontWeight` is only referenced for documentation /
// type-clarity; silence the unused-import warning when no
// reference survives optimisation.
#[allow(dead_code)]
const _LOADING_FONT_WEIGHT_REF: Option<LoadingFontWeight> = None;

// ---------------------------------------------------------------
// Section-role classification.
// ---------------------------------------------------------------

/// Walk every heading in the skeleton and emit a
/// `SectionRoleEntry` per heading. Order in the input is
/// preserved (the skeleton already sorts by `(page, line_y)`).
fn classify_sections(headings: &[HeadingEntry]) -> Vec<SectionRoleEntry> {
    headings
        .iter()
        .map(|h| {
            let (role, matched) = classify_heading_role(&h.text);
            let layer = role_to_layer(&role);
            let rationale = match matched {
                Some(pattern) => format!("first-cut heuristic: matched \"{pattern}\""),
                None => "first-cut heuristic: no pattern matched".to_string(),
            };
            SectionRoleEntry {
                heading: h.text.clone(),
                page: h.page,
                line: h.line_y.round().max(0.0) as u32,
                font_size: h.font_size,
                font_weight: map_weight(h.is_bold),
                level: h.level,
                spec_md_role: role,
                layer,
                rationale,
            }
        })
        .collect()
}

/// Pattern entry for the heading-role library: a list of
/// case-insensitive substring keywords and the constructor
/// that produces the matching [`SpecMdRole`].
type RolePattern = (&'static [&'static str], fn(&str) -> SpecMdRole);

/// Decide the [`SpecMdRole`] for a heading from its text.
/// Returns the matched pattern label so callers can build a
/// human-readable rationale.
///
/// Order matters: the first matching pattern wins. The acronym-
/// in-parentheses pattern (e.g. "Instruction Fetch (IF)") is
/// checked after the explicit substring patterns so a heading
/// like "Memory Map (MM)" still routes to `MemoryMap` rather
/// than `Block`.
fn classify_heading_role(heading: &str) -> (SpecMdRole, Option<String>) {
    let lower = heading.to_lowercase();

    // Substring patterns listed in priority order. The first
    // match wins; a heading containing multiple keywords routes
    // to the earliest entry in this table.
    let patterns: &[RolePattern] = &[
        (&["glossary"], |_| SpecMdRole::Glossary),
        (&["memory map", "address map"], |_| SpecMdRole::MemoryMap),
        (
            &["csr", "control and status", "control & status register"],
            |_| SpecMdRole::Csrs,
        ),
        (&["pipeline", "execution pipeline"], |_| {
            SpecMdRole::PipelineAndHierarchy
        }),
        (&["parameters", "configurations", "core parameters"], |_| {
            SpecMdRole::Parameters
        }),
        (&["errors", "exceptions", "exception cause"], |_| {
            SpecMdRole::Errors
        }),
        (&["state machine", "fsm"], |_| SpecMdRole::StateMachines),
        (&["encoding"], |_| SpecMdRole::Encodings),
        (&["reset", "initialization", "init"], |_| {
            SpecMdRole::ResetInitFlushDrain
        }),
        (&["clock domain", "clocks"], |_| SpecMdRole::ClockDomains),
        (&["power domain", "power"], |_| SpecMdRole::PowerDomains),
        (&["reset domain"], |_| SpecMdRole::ResetDomains),
        (&["privilege", "security boundar"], |_| {
            SpecMdRole::SecurityBoundaries
        }),
        (&["numerical convention", "fixed-point", "q-format"], |_| {
            SpecMdRole::NumericalConventions
        }),
        (&["performance counter", "pmu", "perf event"], |_| {
            SpecMdRole::PerformanceCounters
        }),
        (&["external interface", "top-level", "i/o"], |_| {
            SpecMdRole::ExternalInterfaces
        }),
        (&["timing", "throughput", "latency"], |_| {
            SpecMdRole::TimingAndThroughput
        }),
        (
            &["functional behavior", "functional behaviour", "operation"],
            |_| SpecMdRole::FunctionalBehavior,
        ),
        (&["worked example", "scenario"], |_| {
            SpecMdRole::WorkedExamples
        }),
        (&["connectivity", "topology", "noc"], |_| {
            SpecMdRole::Connectivity
        }),
        (&["purpose", "scope", "non-goals", "non goals"], |_| {
            SpecMdRole::Metadata
        }),
        (&["assumption", "constraint", "quantitative"], |_| {
            SpecMdRole::Assumptions
        }),
    ];

    for (keywords, ctor) in patterns {
        for kw in *keywords {
            if lower.contains(kw) {
                return (ctor(heading), Some((*kw).to_string()));
            }
        }
    }

    // Last fallback: `<Name> (<ACR>)` block-stage heading. ACR
    // must be 2-5 upper-case alphanumerics, possibly preceded
    // by a digit. We match against the original text (case-
    // sensitive) so lower-case parenthesised text doesn't false-
    // positive.
    let acr_re = Regex::new(r"\(([A-Z][A-Z0-9]{1,4})\)\s*$").expect("acronym regex");
    if let Some(cap) = acr_re.captures(heading.trim()) {
        let acr = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        return (
            SpecMdRole::Block {
                block_name: heading.to_string(),
            },
            Some(format!("(<ACR>) suffix: {acr}")),
        );
    }

    (SpecMdRole::Unknown, None)
}

/// Layer heuristic per the milestone scope. Architectural-layer
/// sections describe software-visible state; micro-layer
/// sections describe implementation. Most other roles default
/// to `Unknown`.
fn role_to_layer(role: &SpecMdRole) -> Layer {
    match role {
        SpecMdRole::Csrs
        | SpecMdRole::CsrFields { .. }
        | SpecMdRole::Errors
        | SpecMdRole::Encodings
        | SpecMdRole::ExternalInterfaces
        | SpecMdRole::MemoryMap
        | SpecMdRole::SecurityBoundaries => Layer::Architectural,
        SpecMdRole::Block { .. }
        | SpecMdRole::PipelineAndHierarchy
        | SpecMdRole::StateMachines
        | SpecMdRole::ResetInitFlushDrain
        | SpecMdRole::ClockDomains
        | SpecMdRole::PowerDomains
        | SpecMdRole::ResetDomains => Layer::Micro,
        _ => Layer::Unknown,
    }
}

// ---------------------------------------------------------------
// Table classification.
// ---------------------------------------------------------------

/// Classify every detected table by its column-header
/// signature. The descriptor's `first_line` is the table bbox's
/// top Y rounded to the nearest non-negative integer (a rough
/// proxy; the LLM critique pass can refine).
fn classify_tables(
    tables: &[SkelTable],
    headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
) -> Vec<TableEntry> {
    tables
        .iter()
        .map(|t| {
            let lower_headers: Vec<String> = t
                .header_row
                .iter()
                .map(|cell| cell.trim().to_lowercase())
                .collect();
            let (kind, target, pattern_label) =
                classify_table_kind(&lower_headers, t, headings, section_roles);
            let column_map = build_column_map(&t.header_row, kind);
            let rationale = match pattern_label {
                Some(p) => {
                    format!("first-cut heuristic: column headers matched {p}")
                }
                None => "first-cut heuristic: no column-header pattern matched".to_string(),
            };
            TableEntry {
                id: t.id.clone(),
                page: t.page,
                first_line: t.bbox.y.round().max(0.0) as u32,
                row_count: t.row_count,
                col_count: t.col_count,
                kind,
                spec_md_target: target,
                column_map,
                wrap_strategy: WrapStrategy::SingleRow,
                rationale,
            }
        })
        .collect()
}

/// Return `true` iff every needle is present in the
/// case-insensitive `header_cells` collection (treated as a
/// substring-on-each-cell match, since pdf_oxide may include
/// extra punctuation/whitespace inside a header cell).
fn headers_contain_all(header_cells: &[String], needles: &[&str]) -> bool {
    needles.iter().all(|needle| {
        let n = needle.to_lowercase();
        header_cells.iter().any(|cell| cell.contains(&n))
    })
}

/// Return `true` iff at least one of the needles is present in
/// the case-insensitive cell collection.
fn headers_contain_any(header_cells: &[String], needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        let n = needle.to_lowercase();
        header_cells.iter().any(|cell| cell.contains(&n))
    })
}

/// Classify a table's `(kind, target)` from its lower-cased
/// header cells plus contextual heading information. Returns
/// the matched pattern label for the rationale string.
fn classify_table_kind(
    headers: &[String],
    table: &SkelTable,
    skel_headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
) -> (TableKind, TableTarget, Option<String>) {
    // Signal table: signal, direction, peer-column, description.
    if headers_contain_all(headers, &["signal", "direction"])
        && headers_contain_any(headers, &["to/from", "from/to", "peer"])
        && headers_contain_all(headers, &["description"])
    {
        let block_name = nearest_block_name(table, skel_headings, section_roles);
        return (
            TableKind::SignalTable,
            TableTarget::BlockSignals { block_name },
            Some("SignalTable".to_string()),
        );
    }

    // External signal table: signal + direction + description
    // + at least one of (type, i/o). Checked before the bare
    // SignalTable fallback so it takes precedence on top-level
    // I/O sheets.
    if headers_contain_all(headers, &["signal", "direction", "description"])
        && headers_contain_any(headers, &["type", "i/o"])
    {
        return (
            TableKind::ExternalSignalTable,
            TableTarget::ExternalSignals,
            Some("ExternalSignalTable".to_string()),
        );
    }

    // Parameter table (4-col form): parameter / type / default / description.
    if headers_contain_all(headers, &["parameter", "type", "default", "description"]) {
        return (
            TableKind::ParameterTable,
            TableTarget::Parameters,
            Some("ParameterTable (4-col)".to_string()),
        );
    }

    // Parameter table (3-col simpler form): parameter / description.
    if headers_contain_all(headers, &["parameter", "description"]) {
        return (
            TableKind::ParameterTable,
            TableTarget::Parameters,
            Some("ParameterTable (3-col)".to_string()),
        );
    }

    // CSR table: address + name + (reset|reset value).
    if headers_contain_all(headers, &["address", "name"])
        && headers_contain_any(headers, &["reset", "reset value"])
    {
        return (
            TableKind::CsrTable,
            TableTarget::Csrs,
            Some("CsrTable (address/name/reset)".to_string()),
        );
    }

    // CSR table (3-col simpler form): address + name + description.
    if headers_contain_all(headers, &["address", "name", "description"]) {
        return (
            TableKind::CsrTable,
            TableTarget::Csrs,
            Some("CsrTable (address/name/description)".to_string()),
        );
    }

    // CSR field table: bits/bit/field + name + description.
    if headers_contain_any(headers, &["bits", "bit", "field"])
        && headers_contain_all(headers, &["name", "description"])
    {
        let csr_name = nearest_csr_name(table, skel_headings, section_roles);
        return (
            TableKind::CsrFieldTable,
            TableTarget::CsrFields { csr_name },
            Some("CsrFieldTable".to_string()),
        );
    }

    // Memory map: region + (base|start) + size.
    if headers_contain_all(headers, &["region"])
        && headers_contain_any(headers, &["base", "start"])
        && headers_contain_all(headers, &["size"])
    {
        return (
            TableKind::MemoryMapTable,
            TableTarget::MemoryMap,
            Some("MemoryMapTable".to_string()),
        );
    }

    // Error / exception table: exception|interrupt + code + description.
    if headers_contain_any(headers, &["exception", "interrupt"])
        && headers_contain_all(headers, &["code", "description"])
    {
        return (
            TableKind::ErrorTable,
            TableTarget::Errors,
            Some("ErrorTable".to_string()),
        );
    }

    // FSM transition table: state + (transition|next|next state).
    if headers_contain_all(headers, &["state"])
        && headers_contain_any(headers, &["transition", "next", "next state"])
    {
        let fsm_name = nearest_heading_text(table, skel_headings);
        return (
            TableKind::FsmTransitionTable,
            TableTarget::StateMachineTransitions { fsm_name },
            Some("FsmTransitionTable".to_string()),
        );
    }

    // FSM state table (3-col simpler form): state + description.
    if headers_contain_all(headers, &["state", "description"]) {
        let fsm_name = nearest_heading_text(table, skel_headings);
        return (
            TableKind::FsmStateTable,
            TableTarget::StateMachineStates { fsm_name },
            Some("FsmStateTable".to_string()),
        );
    }

    // Encoding table: (value|code|level) + (name|encoding) +
    // (description|abbreviation).
    if headers_contain_any(headers, &["value", "code", "level"])
        && headers_contain_any(headers, &["name", "encoding"])
        && headers_contain_any(headers, &["description", "abbreviation"])
    {
        let encoding_name = nearest_heading_text(table, skel_headings);
        return (
            TableKind::EncodingTable,
            TableTarget::Encoding { encoding_name },
            Some("EncodingTable".to_string()),
        );
    }

    // Latency table: operation + (cycles|latency).
    if headers_contain_all(headers, &["operation"])
        && headers_contain_any(headers, &["cycles", "latency"])
    {
        return (
            TableKind::LatencyTable,
            TableTarget::TimingLatency,
            Some("LatencyTable".to_string()),
        );
    }

    // PMU event table: (event|counter) + (id|address).
    if headers_contain_any(headers, &["event", "counter"])
        && headers_contain_any(headers, &["id", "address"])
    {
        return (
            TableKind::PmuEventTable,
            TableTarget::PmuEvents,
            Some("PmuEventTable".to_string()),
        );
    }

    (TableKind::Unknown, TableTarget::Unknown, None)
}

/// Find the text of the most-recent heading that precedes the
/// table's `(page, line_y)` position. Uses the table bbox's
/// top-of-box Y for the line ordering.
fn nearest_heading_text(table: &SkelTable, headings: &[HeadingEntry]) -> String {
    let table_y = table.bbox.y;
    let mut best: Option<&HeadingEntry> = None;
    for h in headings {
        let earlier_page = h.page < table.page;
        let same_page_above = h.page == table.page && h.line_y > table_y;
        if earlier_page || same_page_above {
            best = match best {
                None => Some(h),
                Some(prev) => {
                    let prev_key = (prev.page, ordered_y(prev.line_y));
                    let cand_key = (h.page, ordered_y(h.line_y));
                    if cand_key > prev_key {
                        Some(h)
                    } else {
                        Some(prev)
                    }
                }
            };
        }
    }
    best.map(|h| h.text.clone()).unwrap_or_default()
}

/// Sort key helper so we can pick the "latest preceding"
/// heading. Y grows toward the top of the page, so a heading
/// with a smaller Y is later in reading order on the same page.
fn ordered_y(y: f32) -> i32 {
    // Bigger Y = earlier in reading order, so we negate so that
    // "later in reading order" maps to a larger key.
    -((y * 100.0) as i32)
}

/// Block name = text of the most-recent heading whose role is
/// `Block`. Returns the empty string if no block heading
/// precedes the table.
fn nearest_block_name(
    table: &SkelTable,
    headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
) -> String {
    nearest_role_heading(table, headings, section_roles, |role| {
        matches!(role, SpecMdRole::Block { .. })
    })
}

/// CSR name = text of the most-recent heading whose role is
/// `Csrs` (i.e. a CSR section). Returns the empty string when
/// no CSR-kind heading precedes the table.
fn nearest_csr_name(
    table: &SkelTable,
    headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
) -> String {
    nearest_role_heading(table, headings, section_roles, |role| {
        matches!(role, SpecMdRole::Csrs)
    })
}

/// Generic "most-recent heading of role X before this table"
/// helper. Pairs each heading with its classified role and
/// applies the caller's predicate. Returns the empty string
/// when nothing matches.
fn nearest_role_heading(
    table: &SkelTable,
    headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
    pred: impl Fn(&SpecMdRole) -> bool,
) -> String {
    let table_y = table.bbox.y;
    let mut best: Option<(&HeadingEntry, &SpecMdRole)> = None;
    for (h, entry) in headings.iter().zip(section_roles.iter()) {
        if !pred(&entry.spec_md_role) {
            continue;
        }
        let earlier_page = h.page < table.page;
        let same_page_above = h.page == table.page && h.line_y > table_y;
        if earlier_page || same_page_above {
            best = match best {
                None => Some((h, &entry.spec_md_role)),
                Some((prev, _)) => {
                    let prev_key = (prev.page, ordered_y(prev.line_y));
                    let cand_key = (h.page, ordered_y(h.line_y));
                    if cand_key > prev_key {
                        Some((h, &entry.spec_md_role))
                    } else {
                        Some((prev, &entry.spec_md_role))
                    }
                }
            };
        }
    }
    best.map(|(h, _)| h.text.clone()).unwrap_or_default()
}

/// Project the table's raw header cells onto canonical spec_md
/// row-field names. Unmapped columns are skipped (no
/// `ColumnMapping` is emitted) so the LLM critique can fill in
/// novel column meanings without us guessing wrong.
///
/// Every emitted canonical is validated against
/// `valid_canonicals_for_kind` (the same list classify.rs uses) so
/// the first cut never proposes a canonical that the classifier
/// will reject. This eliminates the `classify_unknown_canonical`
/// warning class for first-cut output.
pub(super) fn build_column_map(headers: &[String], kind: TableKind) -> Vec<ColumnMapping> {
    let dict: &[(&[&str], &str)] = canonical_dict_for(kind);
    if dict.is_empty() {
        return Vec::new();
    }
    let valid = valid_canonicals_for_kind(kind);
    headers
        .iter()
        .filter_map(|cell| {
            let lower = cell.trim().to_lowercase();
            for (synonyms, canonical) in dict {
                for syn in *synonyms {
                    if lower == *syn || lower.contains(syn) {
                        if !valid.contains(canonical) {
                            // The synonym dict points at a
                            // canonical that doesn't exist on the
                            // target row schema (e.g. an enum
                            // value we renamed). Skip rather than
                            // emit a classify-time warning.
                            return None;
                        }
                        return Some(ColumnMapping {
                            source: cell.clone(),
                            canonical: (*canonical).to_string(),
                        });
                    }
                }
            }
            None
        })
        .collect()
}

/// Canonical field names allowed for each `TableKind`'s row schema.
/// Mirrors `classify.rs::canonical_fields_for_kind` (which is
/// private to that module). Kept in lockstep so the first-cut
/// column map only proposes canonicals classify.rs will accept.
pub(super) fn valid_canonicals_for_kind(kind: TableKind) -> &'static [&'static str] {
    match kind {
        TableKind::SignalTable => &["name", "direction", "peer", "description", "role"],
        TableKind::ExternalSignalTable => &[
            "name",
            "direction",
            "width",
            "type",
            "required",
            "description",
        ],
        TableKind::ParameterTable => &[
            "name",
            "type",
            "default",
            "valid_range",
            "behavioral_impact",
        ],
        TableKind::CsrTable => &[
            "address",
            "name",
            "access",
            "reset_value",
            "required_privilege",
            "description",
        ],
        TableKind::CsrFieldTable => &["bits", "name", "access", "description"],
        TableKind::RegisterFileTable => &["name", "width", "count", "description"],
        TableKind::MemoryMapTable => &[
            "start",
            "end",
            "name",
            "purpose",
            "access",
            "required_privilege",
        ],
        TableKind::EncodingTable => &["value", "name", "abbreviation"],
        TableKind::ErrorTable => &[
            "error_type",
            "detecting_component",
            "detection_behavior",
            "bus_response",
            "master_behavior",
            "software_response",
        ],
        TableKind::FsmStateTable => &["name", "description"],
        TableKind::FsmTransitionTable => &["from", "input", "to", "output"],
        TableKind::LatencyTable => &["operation", "best_case", "worst_case", "notes"],
        TableKind::ConnectivityTable => {
            &["id", "type", "coordinate", "role", "from", "to", "channel"]
        }
        TableKind::PmuEventTable => &["id", "name", "description", "csr_address"],
        TableKind::Unknown => &[],
    }
}

/// Per-kind canonical-word dictionaries: `(synonyms[], canonical)`.
/// The classifier scans each header cell against the synonyms in
/// declaration order; the first match wins per cell. Synonyms
/// are lower-case so the matcher can compare against
/// `cell.to_lowercase()` directly.
fn canonical_dict_for(kind: TableKind) -> &'static [(&'static [&'static str], &'static str)] {
    match kind {
        TableKind::SignalTable => &[
            (&["signal"], "name"),
            (&["direction"], "direction"),
            (&["to/from", "from/to", "peer"], "peer"),
            (&["description"], "description"),
        ],
        TableKind::ExternalSignalTable => &[
            (&["signal"], "name"),
            (&["direction"], "direction"),
            (&["type"], "type"),
            (&["i/o"], "io"),
            (&["description"], "description"),
        ],
        TableKind::ParameterTable => &[
            (&["parameter"], "name"),
            (&["type", "kind"], "type"),
            (&["default", "value"], "default"),
            (&["description"], "description"),
        ],
        TableKind::CsrTable => &[
            (&["address", "offset"], "address"),
            (&["name"], "name"),
            (&["access"], "access"),
            (&["reset value", "reset"], "reset_value"),
            (&["privilege"], "required_privilege"),
            (&["description"], "description"),
        ],
        TableKind::CsrFieldTable => &[
            (&["bits", "bit", "field"], "bits"),
            (&["name"], "name"),
            (&["access"], "access"),
            (&["description"], "description"),
        ],
        TableKind::MemoryMapTable => &[
            (&["region"], "name"),
            (&["base", "start"], "start"),
            (&["end", "limit", "top"], "end"),
            (&["purpose", "description"], "purpose"),
            (&["access"], "access"),
            (&["privilege"], "required_privilege"),
        ],
        TableKind::EncodingTable => &[
            (&["value", "code", "level"], "value"),
            (&["name", "encoding"], "name"),
            (&["abbreviation", "abbrev"], "abbreviation"),
        ],
        TableKind::ErrorTable => &[
            // ErrorEntry's canonical columns are detector-side and
            // response-side. The first cut covers the cases where
            // an obvious "exception"/"interrupt" header column is
            // the error type; the LLM critique fills in the rest
            // for richer error tables.
            (&["exception", "interrupt", "error"], "error_type"),
            (&["detecting component", "detector"], "detecting_component"),
            (&["behavior", "behaviour"], "detection_behavior"),
            (&["bus"], "bus_response"),
            (&["master"], "master_behavior"),
            (&["software", "sw"], "software_response"),
        ],
        TableKind::FsmStateTable => &[(&["state"], "name"), (&["description"], "description")],
        TableKind::FsmTransitionTable => &[
            (&["state"], "from"),
            (&["transition", "next state", "next"], "to"),
            (&["input", "trigger", "condition"], "input"),
            (&["output", "action"], "output"),
        ],
        TableKind::LatencyTable => &[
            (&["operation"], "name"),
            (&["cycles", "latency"], "cycles"),
            (&["description"], "description"),
        ],
        TableKind::PmuEventTable => &[
            (&["event", "counter"], "name"),
            (&["id", "address"], "id"),
            (&["description"], "description"),
        ],
        _ => &[],
    }
}

// ---------------------------------------------------------------
// Figure classification.
// ---------------------------------------------------------------

/// Classify each detected figure by its neighbouring heading's
/// role and walk the acronym candidates to find references in
/// the heading text.
fn classify_figures(
    figures: &[SkelFigure],
    headings: &[HeadingEntry],
    section_roles: &[SectionRoleEntry],
    acronyms: &[AcronymCandidate],
) -> Vec<FigureEntry> {
    // Build a heading-text → role index so we can resolve the
    // figure's neighbouring heading to a role without a fresh
    // classification pass. This preserves the priority order of
    // `classify_heading_role` so e.g. "Memory Map (MM)" still
    // routes to MemoryMap rather than Block.
    let mut role_by_heading: BTreeMap<&str, &SpecMdRole> = BTreeMap::new();
    for (h, entry) in headings.iter().zip(section_roles.iter()) {
        role_by_heading.insert(h.text.as_str(), &entry.spec_md_role);
    }

    figures
        .iter()
        .map(|f| {
            let neighbour = f.neighbouring_heading.clone().unwrap_or_default();
            let role = role_by_heading.get(neighbour.as_str()).copied();
            let (kind, target) = figure_kind_and_target(role, &neighbour);
            let referenced = acronyms
                .iter()
                .filter(|a| !a.acronym.is_empty() && neighbour.contains(&a.acronym))
                .map(|a| a.acronym.clone())
                .collect();
            let rationale = match role {
                Some(_) => "first-cut heuristic: neighbouring heading role".to_string(),
                None => "first-cut heuristic: no neighbouring heading role".to_string(),
            };
            FigureEntry {
                id: f.id.clone(),
                page: f.page,
                kind,
                rasterized_to: f.raster_path.clone(),
                spec_md_target: target,
                referenced_acronyms: referenced,
                rationale,
            }
        })
        .collect()
}

/// Decide `(FigureKind, FigureTarget)` for a figure given the
/// resolved role of its neighbouring heading and the heading
/// text (used to populate the per-variant parent names).
fn figure_kind_and_target(
    role: Option<&SpecMdRole>,
    neighbour: &str,
) -> (FigureKind, FigureTarget) {
    match role {
        Some(SpecMdRole::Block { .. }) => (
            FigureKind::BlockDiagram,
            FigureTarget::BlockDiagram {
                block_name: neighbour.to_string(),
            },
        ),
        Some(SpecMdRole::StateMachines) => (
            FigureKind::StateDiagram,
            FigureTarget::StateDiagram {
                fsm_name: neighbour.to_string(),
            },
        ),
        Some(SpecMdRole::MemoryMap) => {
            (FigureKind::MemoryMapDiagram, FigureTarget::MemoryMapDiagram)
        }
        Some(SpecMdRole::Connectivity) => (
            FigureKind::ConnectivityTopology,
            FigureTarget::ConnectivityTopology,
        ),
        Some(SpecMdRole::PipelineAndHierarchy) => {
            (FigureKind::PipelineDiagram, FigureTarget::PipelineDiagram)
        }
        Some(SpecMdRole::TimingAndThroughput) => {
            (FigureKind::TimingDiagram, FigureTarget::TimingDiagram)
        }
        _ => (FigureKind::Generic, FigureTarget::Generic),
    }
}

// ---------------------------------------------------------------
// Glossary + chrome.
// ---------------------------------------------------------------

/// Emit one `GlossaryEntry` per detected acronym candidate.
/// `used_in_blocks` is left empty in v1 — the LLM critique pass
/// can populate it once it sees block-section boundaries; the
/// deterministic first cut doesn't try to attribute acronyms.
fn build_glossary(acronyms: &[AcronymCandidate]) -> Vec<GlossaryEntry> {
    acronyms
        .iter()
        .map(|a| GlossaryEntry {
            acronym: a.acronym.clone(),
            expansion: a.expansion.clone(),
            first_page: a.first_page,
            scope: "spec".to_string(),
            used_in_blocks: Vec::new(),
            source: GlossarySource::ParenthesisedFirstMention,
        })
        .collect()
}

/// Emit one `ChromeEntry` per repeated chrome line. The
/// `regex` field is the literal-escaped line text anchored
/// with `^...$`; `kind` is decided heuristically.
fn build_chrome(chrome_lines: &[String]) -> Vec<ChromeEntry> {
    let page_re = Regex::new(r"page\s+\d+|\d+\s+of\s+\d+").expect("page-number regex");
    chrome_lines
        .iter()
        .map(|line| {
            let lower = line.to_lowercase();
            let kind = if page_re.is_match(&lower) {
                ChromeKind::PageNumber
            } else if lower.trim_start().starts_with("http")
                || lower.contains("http://")
                || lower.contains("https://")
            {
                ChromeKind::FooterLink
            } else {
                ChromeKind::RunningHeader
            };
            ChromeEntry {
                regex: format!("^{}$", regex::escape(line)),
                kind,
                y_band_pt: None,
                match_count: 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_ingest::stages::loading::BBox;

    fn skel_heading(page: u32, text: &str, level: u8, line_y: f32) -> HeadingEntry {
        HeadingEntry {
            page,
            level,
            text: text.to_string(),
            font_size: 14.7,
            is_bold: true,
            line_y,
            cluster_id: (level as u32).saturating_sub(1),
        }
    }

    fn skel_table(
        id: &str,
        page: u32,
        header: &[&str],
        first_row: &[&str],
        bbox_y: f32,
    ) -> SkelTable {
        let header_row: Vec<String> = header.iter().map(|s| s.to_string()).collect();
        let first_data_row: Vec<String> = first_row.iter().map(|s| s.to_string()).collect();
        let col_count = header.len() as u32;
        SkelTable {
            id: id.to_string(),
            page,
            row_count: 2,
            col_count,
            header_row,
            first_data_row,
            bbox: BBox {
                x: 50.0,
                y: bbox_y,
                w: 400.0,
                h: 80.0,
            },
        }
    }

    fn skel_figure(id: &str, page: u32, neighbour: Option<&str>) -> SkelFigure {
        SkelFigure {
            id: id.to_string(),
            page,
            raster_path: format!("figures/page-{page:03}.png"),
            neighbouring_heading: neighbour.map(|s| s.to_string()),
            vector_path_count: 50,
            embedded_image_count: 1,
        }
    }

    fn empty_skeleton() -> Skeleton {
        Skeleton {
            document: super::super::skeleton::DocumentSummary {
                total_pages: 0,
                font_clusters: Vec::new(),
                source_kind: "pdf".to_string(),
            },
            headings: Vec::new(),
            tables: Vec::new(),
            figures: Vec::new(),
            acronym_candidates: Vec::new(),
            chrome_repeated_lines: Vec::new(),
        }
    }

    #[test]
    fn block_section_signal_table_and_block_diagram() {
        // Block-kind heading "Instruction Fetch (IF)" sits at
        // line_y=700; a signal table follows at bbox_y=500 (same
        // page, lower on page in PDF coords); a figure with the
        // same neighbouring heading completes the case.
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(1, "Instruction Fetch (IF)", 2, 700.0)];
        skel.tables = vec![skel_table(
            "T01",
            1,
            &["Signal", "Direction", "To/From", "Description"],
            &["if_pc", "out", "Bus", "Next PC"],
            500.0,
        )];
        skel.figures = vec![skel_figure("F01", 1, Some("Instruction Fetch (IF)"))];

        let fc = classify(&skel);

        assert_eq!(fc.model, "first-cut-builtin");
        assert_eq!(fc.prompt_version, "first-cut-v1");

        assert_eq!(fc.section_roles.len(), 1);
        let s = &fc.section_roles[0];
        assert_eq!(
            s.spec_md_role,
            SpecMdRole::Block {
                block_name: "Instruction Fetch (IF)".to_string()
            }
        );
        assert_eq!(s.layer, Layer::Micro);
        assert_eq!(s.heading, "Instruction Fetch (IF)");
        assert_eq!(s.font_weight, FontWeight::Bold);

        assert_eq!(fc.tables.len(), 1);
        let t = &fc.tables[0];
        assert_eq!(t.id, "T01");
        assert_eq!(t.kind, TableKind::SignalTable);
        assert_eq!(
            t.spec_md_target,
            TableTarget::BlockSignals {
                block_name: "Instruction Fetch (IF)".to_string()
            }
        );
        assert_eq!(t.wrap_strategy, WrapStrategy::SingleRow);
        // column_map should project Signal/Direction/To/From/Description.
        let canonicals: Vec<&str> = t.column_map.iter().map(|c| c.canonical.as_str()).collect();
        assert!(canonicals.contains(&"name"), "{canonicals:?}");
        assert!(canonicals.contains(&"direction"), "{canonicals:?}");
        assert!(canonicals.contains(&"peer"), "{canonicals:?}");
        assert!(canonicals.contains(&"description"), "{canonicals:?}");

        assert_eq!(fc.figures.len(), 1);
        let f = &fc.figures[0];
        assert_eq!(f.kind, FigureKind::BlockDiagram);
        assert_eq!(
            f.spec_md_target,
            FigureTarget::BlockDiagram {
                block_name: "Instruction Fetch (IF)".to_string()
            }
        );
    }

    #[test]
    fn parameter_table_classifies_to_parameters_target() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(2, "Core Parameters", 1, 700.0)];
        skel.tables = vec![skel_table(
            "T01",
            2,
            &["Parameter", "Type", "Default", "Description"],
            &["JEDEC_BANK", "Integer", "0x0A", "JEDEC Bank ID"],
            500.0,
        )];

        let fc = classify(&skel);
        assert_eq!(fc.tables.len(), 1);
        assert_eq!(fc.tables[0].kind, TableKind::ParameterTable);
        assert_eq!(fc.tables[0].spec_md_target, TableTarget::Parameters);
        // Section is Parameters (matched "core parameters" /
        // "parameters" substring).
        assert_eq!(fc.section_roles[0].spec_md_role, SpecMdRole::Parameters);
    }

    #[test]
    fn csr_table_classifies_to_csrs_target() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(3, "CSR Listing", 1, 700.0)];
        skel.tables = vec![skel_table(
            "T01",
            3,
            &["Address", "Name", "Reset"],
            &["0x000", "mstatus", "0x0"],
            500.0,
        )];

        let fc = classify(&skel);
        assert_eq!(fc.section_roles[0].spec_md_role, SpecMdRole::Csrs);
        assert_eq!(fc.section_roles[0].layer, Layer::Architectural);
        assert_eq!(fc.tables.len(), 1);
        assert_eq!(fc.tables[0].kind, TableKind::CsrTable);
        assert_eq!(fc.tables[0].spec_md_target, TableTarget::Csrs);
    }

    #[test]
    fn glossary_heading_routes_to_glossary() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(50, "Glossary", 1, 700.0)];

        let fc = classify(&skel);
        assert_eq!(fc.section_roles.len(), 1);
        assert_eq!(fc.section_roles[0].spec_md_role, SpecMdRole::Glossary);
        assert_eq!(fc.section_roles[0].layer, Layer::Unknown);
        assert!(
            fc.section_roles[0]
                .rationale
                .contains("matched \"glossary\"")
        );
    }

    #[test]
    fn acronym_candidates_become_glossary_entries() {
        let mut skel = empty_skeleton();
        skel.acronym_candidates = vec![
            AcronymCandidate {
                acronym: "IF".to_string(),
                expansion: "Instruction Fetch".to_string(),
                first_page: 11,
                later_usage_count: 47,
            },
            AcronymCandidate {
                acronym: "PD".to_string(),
                expansion: "Pre-Decode".to_string(),
                first_page: 15,
                later_usage_count: 22,
            },
        ];

        let fc = classify(&skel);
        assert_eq!(fc.glossary.len(), 2);
        for g in &fc.glossary {
            assert_eq!(g.source, GlossarySource::ParenthesisedFirstMention);
            assert!(g.used_in_blocks.is_empty());
            assert_eq!(g.scope, "spec");
        }
        let acrs: Vec<&str> = fc.glossary.iter().map(|g| g.acronym.as_str()).collect();
        assert!(acrs.contains(&"IF"));
        assert!(acrs.contains(&"PD"));
    }

    #[test]
    fn chrome_lines_classify_by_kind() {
        let mut skel = empty_skeleton();
        skel.chrome_repeated_lines = vec![
            "rv12 risc-v 32/64-bit cpu core data sheet".to_string(),
            "page 12".to_string(),
            "https://roalogic.github.io/rv12".to_string(),
        ];

        let fc = classify(&skel);
        assert_eq!(fc.chrome.len(), 3);

        let find_kind = |kind: ChromeKind| -> &ChromeEntry {
            fc.chrome
                .iter()
                .find(|c| c.kind == kind)
                .unwrap_or_else(|| panic!("expected a {kind:?} chrome entry"))
        };

        // Running header (no http / no page-number pattern).
        let _rh = find_kind(ChromeKind::RunningHeader);
        // Page number ("page 12").
        let pn = find_kind(ChromeKind::PageNumber);
        assert!(pn.regex.contains("page 12"));
        // Footer link ("https://...").
        let fl = find_kind(ChromeKind::FooterLink);
        assert!(fl.regex.contains("https"));

        // All regexes anchored.
        for c in &fc.chrome {
            assert!(c.regex.starts_with('^'), "{}", c.regex);
            assert!(c.regex.ends_with('$'), "{}", c.regex);
            assert_eq!(c.match_count, 0);
            assert!(c.y_band_pt.is_none());
        }
    }

    #[test]
    fn unknown_heading_and_unknown_table_pass_through() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(7, "Some Random Heading", 1, 700.0)];
        // Header cells don't match any kind pattern.
        skel.tables = vec![skel_table(
            "T01",
            7,
            &["Foo", "Bar", "Baz"],
            &["a", "b", "c"],
            500.0,
        )];

        let fc = classify(&skel);
        assert_eq!(fc.section_roles[0].spec_md_role, SpecMdRole::Unknown);
        assert_eq!(fc.section_roles[0].layer, Layer::Unknown);
        assert_eq!(fc.tables[0].kind, TableKind::Unknown);
        assert_eq!(fc.tables[0].spec_md_target, TableTarget::Unknown);
        // Unknown tables emit no column_map.
        assert!(fc.tables[0].column_map.is_empty());
    }

    #[test]
    fn figure_references_acronyms_in_neighbouring_heading() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(11, "Instruction Fetch (IF)", 2, 700.0)];
        skel.figures = vec![skel_figure("F01", 11, Some("Instruction Fetch (IF)"))];
        skel.acronym_candidates = vec![AcronymCandidate {
            acronym: "IF".to_string(),
            expansion: "Instruction Fetch".to_string(),
            first_page: 11,
            later_usage_count: 47,
        }];

        let fc = classify(&skel);
        assert_eq!(fc.figures[0].kind, FigureKind::BlockDiagram);
        assert_eq!(fc.figures[0].referenced_acronyms, vec!["IF".to_string()]);
    }

    #[test]
    fn csr_field_table_uses_nearest_csr_heading() {
        let mut skel = empty_skeleton();
        // First heading is a CSR section, second is the
        // mstatus CSR; the table follows.
        skel.headings = vec![
            skel_heading(3, "CSR Listing", 1, 700.0),
            skel_heading(4, "mstatus", 2, 700.0),
        ];
        skel.tables = vec![skel_table(
            "T01",
            4,
            &["Bits", "Name", "Description"],
            &["[31]", "SD", "State Dirty"],
            500.0,
        )];

        let fc = classify(&skel);
        assert_eq!(fc.tables[0].kind, TableKind::CsrFieldTable);
        // The CSR field target's `csr_name` is the most-recent
        // heading whose classified role was Csrs. The "CSR
        // Listing" heading on page 3 is the only Csrs-role
        // heading; the "mstatus" heading on page 4 has no
        // pattern match (its text doesn't contain "csr").
        match &fc.tables[0].spec_md_target {
            TableTarget::CsrFields { csr_name } => {
                assert_eq!(csr_name, "CSR Listing");
            }
            other => panic!("expected CsrFields, got {other:?}"),
        }
    }

    #[test]
    fn memory_map_table_classifies() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(8, "Memory Map", 1, 700.0)];
        skel.tables = vec![skel_table(
            "T01",
            8,
            &["Region", "Base", "Size"],
            &["DDR", "0x80000000", "2GB"],
            500.0,
        )];

        let fc = classify(&skel);
        assert_eq!(fc.section_roles[0].spec_md_role, SpecMdRole::MemoryMap);
        assert_eq!(fc.section_roles[0].layer, Layer::Architectural);
        assert_eq!(fc.tables[0].kind, TableKind::MemoryMapTable);
        assert_eq!(fc.tables[0].spec_md_target, TableTarget::MemoryMap);
    }

    #[test]
    fn first_cut_is_deterministic() {
        let mut skel = empty_skeleton();
        skel.headings = vec![skel_heading(1, "Instruction Fetch (IF)", 2, 700.0)];
        skel.tables = vec![skel_table(
            "T01",
            1,
            &["Signal", "Direction", "To/From", "Description"],
            &["if_pc", "out", "Bus", "Next PC"],
            500.0,
        )];
        skel.figures = vec![skel_figure("F01", 1, Some("Instruction Fetch (IF)"))];
        skel.acronym_candidates = vec![AcronymCandidate {
            acronym: "IF".to_string(),
            expansion: "Instruction Fetch".to_string(),
            first_page: 1,
            later_usage_count: 5,
        }];
        skel.chrome_repeated_lines = vec!["page 1".to_string()];

        let a = classify(&skel);
        let b = classify(&skel);
        assert_eq!(a, b);
    }
}
