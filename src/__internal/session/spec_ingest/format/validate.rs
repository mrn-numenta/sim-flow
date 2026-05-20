//! Deterministic validation post-pass for the resolved `format.json`
//! descriptor (Phase 9.5b / Chapter 7 §7.3.7 + §7.9).
//!
//! Takes a [`FormatJson`] and the [`Skeleton`] it was derived from
//! and re-verifies the descriptor's classifications against the
//! actual document. Populates [`ValidationBlock`] with counts and
//! emits [`ValidationWarning`] entries for the divergence patterns
//! Chapter 7 §7.9 catalogues:
//!
//! - `wrap_strategy_zero_merges` -- table claims a multi-row wrap
//!   strategy but the skeleton's table has too few rows for the
//!   strategy to fire.
//! - `csrs_role_collision` -- two descriptor entries claim the same
//!   `csr_name` target.
//! - `unresolved_acronyms` -- skeleton detected an acronym candidate
//!   that the descriptor's glossary does not cover.
//! - `chrome_over_match` -- a chrome regex matched more often than a
//!   per-page chrome line could plausibly account for, so it's
//!   eating real spec content.
//! - `unknown_canonical` -- a column-map entry maps to a canonical
//!   field name that isn't recognised for its table kind.
//! - `section_heading_drift` -- a section_roles entry's heading text
//!   doesn't match the skeleton's heading at the same page.
//!
//! Counts populated independently of the warnings:
//! `section_roles_assigned` (entries whose role isn't `Unknown`),
//! `tables_classified` (per-kind table count, BTreeMap for stable
//! ordering), `tables_unknown`, `glossary_entries`,
//! `chrome_lines_stripped` (sum of `match_count` across chrome
//! entries).
//!
//! The validator is pure -- it doesn't touch disk, network, or any
//! LLM. The same `(descriptor, skeleton)` pair always produces the
//! same [`ValidationBlock`], which keeps cache hashes stable.

use std::collections::{BTreeMap, BTreeSet};

use super::descriptor::{
    ChromeEntry, FormatJson, SectionRoleEntry, SpecMdRole, TableEntry, TableKind, TableTarget,
    ValidationBlock, ValidationWarning, WrapStrategy,
};
use super::skeleton::Skeleton;

/// Recompute the [`ValidationBlock`] for a resolved descriptor against
/// the skeleton it was derived from. Returns a fully-populated block
/// (counts + warnings) suitable for assignment to
/// [`FormatJson::validation`] before persisting.
///
/// Callers in the ingest pipeline should invoke this once per
/// descriptor build, regardless of which path (default / first-cut /
/// LLM-critiqued) produced it, so the validation block always
/// reflects the descriptor that will be cached.
pub fn validate(descriptor: &FormatJson, skeleton: &Skeleton) -> ValidationBlock {
    let mut block = ValidationBlock {
        section_roles_assigned: count_section_roles_assigned(&descriptor.section_roles),
        tables_classified: count_tables_classified(&descriptor.tables),
        tables_unknown: count_tables_unknown(&descriptor.tables),
        glossary_entries: descriptor.glossary.len() as u32,
        chrome_lines_stripped: descriptor.chrome.iter().map(|c| c.match_count).sum(),
        warnings: Vec::new(),
    };
    block
        .warnings
        .extend(check_section_heading_drift(descriptor, skeleton));
    block
        .warnings
        .extend(check_wrap_strategy_zero_merges(descriptor, skeleton));
    block.warnings.extend(check_csrs_role_collision(descriptor));
    block
        .warnings
        .extend(check_unresolved_acronyms(descriptor, skeleton));
    block
        .warnings
        .extend(check_chrome_over_match(&descriptor.chrome, skeleton));
    block.warnings.extend(check_unknown_canonical(descriptor));
    block
}

fn count_section_roles_assigned(entries: &[SectionRoleEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| !matches!(e.spec_md_role, SpecMdRole::Unknown))
        .count() as u32
}

fn count_tables_classified(tables: &[TableEntry]) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = BTreeMap::new();
    for t in tables {
        if matches!(t.kind, TableKind::Unknown) {
            continue;
        }
        let key = table_kind_key(t.kind);
        *out.entry(key.to_string()).or_insert(0) += 1;
    }
    out
}

fn count_tables_unknown(tables: &[TableEntry]) -> u32 {
    tables
        .iter()
        .filter(|t| matches!(t.kind, TableKind::Unknown))
        .count() as u32
}

fn table_kind_key(kind: TableKind) -> &'static str {
    match kind {
        TableKind::SignalTable => "signal_table",
        TableKind::ExternalSignalTable => "external_signal_table",
        TableKind::ParameterTable => "parameter_table",
        TableKind::CsrTable => "csr_table",
        TableKind::CsrFieldTable => "csr_field_table",
        TableKind::RegisterFileTable => "register_file_table",
        TableKind::MemoryMapTable => "memory_map_table",
        TableKind::EncodingTable => "encoding_table",
        TableKind::ErrorTable => "error_table",
        TableKind::FsmStateTable => "fsm_state_table",
        TableKind::FsmTransitionTable => "fsm_transition_table",
        TableKind::LatencyTable => "latency_table",
        TableKind::ConnectivityTable => "connectivity_table",
        TableKind::PmuEventTable => "pmu_event_table",
        TableKind::Unknown => "unknown",
    }
}

// ---------------------------------------------------------------------
// Section heading drift
// ---------------------------------------------------------------------

/// For each `section_roles[]` entry, look for a skeleton heading on
/// the same page whose text matches the descriptor's heading. Emits
/// `section_heading_drift` when none of the page's headings match.
/// Skips entries pointing at pages the skeleton has no headings on
/// (markdown sources collapse all headings to page 1; the check
/// would false-positive on page-2+ section_roles in that case).
fn check_section_heading_drift(
    descriptor: &FormatJson,
    skeleton: &Skeleton,
) -> Vec<ValidationWarning> {
    let mut warnings = Vec::new();
    for (idx, entry) in descriptor.section_roles.iter().enumerate() {
        let page_headings: Vec<&str> = skeleton
            .headings
            .iter()
            .filter(|h| h.page == entry.page)
            .map(|h| h.text.as_str())
            .collect();
        if page_headings.is_empty() {
            continue;
        }
        let descriptor_text = entry.heading.trim().to_lowercase();
        let any_match = page_headings
            .iter()
            .any(|t| t.trim().to_lowercase() == descriptor_text);
        if !any_match {
            warnings.push(ValidationWarning {
                code: "section_heading_drift".to_string(),
                message: format!(
                    "section_roles[{idx}] heading `{}` not found on page {}; \
                     descriptor diverges from the skeleton's headings on that page",
                    entry.heading, entry.page
                ),
                table_id: None,
                section_id: Some(section_id_for(entry)),
            });
        }
    }
    warnings
}

fn section_id_for(entry: &SectionRoleEntry) -> String {
    format!("p{}:l{}", entry.page, entry.line)
}

// ---------------------------------------------------------------------
// Wrap strategy zero merges
// ---------------------------------------------------------------------

/// For each table entry that uses `MergeContinuationRows` or
/// `JoinOnBlankFirstCol`, verify the skeleton's table has enough
/// rows for the strategy to do something. A multi-row wrap on a
/// single-row table can never fire, so the strategy is almost
/// certainly the wrong one for the table.
fn check_wrap_strategy_zero_merges(
    descriptor: &FormatJson,
    skeleton: &Skeleton,
) -> Vec<ValidationWarning> {
    let mut warnings = Vec::new();
    for table in &descriptor.tables {
        let needs_multi_row = matches!(
            table.wrap_strategy,
            WrapStrategy::MergeContinuationRows | WrapStrategy::JoinOnBlankFirstCol
        );
        if !needs_multi_row {
            continue;
        }
        // The descriptor's row_count is authored; the skeleton is
        // the ground truth. Prefer the skeleton's count when
        // available so a typo in the descriptor doesn't mask a real
        // mismatch.
        let skeleton_rows = skeleton
            .tables
            .iter()
            .find(|t| t.page == table.page && t.id == table.id)
            .map(|t| t.row_count)
            .unwrap_or(table.row_count);
        if skeleton_rows >= 2 {
            continue;
        }
        warnings.push(ValidationWarning {
            code: "wrap_strategy_zero_merges".to_string(),
            message: format!(
                "table `{}` declares wrap_strategy `{}` but the skeleton has only {} row(s); \
                 the strategy cannot fire and is almost certainly misclassified",
                table.id,
                wrap_strategy_key(table.wrap_strategy),
                skeleton_rows
            ),
            table_id: Some(table.id.clone()),
            section_id: None,
        });
    }
    warnings
}

fn wrap_strategy_key(s: WrapStrategy) -> &'static str {
    match s {
        WrapStrategy::SingleRow => "single_row",
        WrapStrategy::MergeContinuationRows => "merge_continuation_rows",
        WrapStrategy::JoinOnBlankFirstCol => "join_on_blank_first_col",
    }
}

// ---------------------------------------------------------------------
// CSR role collision
// ---------------------------------------------------------------------

/// Walks every `csr_name`-carrying entry (table targets +
/// section_roles + CSR-field table targets) and emits a warning per
/// CSR name that appears more than once. Catches the LLM critique
/// pattern where the model classifies both a "CSR Listing" table
/// and a "CSR Description" table with the same target CSR.
fn check_csrs_role_collision(descriptor: &FormatJson) -> Vec<ValidationWarning> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for table in &descriptor.tables {
        if let TableTarget::CsrFields { csr_name } = &table.spec_md_target {
            seen.entry(csr_name.clone())
                .or_default()
                .push(format!("table {}", table.id));
        }
    }
    for entry in &descriptor.section_roles {
        if let SpecMdRole::CsrFields { csr_name } = &entry.spec_md_role {
            seen.entry(csr_name.clone())
                .or_default()
                .push(format!("section {}", section_id_for(entry)));
        }
    }
    let mut warnings = Vec::new();
    for (csr_name, sources) in seen.iter().filter(|(_, srcs)| srcs.len() > 1) {
        warnings.push(ValidationWarning {
            code: "csrs_role_collision".to_string(),
            message: format!(
                "CSR `{csr_name}` is referenced by {} descriptor entries: {}",
                sources.len(),
                sources.join(", ")
            ),
            table_id: None,
            section_id: None,
        });
    }
    warnings
}

// ---------------------------------------------------------------------
// Unresolved acronyms
// ---------------------------------------------------------------------

/// For every acronym candidate the skeleton detected, check the
/// descriptor's glossary covers it. Skips candidates with zero
/// later-usage (the spec mentioned the acronym once parenthesised
/// and never again -- typically a stray definition).
fn check_unresolved_acronyms(
    descriptor: &FormatJson,
    skeleton: &Skeleton,
) -> Vec<ValidationWarning> {
    let glossary: BTreeSet<String> = descriptor
        .glossary
        .iter()
        .map(|g| g.acronym.to_ascii_uppercase())
        .collect();
    let mut warnings = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for cand in &skeleton.acronym_candidates {
        if cand.later_usage_count == 0 {
            continue;
        }
        if glossary.contains(&cand.acronym.to_ascii_uppercase()) {
            continue;
        }
        missing.push(cand.acronym.clone());
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        warnings.push(ValidationWarning {
            code: "unresolved_acronyms".to_string(),
            message: format!(
                "{} acronym(s) referenced in body text but missing from glossary: {}",
                missing.len(),
                missing.join(", ")
            ),
            table_id: None,
            section_id: None,
        });
    }
    warnings
}

// ---------------------------------------------------------------------
// Chrome over-match
// ---------------------------------------------------------------------

/// A chrome regex that matches more lines than a sane chrome-per-
/// page rate (default: 3x total_pages) is almost certainly eating
/// real spec body content. Emit one warning per offending regex.
fn check_chrome_over_match(chrome: &[ChromeEntry], skeleton: &Skeleton) -> Vec<ValidationWarning> {
    let mut warnings = Vec::new();
    let pages = skeleton.document.total_pages.max(1);
    let limit = pages.saturating_mul(3);
    for entry in chrome {
        if entry.match_count <= limit {
            continue;
        }
        warnings.push(ValidationWarning {
            code: "chrome_over_match".to_string(),
            message: format!(
                "chrome regex `{}` matched {} lines across {} page(s) (limit: {}); \
                 likely too greedy and stripping real spec content",
                entry.regex, entry.match_count, pages, limit
            ),
            table_id: None,
            section_id: None,
        });
    }
    warnings
}

// ---------------------------------------------------------------------
// Unknown canonical
// ---------------------------------------------------------------------

/// For each table's `column_map[]`, verify the `canonical` field is
/// in the allowed set for the table's `kind`. The allowed set is a
/// curated list of canonical field names spec_md cares about; an
/// unrecognised value (typo, hallucination, or a kind we haven't
/// modelled yet) emits the warning so downstream classify.rs can
/// surface the rejection rather than silently dropping the column.
fn check_unknown_canonical(descriptor: &FormatJson) -> Vec<ValidationWarning> {
    let mut warnings = Vec::new();
    for table in &descriptor.tables {
        let allowed = canonical_set_for_kind(table.kind);
        if allowed.is_empty() {
            continue;
        }
        for mapping in &table.column_map {
            let canonical = mapping.canonical.trim();
            if canonical.is_empty() {
                continue;
            }
            if allowed.contains(&canonical) {
                continue;
            }
            warnings.push(ValidationWarning {
                code: "unknown_canonical".to_string(),
                message: format!(
                    "table `{}` ({}): column_map[`{}` -> `{}`] uses an unrecognised canonical \
                     name (allowed: {})",
                    table.id,
                    table_kind_key(table.kind),
                    mapping.source,
                    mapping.canonical,
                    allowed.join(", ")
                ),
                table_id: Some(table.id.clone()),
                section_id: None,
            });
        }
    }
    warnings
}

/// Curated canonical-column-name set per table kind. Mirrors the
/// row schemas defined in `spec_md::types`; extend alongside any
/// schema additions. `Unknown` returns an empty set so the validator
/// doesn't flag tables nobody committed on.
fn canonical_set_for_kind(kind: TableKind) -> &'static [&'static str] {
    match kind {
        TableKind::SignalTable | TableKind::ExternalSignalTable => &[
            "name",
            "direction",
            "width",
            "peer",
            "description",
            "reset_value",
            "clock_domain",
            "reset_domain",
            "role",
        ],
        TableKind::ParameterTable => &[
            "name",
            "type",
            "default",
            "valid_range",
            "behavioral_impact",
            "description",
        ],
        TableKind::CsrTable => &[
            "name",
            "address",
            "reset_value",
            "privilege",
            "description",
            "fields",
            "width",
            "access",
        ],
        TableKind::CsrFieldTable => &[
            "field_name",
            "bit_range",
            "reset_value",
            "access",
            "description",
        ],
        TableKind::RegisterFileTable => &[
            "name",
            "width",
            "depth",
            "ports",
            "reset_value",
            "description",
        ],
        TableKind::MemoryMapTable => &[
            "region",
            "base_address",
            "size",
            "access",
            "cacheable",
            "description",
        ],
        TableKind::EncodingTable => &["name", "value", "encoding", "meaning", "description"],
        TableKind::ErrorTable => &[
            "name",
            "code",
            "trigger",
            "response",
            "recovery",
            "description",
        ],
        TableKind::FsmStateTable => &["state", "description", "entry_actions", "exit_actions"],
        TableKind::FsmTransitionTable => &[
            "from_state",
            "to_state",
            "trigger",
            "condition",
            "action",
            "description",
        ],
        TableKind::LatencyTable => &[
            "from",
            "to",
            "min_cycles",
            "max_cycles",
            "typical_cycles",
            "description",
        ],
        TableKind::ConnectivityTable => &[
            "from",
            "to",
            "interface",
            "direction",
            "width",
            "description",
        ],
        TableKind::PmuEventTable => &["name", "event_id", "counter", "description", "reset_value"],
        TableKind::Unknown => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::super::descriptor::{
        ChromeEntry, ChromeKind, ColumnMapping, FontWeight as DescFontWeight, GlossaryEntry,
        GlossarySource, Layer, SectionRoleEntry, SpecMdRole, TableEntry, TableKind, TableTarget,
        WrapStrategy,
    };
    use super::super::skeleton::{
        AcronymCandidate, DocumentSummary, HeadingEntry, Skeleton, TableEntry as SkelTable,
    };
    use super::*;
    use crate::session::spec_ingest::stages::loading::BBox;
    use chrono::Utc;

    fn bbox_zero() -> BBox {
        BBox {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }
    }

    fn empty_skeleton(total_pages: u32) -> Skeleton {
        Skeleton {
            document: DocumentSummary {
                total_pages,
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

    fn empty_descriptor() -> FormatJson {
        FormatJson {
            schema_version: 1,
            model: "test".to_string(),
            prompt_version: "test".to_string(),
            source_sha256: "abc".to_string(),
            discovered_at: Utc::now(),
            section_roles: Vec::new(),
            tables: Vec::new(),
            figures: Vec::new(),
            glossary: Vec::new(),
            chrome: Vec::new(),
            validation: ValidationBlock::default(),
        }
    }

    fn signal_table(id: &str, page: u32, wrap: WrapStrategy, row_count: u32) -> TableEntry {
        TableEntry {
            id: id.to_string(),
            page,
            first_line: 1,
            row_count,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "Block".to_string(),
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
            wrap_strategy: wrap,
            rationale: String::new(),
        }
    }

    fn skel_table(id: &str, page: u32, row_count: u32) -> SkelTable {
        SkelTable {
            id: id.to_string(),
            page,
            row_count,
            col_count: 4,
            header_row: vec!["Signal".to_string(), "Direction".to_string()],
            first_data_row: vec!["clk".to_string(), "in".to_string()],
            bbox: bbox_zero(),
        }
    }

    #[test]
    fn counts_section_roles_and_tables() {
        let mut desc = empty_descriptor();
        desc.section_roles.push(SectionRoleEntry {
            heading: "Block A".into(),
            page: 1,
            line: 1,
            font_size: 12.0,
            font_weight: DescFontWeight::Bold,
            level: 1,
            spec_md_role: SpecMdRole::Block {
                block_name: "Block A".into(),
            },
            layer: Layer::Architectural,
            rationale: String::new(),
        });
        desc.section_roles.push(SectionRoleEntry {
            heading: "Unknown".into(),
            page: 2,
            line: 1,
            font_size: 12.0,
            font_weight: DescFontWeight::Normal,
            level: 1,
            spec_md_role: SpecMdRole::Unknown,
            layer: Layer::Unknown,
            rationale: String::new(),
        });
        desc.tables
            .push(signal_table("T01", 1, WrapStrategy::SingleRow, 4));
        desc.tables.push(TableEntry {
            kind: TableKind::Unknown,
            ..signal_table("T02", 2, WrapStrategy::SingleRow, 1)
        });
        let block = validate(&desc, &empty_skeleton(2));
        assert_eq!(block.section_roles_assigned, 1);
        assert_eq!(block.tables_classified.get("signal_table"), Some(&1));
        assert_eq!(block.tables_unknown, 1);
    }

    #[test]
    fn wrap_strategy_zero_merges_fires_on_single_row_table() {
        let mut desc = empty_descriptor();
        desc.tables.push(signal_table(
            "T01",
            1,
            WrapStrategy::MergeContinuationRows,
            1,
        ));
        let mut skel = empty_skeleton(1);
        skel.tables.push(skel_table("T01", 1, 1));
        let block = validate(&desc, &skel);
        assert_eq!(block.warnings.len(), 1);
        assert_eq!(block.warnings[0].code, "wrap_strategy_zero_merges");
        assert_eq!(block.warnings[0].table_id.as_deref(), Some("T01"));
    }

    #[test]
    fn wrap_strategy_zero_merges_silent_when_single_row_strategy() {
        // SingleRow strategy on a single-row table is correct.
        let mut desc = empty_descriptor();
        desc.tables
            .push(signal_table("T01", 1, WrapStrategy::SingleRow, 1));
        let mut skel = empty_skeleton(1);
        skel.tables.push(skel_table("T01", 1, 1));
        let block = validate(&desc, &skel);
        assert!(
            !block
                .warnings
                .iter()
                .any(|w| w.code == "wrap_strategy_zero_merges"),
            "single-row strategy on single-row table should not warn"
        );
    }

    #[test]
    fn csrs_role_collision_fires_on_duplicate_csr_name() {
        let mut desc = empty_descriptor();
        desc.tables.push(TableEntry {
            id: "T01".into(),
            page: 1,
            first_line: 1,
            row_count: 4,
            col_count: 5,
            kind: TableKind::CsrFieldTable,
            spec_md_target: TableTarget::CsrFields {
                csr_name: "mstatus".into(),
            },
            column_map: Vec::new(),
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        desc.tables.push(TableEntry {
            id: "T02".into(),
            page: 2,
            first_line: 1,
            row_count: 4,
            col_count: 5,
            kind: TableKind::CsrFieldTable,
            spec_md_target: TableTarget::CsrFields {
                csr_name: "mstatus".into(),
            },
            column_map: Vec::new(),
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        let block = validate(&desc, &empty_skeleton(2));
        let collisions: Vec<&ValidationWarning> = block
            .warnings
            .iter()
            .filter(|w| w.code == "csrs_role_collision")
            .collect();
        assert_eq!(collisions.len(), 1);
        assert!(collisions[0].message.contains("mstatus"));
    }

    #[test]
    fn unresolved_acronyms_fires_when_glossary_misses_used_acronym() {
        let mut desc = empty_descriptor();
        desc.glossary.push(GlossaryEntry {
            acronym: "BPU".to_string(),
            expansion: "Branch Prediction Unit".to_string(),
            first_page: 1,
            scope: String::new(),
            used_in_blocks: Vec::new(),
            source: GlossarySource::GlossarySection,
        });
        let mut skel = empty_skeleton(2);
        skel.acronym_candidates.push(AcronymCandidate {
            acronym: "BPU".to_string(),
            expansion: "Branch Prediction Unit".to_string(),
            first_page: 1,
            later_usage_count: 5,
        });
        skel.acronym_candidates.push(AcronymCandidate {
            acronym: "CSR".to_string(),
            expansion: "Control and Status Register".to_string(),
            first_page: 1,
            later_usage_count: 12,
        });
        let block = validate(&desc, &skel);
        let unresolved: Vec<&ValidationWarning> = block
            .warnings
            .iter()
            .filter(|w| w.code == "unresolved_acronyms")
            .collect();
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].message.contains("CSR"));
        assert!(!unresolved[0].message.contains("BPU"));
    }

    #[test]
    fn chrome_over_match_fires_when_count_exceeds_3x_pages() {
        let mut desc = empty_descriptor();
        desc.chrome.push(ChromeEntry {
            regex: r".*overly-greedy.*".to_string(),
            kind: ChromeKind::RunningHeader,
            y_band_pt: None,
            match_count: 500,
        });
        // limit = 50 pages * 3 = 150; match_count=500 > 150 -> warn.
        let skel = empty_skeleton(50);
        let block = validate(&desc, &skel);
        let over: Vec<&ValidationWarning> = block
            .warnings
            .iter()
            .filter(|w| w.code == "chrome_over_match")
            .collect();
        assert_eq!(over.len(), 1);
        assert!(over[0].message.contains("500"));
    }

    #[test]
    fn chrome_over_match_silent_below_threshold() {
        let mut desc = empty_descriptor();
        desc.chrome.push(ChromeEntry {
            regex: r"page \d+".to_string(),
            kind: ChromeKind::PageNumber,
            y_band_pt: None,
            match_count: 95,
        });
        // limit = 50 pages * 3 = 150; match_count=95 <= 150 -> no warn.
        let skel = empty_skeleton(50);
        let block = validate(&desc, &skel);
        assert!(
            !block.warnings.iter().any(|w| w.code == "chrome_over_match"),
            "below-threshold chrome should not warn"
        );
    }

    #[test]
    fn unknown_canonical_fires_on_invalid_column_canonical() {
        let mut desc = empty_descriptor();
        desc.tables.push(TableEntry {
            column_map: vec![
                ColumnMapping {
                    source: "Field".to_string(),
                    canonical: "field_name".to_string(),
                },
                ColumnMapping {
                    source: "Reset".to_string(),
                    canonical: "rest_value".to_string(), // typo
                },
            ],
            ..TableEntry {
                id: "T01".into(),
                page: 1,
                first_line: 1,
                row_count: 4,
                col_count: 5,
                kind: TableKind::CsrFieldTable,
                spec_md_target: TableTarget::CsrFields {
                    csr_name: "mstatus".into(),
                },
                column_map: Vec::new(),
                wrap_strategy: WrapStrategy::SingleRow,
                rationale: String::new(),
            }
        });
        let block = validate(&desc, &empty_skeleton(1));
        let unknown: Vec<&ValidationWarning> = block
            .warnings
            .iter()
            .filter(|w| w.code == "unknown_canonical")
            .collect();
        assert_eq!(unknown.len(), 1);
        assert!(unknown[0].message.contains("rest_value"));
        assert!(unknown[0].message.contains("csr_field_table"));
    }

    #[test]
    fn section_heading_drift_fires_when_descriptor_text_diverges() {
        let mut desc = empty_descriptor();
        desc.section_roles.push(SectionRoleEntry {
            heading: "Wrong Heading".into(),
            page: 1,
            line: 5,
            font_size: 12.0,
            font_weight: DescFontWeight::Bold,
            level: 1,
            spec_md_role: SpecMdRole::Block {
                block_name: "Wrong".into(),
            },
            layer: Layer::Architectural,
            rationale: String::new(),
        });
        let mut skel = empty_skeleton(1);
        skel.headings.push(HeadingEntry {
            page: 1,
            level: 1,
            text: "Actual Heading".to_string(),
            font_size: 12.0,
            is_bold: true,
            line_y: 700.0,
            cluster_id: 0,
        });
        let block = validate(&desc, &skel);
        let drift: Vec<&ValidationWarning> = block
            .warnings
            .iter()
            .filter(|w| w.code == "section_heading_drift")
            .collect();
        assert_eq!(drift.len(), 1);
        assert!(drift[0].message.contains("Wrong Heading"));
    }

    #[test]
    fn empty_descriptor_against_empty_skeleton_produces_no_warnings() {
        let block = validate(&empty_descriptor(), &empty_skeleton(1));
        assert!(block.warnings.is_empty());
        assert_eq!(block.section_roles_assigned, 0);
        assert_eq!(block.tables_unknown, 0);
        assert_eq!(block.glossary_entries, 0);
        assert_eq!(block.chrome_lines_stripped, 0);
        assert!(block.tables_classified.is_empty());
    }
}
