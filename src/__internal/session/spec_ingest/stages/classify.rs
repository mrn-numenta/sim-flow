//! Stage 4: structural classification.
//!
//! Annotates the section tree with `kind`, detects stubs and TBDs,
//! and extracts structured tables (signal, parameter, error,
//! encoding, FSM). Per architecture §1.4 stage 4 each kind has its
//! own header-row matcher; mismatches stay as markdown.
//!
//! Phase 9 milestone 9.9 adds the **format-driven** dispatch path:
//! when a `format.json` descriptor is available (see
//! [`super::super::format::FormatJson`]), [`classify_with_format`]
//! locates each declared `TableEntry` against the per-page
//! [`PageTable`]s in [`LoadedSource`], applies `wrap_strategy`,
//! projects rows through `column_map`, and emits typed spec_md row
//! records (`BlockSignalRow`, `Parameter`, `Csr`, etc.). The legacy
//! per-section heuristic extractors stay as the `format = None`
//! fallback so markdown / text inputs that never run discovery keep
//! working unchanged.

use super::super::format::{
    ColumnMapping, FormatJson, TableEntry, TableKind, TableTarget, WrapStrategy,
};
use super::super::pipeline::{IngestConfig, IngestWarning};
use super::loading::{LoadedSource, PageTable};
use super::parse::{Section, SectionKind, SectionTree};
use crate::session::spec_md::types as spec_md;

// ---------------------------------------------------------------------
// Records produced by this stage. The pipeline accumulates them and
// passes them to stage 7 (emit) for serialisation.
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StubRecord {
    pub breadcrumb: Vec<String>,
    pub source_page: u32,
    pub hint: String,
}

#[derive(Debug, Clone)]
pub struct TbdRecord {
    pub breadcrumb: Vec<String>,
    pub source_page: u32,
    pub context: String,
}

#[derive(Debug, Clone)]
pub struct SignalTable {
    pub breadcrumb: Vec<String>,
    pub stage_label: String,
    pub source_page_range: (u32, u32),
    pub rows: Vec<SignalRow>,
}

#[derive(Debug, Clone)]
pub struct SignalRow {
    pub name: String,
    pub direction: String,
    pub peer: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ParameterTable {
    pub breadcrumb: Vec<String>,
    pub group: String,
    pub source_page_range: (u32, u32),
    pub rows: Vec<ParameterRow>,
}

#[derive(Debug, Clone)]
pub struct ParameterRow {
    pub name: String,
    pub kind: Option<String>,
    pub default: String,
    pub comment: String,
}

#[derive(Debug, Clone)]
pub struct ErrorTable {
    pub breadcrumb: Vec<String>,
    pub source_page_range: (u32, u32),
    pub rows: Vec<ErrorRow>,
}

#[derive(Debug, Clone)]
pub struct ErrorRow {
    pub error_type: String,
    pub detecting_component: String,
    pub detecting_behavior: String,
    pub bus_response: String,
    pub master_behavior: String,
    pub software_response: String,
}

#[derive(Debug, Clone)]
pub struct EncodingTable {
    pub breadcrumb: Vec<String>,
    pub field: String,
    pub bit_width: Option<u32>,
    pub source_page_range: (u32, u32),
    pub rows: Vec<EncodingRow>,
}

#[derive(Debug, Clone)]
pub struct EncodingRow {
    pub value: String,
    pub name: String,
    pub abbreviation: String,
}

#[derive(Debug, Clone)]
pub struct FsmTable {
    pub breadcrumb: Vec<String>,
    pub name: String,
    pub reset_state: Option<String>,
    pub source_page_range: (u32, u32),
    pub transitions: Vec<FsmTransition>,
}

#[derive(Debug, Clone)]
pub struct FsmTransition {
    pub from: String,
    pub input: String,
    pub to: String,
    pub output: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClassifyOutputs {
    pub stubs: Vec<StubRecord>,
    pub tbds: Vec<TbdRecord>,
    pub signals: Vec<SignalTable>,
    pub parameters: Vec<ParameterTable>,
    pub errors: Vec<ErrorTable>,
    pub encodings: Vec<EncodingTable>,
    pub fsms: Vec<FsmTable>,
    // ---- Phase 9 milestone 9.9: format-driven typed outputs ----
    /// Block-scoped signal rows. One group per `(table_id, block_name)`
    /// emitted from `signal_table` entries targeting
    /// [`TableTarget::BlockSignals`]. Owned by the descriptor's
    /// `spec_md_target.block_name` field — no heading inference.
    pub block_signals: Vec<BlockSignalGroup>,
    /// External-interface signal rows, emitted from
    /// `external_signal_table` entries.
    pub external_signals: Vec<spec_md::ExternalSignalRow>,
    /// Typed parameter rows (Chapter 2 `## Parameters` schema), as
    /// produced by `parameter_table` entries targeting
    /// [`TableTarget::Parameters`]. The legacy `parameters` field
    /// above is preserved for the None-format path.
    pub typed_parameters: Vec<spec_md::Parameter>,
    /// CSR rows, indexed by `spec_md_target.csr_name` when
    /// `csr_field_table` entries follow up with bit-field rows.
    pub csrs: Vec<spec_md::Csr>,
    /// Memory-map rows.
    pub memory_regions: Vec<spec_md::MemoryRegion>,
    /// Typed encoding tables: one group per encoding name.
    pub typed_encodings: Vec<EncodingGroup>,
    /// Typed error rows (Chapter 2 `## Error Handling` schema).
    pub typed_errors: Vec<spec_md::ErrorEntry>,
    /// FSM state-list groups, one per `fsm_name`.
    pub fsm_states: Vec<FsmStateGroup>,
    /// FSM transition-list groups, one per `fsm_name`.
    pub fsm_transitions: Vec<FsmTransitionGroup>,
    /// Latency rows.
    pub latencies: Vec<spec_md::LatencyRow>,
    /// Performance-monitoring-unit events.
    pub pmu_events: Vec<spec_md::PmuEvent>,
    /// Connectivity nodes / edges.
    pub connectivity_nodes: Vec<spec_md::Node>,
    pub connectivity_edges: Vec<spec_md::Edge>,
    /// Tables the descriptor could not classify; raw rows preserved
    /// so DM0 can `ask_user` about them.
    pub unknown_tables: Vec<UnknownTable>,
}

/// Block-scoped signal rows. Emitted from `signal_table` entries
/// with [`TableTarget::BlockSignals`]; `block_name` carries the
/// descriptor's `spec_md_target.block_name`.
#[derive(Debug, Clone)]
pub struct BlockSignalGroup {
    pub table_id: String,
    pub block_name: String,
    pub source_page: u32,
    pub rows: Vec<spec_md::BlockSignalRow>,
}

/// Encoding-table rows grouped by the descriptor's
/// `spec_md_target.encoding_name`.
#[derive(Debug, Clone)]
pub struct EncodingGroup {
    pub table_id: String,
    pub encoding_name: String,
    pub source_page: u32,
    pub values: Vec<spec_md::EncodingValue>,
}

/// FSM state-list rows grouped by `fsm_name`.
#[derive(Debug, Clone)]
pub struct FsmStateGroup {
    pub table_id: String,
    pub fsm_name: String,
    pub source_page: u32,
    pub states: Vec<spec_md::FsmState>,
}

/// FSM transition-list rows grouped by `fsm_name`.
#[derive(Debug, Clone)]
pub struct FsmTransitionGroup {
    pub table_id: String,
    pub fsm_name: String,
    pub source_page: u32,
    pub transitions: Vec<spec_md::FsmTransition>,
}

/// Raw rows for tables the descriptor's `kind` was `Unknown` or
/// whose target could not be matched. DM0 surfaces these via
/// `ask_user`.
#[derive(Debug, Clone)]
pub struct UnknownTable {
    pub table_id: String,
    pub source_page: u32,
    pub header_row: Vec<String>,
    pub rows: Vec<UnknownRow>,
}

#[derive(Debug, Clone)]
pub struct UnknownRow {
    pub cells: Vec<String>,
}

/// Entry point. Walks the tree, mutating each section to record its
/// kind / extracted-table refs / tbd counts and returning the
/// aggregated classify outputs.
///
/// Thin wrapper over [`classify_with_format`] with no descriptor —
/// the per-section heuristic extractors run as they did before
/// Phase 9 milestone 9.9.
pub fn classify(
    tree: &mut SectionTree,
    config: &IngestConfig,
    warnings: &mut Vec<IngestWarning>,
) -> ClassifyOutputs {
    classify_with_format(None, tree, config, None, warnings)
}

/// Format-aware classify entry point (Phase 9 milestone 9.9).
///
/// When `format` is `Some(&FormatJson)`, dispatches each declared
/// `TableEntry` against the matching [`PageTable`] in `loaded` and
/// projects rows through the descriptor's `column_map` into typed
/// spec_md row records. When `format` is `None`, falls back to the
/// per-section heuristic extractors (legacy / markdown path).
///
/// `loaded` is required for the format-driven path so we can locate
/// `(page, first_line)` against `PageLayout.tables`. The heuristic
/// path tolerates a missing `loaded` because it reads only
/// `section.body`.
pub fn classify_with_format(
    loaded: Option<&LoadedSource>,
    tree: &mut SectionTree,
    _config: &IngestConfig,
    format: Option<&FormatJson>,
    warnings: &mut Vec<IngestWarning>,
) -> ClassifyOutputs {
    let mut out = ClassifyOutputs::default();
    match (format, loaded) {
        (Some(fmt), Some(loaded)) => {
            classify_format_driven(loaded, fmt, &mut out, warnings);
        }
        _ => {
            classify_sections(&mut tree.roots, &mut out);
        }
    }
    out
}

fn classify_sections(sections: &mut [Section], out: &mut ClassifyOutputs) {
    for section in sections.iter_mut() {
        reassemble_tables(section);
        let stubs = detect_section_stub(section);
        let tbds = detect_section_tbds(section);
        let signals = extract_signal_tables(section);
        let parameters = extract_parameter_tables(section);
        let errors = extract_error_tables(section);
        let encodings = extract_encoding_tables(section);
        let fsms = extract_fsm_tables(section);

        // Apply kind based on what we found.
        let extracted_any = !signals.is_empty()
            || !parameters.is_empty()
            || !errors.is_empty()
            || !encodings.is_empty()
            || !fsms.is_empty();
        let has_body = !section.body.trim().is_empty();
        if let Some(hint) = &stubs {
            section.kind = SectionKind::Stub;
            section.stub_hint = Some(hint.clone());
            out.stubs.push(StubRecord {
                breadcrumb: section.breadcrumb.clone(),
                source_page: section.page_range.0,
                hint: hint.clone(),
            });
        } else if extracted_any && has_body {
            section.kind = SectionKind::Mixed;
        } else if extracted_any {
            section.kind = SectionKind::Table;
        }

        // Body-marker stubs for extracted tables.
        for (i, sig) in signals.iter().enumerate() {
            let rel = format!(
                "tables/signals/{:03}-{}.toml",
                out.signals.len() + i,
                slugify(&sig.stage_label)
            );
            section.contained_signal_tables.push(rel.clone());
            section
                .body
                .push_str(&format!("\n<!-- signal-table extracted to {rel} -->\n"));
        }
        for (i, p) in parameters.iter().enumerate() {
            let rel = format!(
                "tables/parameters/{:03}-{}.toml",
                out.parameters.len() + i,
                slugify(&p.group)
            );
            section.contained_parameter_tables.push(rel.clone());
            section
                .body
                .push_str(&format!("\n<!-- parameter-table extracted to {rel} -->\n"));
        }
        for (i, _e) in errors.iter().enumerate() {
            let rel = format!("tables/errors/{:03}.toml", out.errors.len() + i);
            section.contained_error_tables.push(rel.clone());
            section
                .body
                .push_str(&format!("\n<!-- error-table extracted to {rel} -->\n"));
        }
        for (i, en) in encodings.iter().enumerate() {
            let rel = format!(
                "tables/encodings/{:03}-{}.toml",
                out.encodings.len() + i,
                slugify(&en.field)
            );
            section.contained_encoding_tables.push(rel.clone());
            section
                .body
                .push_str(&format!("\n<!-- encoding-table extracted to {rel} -->\n"));
        }
        for (i, f) in fsms.iter().enumerate() {
            let rel = format!(
                "tables/fsms/{:03}-{}.toml",
                out.fsms.len() + i,
                slugify(&f.name)
            );
            section.contained_fsm_tables.push(rel.clone());
            section
                .body
                .push_str(&format!("\n<!-- fsm-table extracted to {rel} -->\n"));
        }

        for tbd in &tbds {
            out.tbds.push(TbdRecord {
                breadcrumb: section.breadcrumb.clone(),
                source_page: section.page_range.0,
                context: tbd.clone(),
            });
        }
        section.tbd_count = tbds.len() as u32;

        out.signals.extend(signals);
        out.parameters.extend(parameters);
        out.errors.extend(errors);
        out.encodings.extend(encodings);
        out.fsms.extend(fsms);

        classify_sections(&mut section.children, out);
    }
}

// ---------------------------------------------------------------------
// Stubs / TBDs
// ---------------------------------------------------------------------

fn detect_section_stub(section: &Section) -> Option<String> {
    let body = section.body.trim();
    if body.is_empty() {
        return Some("section-heading-only".into());
    }
    if body == "TBD" || body.eq_ignore_ascii_case("tbd") {
        return Some("tbd-only".into());
    }
    let placeholder =
        regex::Regex::new(r"(?i)^(to be (defined|determined|written)|placeholder|tbw|tbd)\.?$")
            .unwrap();
    if placeholder.is_match(body) {
        return Some("placeholder-text".into());
    }
    None
}

pub fn detect_stubs(tree: &SectionTree) -> Vec<StubRecord> {
    let mut out = Vec::new();
    for s in tree.iter() {
        if let Some(hint) = detect_section_stub(s) {
            out.push(StubRecord {
                breadcrumb: s.breadcrumb.clone(),
                source_page: s.page_range.0,
                hint,
            });
        }
    }
    out
}

fn detect_section_tbds(section: &Section) -> Vec<String> {
    let re = regex::Regex::new(r"\bTBD\b").unwrap();
    let mut out = Vec::new();
    for line in section.body.lines() {
        if re.is_match(line) {
            out.push(line.trim().to_string());
        }
    }
    out
}

pub fn detect_tbds(tree: &SectionTree) -> Vec<TbdRecord> {
    let mut out = Vec::new();
    for s in tree.iter() {
        for ctx in detect_section_tbds(s) {
            out.push(TbdRecord {
                breadcrumb: s.breadcrumb.clone(),
                source_page: s.page_range.0,
                context: ctx,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------
// Table-row helpers (markdown-pipe form).
// ---------------------------------------------------------------------

/// A parsed markdown table. Pure data; ignores alignment row.
#[derive(Debug, Clone)]
struct MarkdownTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    /// Byte offset in the body where the table starts. Kept around
    /// for future "replace in-place with marker" callers; unused
    /// by the current extract path.
    #[allow(dead_code)]
    start: usize,
    /// Byte offset where the table ends (one past last line).
    #[allow(dead_code)]
    end: usize,
}

fn parse_markdown_tables(body: &str) -> Vec<MarkdownTable> {
    let mut out = Vec::new();
    let lines: Vec<(usize, &str)> = body
        .lines()
        .scan(0usize, |off, line| {
            let pos = *off;
            *off += line.len() + 1; // +1 for newline
            Some((pos, line))
        })
        .collect();

    let mut i = 0;
    while i < lines.len() {
        let (start_off, header_line) = lines[i];
        if is_table_row(header_line) && i + 1 < lines.len() && is_alignment_row(lines[i + 1].1) {
            let headers = split_row(header_line);
            let mut rows = Vec::new();
            let mut j = i + 2;
            while j < lines.len() && is_table_row(lines[j].1) {
                rows.push(split_row(lines[j].1));
                j += 1;
            }
            let end_off = if j < lines.len() {
                lines[j].0
            } else {
                body.len()
            };
            out.push(MarkdownTable {
                headers,
                rows,
                start: start_off,
                end: end_off,
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.ends_with('|') && t.matches('|').count() >= 2
}

fn is_alignment_row(line: &str) -> bool {
    if !is_table_row(line) {
        return false;
    }
    let cells = split_row(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let t = c.trim();
            !t.is_empty()
                && t.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
                && t.contains('-')
        })
}

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let inner = t.trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

fn headers_match(headers: &[String], alias: &[&str]) -> bool {
    if headers.len() < alias.len() {
        return false;
    }
    let mut needed: Vec<String> = alias.iter().map(|s| s.to_lowercase()).collect();
    for h in headers {
        let hl = h.to_lowercase();
        if let Some(pos) = needed
            .iter()
            .position(|n| n == &hl || equivalent_header(n, &hl))
        {
            needed.remove(pos);
        }
    }
    needed.is_empty()
}

fn equivalent_header(a: &str, b: &str) -> bool {
    let canon = |s: &str| -> String { s.replace(['/', '-', ' '], "").to_lowercase() };
    let ca = canon(a);
    let cb = canon(b);
    if ca == cb {
        return true;
    }
    // Common abbreviations.
    let pairs = [
        ("direction", "dir"),
        ("tofrom", "fromto"),
        ("description", "desc"),
    ];
    for (x, y) in pairs.iter() {
        if (ca == *x && cb == *y) || (ca == *y && cb == *x) {
            return true;
        }
    }
    false
}

fn column_index<F: Fn(&str) -> bool>(headers: &[String], pred: F) -> Option<usize> {
    headers.iter().position(|h| pred(&h.to_lowercase()))
}

// ---------------------------------------------------------------------
// Signal tables
// ---------------------------------------------------------------------

pub fn extract_signal_tables(section: &Section) -> Vec<SignalTable> {
    let mut out = extract_signal_tables_markdown(section);
    out.extend(extract_signal_tables_pdf_text(section));
    out
}

fn extract_signal_tables_markdown(section: &Section) -> Vec<SignalTable> {
    let mut out = Vec::new();
    let defaults: [&[&str]; 3] = [
        &["Signal", "Direction", "To/From", "Description"],
        &["Signal", "Dir", "From/To", "Description"],
        &["Signal", "Direction", "From/To", "Description"],
    ];
    for t in parse_markdown_tables(&section.body) {
        let matched = defaults
            .iter()
            .any(|alias| headers_match(&t.headers, alias));
        if !matched {
            continue;
        }
        let name_idx = column_index(&t.headers, |h| h == "signal");
        let dir_idx = column_index(&t.headers, |h| h == "direction" || h == "dir");
        let peer_idx = column_index(&t.headers, |h| {
            h == "to/from" || h == "from/to" || h == "tofrom" || h == "fromto"
        });
        let desc_idx = column_index(&t.headers, |h| h == "description" || h == "desc");
        let (Some(n), Some(d), Some(p), Some(de)) = (name_idx, dir_idx, peer_idx, desc_idx) else {
            continue;
        };
        let rows: Vec<SignalRow> = t
            .rows
            .iter()
            .filter_map(|r| {
                Some(SignalRow {
                    name: r.get(n)?.clone(),
                    direction: normalize_direction(r.get(d)?),
                    peer: r.get(p)?.clone(),
                    description: r.get(de)?.clone(),
                })
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push(SignalTable {
            breadcrumb: section.breadcrumb.clone(),
            stage_label: section.heading.clone(),
            source_page_range: section.page_range,
            rows,
        });
    }
    out
}

/// PDF text-extraction case: tables don't survive into markdown
/// pipes, but the column headers do produce a recognizable line
/// (e.g. `Signal Direction To/From Description`). Each subsequent
/// row is a (possibly multi-line) record beginning with an
/// identifier-like signal name plus a direction token. We treat
/// the table as a sequence of records terminated by a blank line,
/// a heading-like line, or end-of-section.
fn extract_signal_tables_pdf_text(section: &Section) -> Vec<SignalTable> {
    let mut out = Vec::new();
    // Match the canonical header line. Order matters here -- "Signal"
    // must come first. We tolerate whitespace runs and the common
    // "To/From" vs "From/To" swap.
    let header_re = regex::Regex::new(
        r"(?i)^\s*Signal\s+(?:Direction|Dir)\s+(?:To\s*/\s*From|From\s*/\s*To)\s+Description\s*$",
    )
    .unwrap();
    // A new-record line: optional leading whitespace, an identifier-
    // like signal name, a direction token, then anything else. We
    // accept '_' / digits in the name plus a leading optional `i_` /
    // `o_` prefix.
    let row_start_re = regex::Regex::new(
        r"(?i)^\s*([A-Za-z_][A-Za-z0-9_]*)\s+(in|out|input|output|inout|to|from|i/o|bidir)\b(.*)$",
    )
    .unwrap();

    let numbered_heading_re = regex::Regex::new(r"^\s*\d+(?:\.\d+)+\s+\S").unwrap();
    let title_paren_re = regex::Regex::new(r"^\s*[A-Z][A-Za-z\- ]+\([A-Z][A-Z0-9]*\)\s*$").unwrap();
    let body = &section.body;
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if !header_re.is_match(lines[i]) {
            i += 1;
            continue;
        }
        // Begin a table at lines[i]; collect rows.
        let mut rows: Vec<SignalRow> = Vec::new();
        let mut j = i + 1;
        let mut current: Option<SignalRow> = None;
        while j < lines.len() {
            let line = lines[j];
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if let Some(r) = current.take() {
                    rows.push(r);
                }
                break;
            }
            // Heading-like line ends the table. We treat as a
            // heading: numbered TOC entries ("3.1 Instruction
            // Fetch"); short title-cased lines ending in a
            // parenthesised acronym ("Pre-Decode (PD)"); body-
            // marker comments (`<!-- ... -->`).
            let is_numbered_heading = numbered_heading_re.is_match(line);
            let is_title_with_paren = title_paren_re.is_match(line);
            let is_marker_comment = trimmed.starts_with("<!--");
            if is_numbered_heading || is_title_with_paren || is_marker_comment {
                if let Some(r) = current.take() {
                    rows.push(r);
                }
                break;
            }
            if let Some(caps) = row_start_re.captures(trimmed) {
                if let Some(r) = current.take() {
                    rows.push(r);
                }
                let name = caps.get(1).unwrap().as_str().to_string();
                let direction = normalize_direction(caps.get(2).unwrap().as_str());
                let rest = caps.get(3).unwrap().as_str().trim().to_string();
                current = Some(SignalRow {
                    name,
                    direction,
                    peer: rest,
                    description: String::new(),
                });
            } else if let Some(r) = current.as_mut() {
                // Continuation: append to description (or peer if
                // peer is the only column populated so far).
                if r.description.is_empty()
                    && r.peer.split_whitespace().count() < 4
                    && trimmed
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_alphabetic())
                {
                    // First continuation line is usually the second
                    // half of the "peer" column ("Bus" + next line
                    // "Interface") or the description starts here.
                    if r.peer.split_whitespace().count() < 2
                        && trimmed.split_whitespace().count() <= 3
                    {
                        r.peer = format!("{} {}", r.peer, trimmed).trim().to_string();
                    } else {
                        r.description = trimmed.to_string();
                    }
                } else if !r.description.is_empty() {
                    r.description.push(' ');
                    r.description.push_str(trimmed);
                } else {
                    r.description = trimmed.to_string();
                }
            }
            j += 1;
        }
        if let Some(r) = current.take() {
            rows.push(r);
        }
        if !rows.is_empty() {
            out.push(SignalTable {
                breadcrumb: section.breadcrumb.clone(),
                stage_label: section.heading.clone(),
                source_page_range: section.page_range,
                rows,
            });
        }
        i = j + 1;
    }
    out
}

fn normalize_direction(s: &str) -> String {
    let t = s.trim().to_lowercase();
    match t.as_str() {
        "in" | "input" => "in".into(),
        "out" | "output" => "out".into(),
        "inout" | "i/o" | "io" | "bidir" | "bidirectional" => "inout".into(),
        _ => t,
    }
}

// ---------------------------------------------------------------------
// Parameter tables
// ---------------------------------------------------------------------

pub fn extract_parameter_tables(section: &Section) -> Vec<ParameterTable> {
    let mut out = Vec::new();
    for t in parse_markdown_tables(&section.body) {
        let name_idx = column_index(&t.headers, |h| h == "name" || h == "parameter");
        let default_idx = column_index(&t.headers, |h| h == "default" || h == "value");
        let comment_idx = column_index(&t.headers, |h| {
            h == "comment" || h == "description" || h == "notes"
        });
        let type_idx = column_index(&t.headers, |h| h == "type");
        let (Some(n), Some(d), Some(c)) = (name_idx, default_idx, comment_idx) else {
            continue;
        };
        let rows: Vec<ParameterRow> = t
            .rows
            .iter()
            .filter_map(|r| {
                Some(ParameterRow {
                    name: r.get(n)?.clone(),
                    kind: type_idx.and_then(|i| r.get(i)).cloned(),
                    default: r.get(d)?.clone(),
                    comment: r.get(c)?.clone(),
                })
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push(ParameterTable {
            breadcrumb: section.breadcrumb.clone(),
            group: section.heading.clone(),
            source_page_range: section.page_range,
            rows,
        });
    }
    out
}

// ---------------------------------------------------------------------
// Error tables
// ---------------------------------------------------------------------

pub fn extract_error_tables(section: &Section) -> Vec<ErrorTable> {
    let mut out = Vec::new();
    for t in parse_markdown_tables(&section.body) {
        let err_idx = column_index(&t.headers, |h| h.starts_with("error"));
        let comp_idx = column_index(&t.headers, |h| {
            h.contains("detecting") && h.contains("component")
        });
        if err_idx.is_none() || comp_idx.is_none() {
            continue;
        }
        let beh_idx = column_index(&t.headers, |h| {
            h.contains("detecting") && h.contains("behavior")
        });
        let bus_idx = column_index(&t.headers, |h| h.contains("bus") && h.contains("response"));
        let master_idx = column_index(&t.headers, |h| {
            h.contains("master") && h.contains("behavior")
        });
        let sw_idx = column_index(&t.headers, |h| {
            h.contains("software") && h.contains("response")
        });
        let rows: Vec<ErrorRow> = t
            .rows
            .iter()
            .map(|r| ErrorRow {
                error_type: r.get(err_idx.unwrap()).cloned().unwrap_or_default(),
                detecting_component: r.get(comp_idx.unwrap()).cloned().unwrap_or_default(),
                detecting_behavior: beh_idx.and_then(|i| r.get(i)).cloned().unwrap_or_default(),
                bus_response: bus_idx.and_then(|i| r.get(i)).cloned().unwrap_or_default(),
                master_behavior: master_idx
                    .and_then(|i| r.get(i))
                    .cloned()
                    .unwrap_or_default(),
                software_response: sw_idx.and_then(|i| r.get(i)).cloned().unwrap_or_default(),
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push(ErrorTable {
            breadcrumb: section.breadcrumb.clone(),
            source_page_range: section.page_range,
            rows,
        });
    }
    out
}

// ---------------------------------------------------------------------
// Encoding tables
// ---------------------------------------------------------------------

pub fn extract_encoding_tables(section: &Section) -> Vec<EncodingTable> {
    let mut out = Vec::new();
    for t in parse_markdown_tables(&section.body) {
        let value_idx = column_index(&t.headers, |h| {
            h == "value" || h == "code" || h == "encoding"
        });
        let name_idx = column_index(&t.headers, |h| h == "name" || h == "meaning");
        let abbr_idx = column_index(&t.headers, |h| {
            h == "abbreviation" || h == "abbr" || h == "symbol"
        });
        let (Some(v), Some(n)) = (value_idx, name_idx) else {
            continue;
        };
        // Reject the parameter-table header set (Name | Default | Comment).
        if column_index(&t.headers, |h| h == "default").is_some() {
            continue;
        }
        let rows: Vec<EncodingRow> = t
            .rows
            .iter()
            .filter_map(|r| {
                Some(EncodingRow {
                    value: r.get(v)?.clone(),
                    name: r.get(n)?.clone(),
                    abbreviation: abbr_idx.and_then(|i| r.get(i)).cloned().unwrap_or_default(),
                })
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push(EncodingTable {
            breadcrumb: section.breadcrumb.clone(),
            field: section.heading.clone(),
            bit_width: None,
            source_page_range: section.page_range,
            rows,
        });
    }
    out
}

// ---------------------------------------------------------------------
// FSM tables
// ---------------------------------------------------------------------

pub fn extract_fsm_tables(section: &Section) -> Vec<FsmTable> {
    let mut out = Vec::new();
    for t in parse_markdown_tables(&section.body) {
        let from_idx = column_index(&t.headers, |h| {
            h == "from" || h == "current state" || h == "state"
        });
        let to_idx = column_index(&t.headers, |h| h == "to" || h == "next state");
        let input_idx = column_index(&t.headers, |h| {
            h == "input" || h == "condition" || h == "event"
        });
        if from_idx.is_none() || to_idx.is_none() || input_idx.is_none() {
            continue;
        }
        let out_idx = column_index(&t.headers, |h| h == "output" || h == "action");
        let transitions: Vec<FsmTransition> = t
            .rows
            .iter()
            .filter_map(|r| {
                Some(FsmTransition {
                    from: r.get(from_idx.unwrap())?.clone(),
                    input: r.get(input_idx.unwrap())?.clone(),
                    to: r.get(to_idx.unwrap())?.clone(),
                    output: out_idx.and_then(|i| r.get(i)).cloned().unwrap_or_default(),
                })
            })
            .collect();
        if transitions.is_empty() {
            continue;
        }
        out.push(FsmTable {
            breadcrumb: section.breadcrumb.clone(),
            name: section.heading.clone(),
            reset_state: transitions.first().map(|t| t.from.clone()),
            source_page_range: section.page_range,
            transitions,
        });
    }
    out
}

// ---------------------------------------------------------------------
// Cross-page table reassembly
// ---------------------------------------------------------------------

pub fn reassemble_tables(section: &mut Section) {
    // PDF page breaks produce a "blank line + repeated header + new
    // alignment row" sequence in the middle of an otherwise-
    // continuous table. We rewrite the body in two passes so the
    // markdown-table extractor sees a single contiguous block.
    let body = section.body.clone();
    let lines: Vec<&str> = body.lines().collect();
    // First scan: find the first header (a table row followed by an
    // alignment row). Anything that matches that header later in the
    // body, preceded by a blank line and followed by an alignment
    // row, is a continuation we want to remove.
    let mut first_header: Option<String> = None;
    for i in 0..lines.len() {
        if is_table_row(lines[i]) && i + 1 < lines.len() && is_alignment_row(lines[i + 1]) {
            first_header = Some(lines[i].to_string());
            break;
        }
    }
    let Some(header) = first_header else {
        return;
    };
    let mut out_lines: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // Detect "(possibly blank lines) + repeated header +
        // alignment row" and skip the trailing blank lines we may
        // have already pushed.
        if lines[i] == header
            && i + 1 < lines.len()
            && is_alignment_row(lines[i + 1])
            && !out_lines.is_empty()
        {
            // We're at a candidate repeated header. Only skip if we
            // already wrote at least one row of this table earlier
            // (i.e. we encountered the same header before).
            let already_started = out_lines.iter().any(|l| l == &header);
            if already_started {
                // Pop any trailing blank lines so the table flows.
                while out_lines.last().is_some_and(|l| l.trim().is_empty()) {
                    out_lines.pop();
                }
                // Skip the header + alignment row.
                i += 2;
                continue;
            }
        }
        out_lines.push(lines[i].to_string());
        i += 1;
    }
    let new_body = out_lines.join("\n");
    section.body = if body.ends_with('\n') {
        format!("{new_body}\n")
    } else {
        new_body
    };
}

// ---------------------------------------------------------------------
// Phase 9 milestone 9.9: format-driven dispatch.
//
// For each `TableEntry` in the descriptor, locate the matching
// `PageTable` in `LoadedSource.pages[entry.page - 1]`, coalesce
// continuation rows per `wrap_strategy`, project source headers to
// canonical row-field names via `column_map`, then build the typed
// spec_md row record per `kind` / `spec_md_target`.
// ---------------------------------------------------------------------

/// Tolerance, in points, used when matching a `TableEntry.first_line`
/// against a `PageTable.bbox.y`. The descriptor records the line
/// number of the table's first row; in practice we compare the
/// top-of-bbox Y of every detected table on the page against the
/// entry's line and accept the closest match within a generous band.
const TABLE_LOCATE_Y_TOLERANCE_PT: f32 = 24.0;

fn classify_format_driven(
    loaded: &LoadedSource,
    format: &FormatJson,
    out: &mut ClassifyOutputs,
    warnings: &mut Vec<IngestWarning>,
) {
    for entry in &format.tables {
        let Some(table) = locate_page_table(loaded, entry, warnings) else {
            continue;
        };
        let coalesced_rows = apply_wrap_strategy(&table.rows, entry, warnings);
        if coalesced_rows.is_empty() {
            continue;
        }
        // First row is the header; data rows follow.
        let (header, data_rows) = match coalesced_rows.split_first() {
            Some(parts) => parts,
            None => continue,
        };
        let column_indices = resolve_column_map(header, entry, warnings);
        emit_typed_rows(entry, table, header, data_rows, &column_indices, out);
    }
}

/// Find the `PageTable` on `entry.page` whose bbox top-Y is closest to
/// `entry.first_line` within
/// [`TABLE_LOCATE_Y_TOLERANCE_PT`]. Returns `None` (and emits a
/// `classify_table_location_missed` warning) on miss.
fn locate_page_table<'a>(
    loaded: &'a LoadedSource,
    entry: &TableEntry,
    warnings: &mut Vec<IngestWarning>,
) -> Option<&'a PageTable> {
    let page_idx = entry.page as usize;
    let Some(page) = page_idx
        .checked_sub(1)
        .and_then(|idx| loaded.pages.get(idx))
    else {
        warnings.push(IngestWarning::new(
            "classify_table_location_missed",
            format!(
                "table {} at page {} line {} not found in PageLayout",
                entry.id, entry.page, entry.first_line
            ),
            4,
        ));
        return None;
    };
    if page.tables.is_empty() {
        warnings.push(IngestWarning::new(
            "classify_table_location_missed",
            format!(
                "table {} at page {} line {} not found in PageLayout",
                entry.id, entry.page, entry.first_line
            ),
            4,
        ));
        return None;
    }
    // Match by bbox top-Y proximity. The descriptor's `first_line`
    // is a 1-based line number; the PDF coordinate is a Y in points.
    // We compare the entry's line-as-Y against each table's bbox.y
    // and accept the closest within tolerance.
    let target_y = entry.first_line as f32;
    let mut best: Option<(f32, &PageTable)> = None;
    for table in &page.tables {
        let dy = (table.bbox.y - target_y).abs();
        match best {
            Some((cur, _)) if cur <= dy => {}
            _ => best = Some((dy, table)),
        }
    }
    match best {
        Some((dy, table)) if dy <= TABLE_LOCATE_Y_TOLERANCE_PT => Some(table),
        _ => {
            // Fallback: if there is exactly one table on the page, use
            // it. This is conservative; many descriptors will record
            // `first_line` in line-units rather than PDF-points and
            // we don't want to drop their tables on the floor.
            if page.tables.len() == 1 {
                Some(&page.tables[0])
            } else {
                warnings.push(IngestWarning::new(
                    "classify_table_location_missed",
                    format!(
                        "table {} at page {} line {} not found in PageLayout",
                        entry.id, entry.page, entry.first_line
                    ),
                    4,
                ));
                None
            }
        }
    }
}

/// Apply `entry.wrap_strategy` to `rows`. Emits a
/// `wrap_strategy_zero_merges` warning if a merging strategy was
/// requested but produced zero merges.
fn apply_wrap_strategy(
    rows: &[Vec<String>],
    entry: &TableEntry,
    warnings: &mut Vec<IngestWarning>,
) -> Vec<Vec<String>> {
    match entry.wrap_strategy {
        WrapStrategy::SingleRow => rows.to_vec(),
        WrapStrategy::MergeContinuationRows => {
            let (merged, merge_count) = merge_continuation_rows(rows);
            if merge_count == 0 && rows.len() > 1 {
                warnings.push(IngestWarning::new(
                    "wrap_strategy_zero_merges",
                    format!(
                        "table {} wrap_strategy=merge_continuation_rows but no continuations",
                        entry.id
                    ),
                    4,
                ));
            }
            merged
        }
        WrapStrategy::JoinOnBlankFirstCol => {
            let (merged, merge_count) = join_on_blank_first_col(rows);
            if merge_count == 0 && rows.len() > 1 {
                warnings.push(IngestWarning::new(
                    "wrap_strategy_zero_merges",
                    format!(
                        "table {} wrap_strategy=join_on_blank_first_col but no continuations",
                        entry.id
                    ),
                    4,
                ));
            }
            merged
        }
    }
}

/// Merge rows whose first column is empty into the previous row.
/// Chapter 7 §7.3.3 describes this as "rows whose first non-empty
/// cell is in the same column as the previous row's first non-empty
/// cell, but whose own first column is empty"; in practice the
/// column-alignment clause is redundant once we require an empty
/// first column (prev rows are typically fully populated) so we
/// treat any row with an empty first column carrying at least one
/// non-empty cell as a continuation. Continuation cells are
/// appended column-aligned to the previous row with a `" "`
/// separator. Returns the merged rows and the number of merges
/// performed.
fn merge_continuation_rows(rows: &[Vec<String>]) -> (Vec<Vec<String>>, usize) {
    let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    let mut merges = 0usize;
    for row in rows {
        let is_continuation = match out.last() {
            Some(_prev) => {
                let cur_first = first_nonempty_col(row);
                let cur_col_empty = row.first().map(|c| c.trim().is_empty()).unwrap_or(true);
                cur_col_empty && cur_first.is_some()
            }
            None => false,
        };
        if is_continuation {
            let prev = out.last_mut().expect("checked above");
            for (i, cell) in row.iter().enumerate() {
                if let Some(prev_cell) = prev.get_mut(i) {
                    let trimmed = cell.trim();
                    if !trimmed.is_empty() {
                        if prev_cell.is_empty() {
                            *prev_cell = trimmed.to_string();
                        } else {
                            prev_cell.push(' ');
                            prev_cell.push_str(trimmed);
                        }
                    }
                }
            }
            merges += 1;
        } else {
            out.push(row.clone());
        }
    }
    (out, merges)
}

/// Fold rows whose first column is empty into the immediately
/// previous row. Returns the merged rows and merge count.
fn join_on_blank_first_col(rows: &[Vec<String>]) -> (Vec<Vec<String>>, usize) {
    let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    let mut merges = 0usize;
    for row in rows {
        let cur_col_empty = row.first().map(|c| c.trim().is_empty()).unwrap_or(true);
        if cur_col_empty && !out.is_empty() {
            let prev = out.last_mut().expect("checked above");
            for (i, cell) in row.iter().enumerate() {
                if let Some(prev_cell) = prev.get_mut(i) {
                    let trimmed = cell.trim();
                    if !trimmed.is_empty() {
                        if prev_cell.is_empty() {
                            *prev_cell = trimmed.to_string();
                        } else {
                            prev_cell.push(' ');
                            prev_cell.push_str(trimmed);
                        }
                    }
                }
            }
            merges += 1;
        } else {
            out.push(row.clone());
        }
    }
    (out, merges)
}

fn first_nonempty_col(row: &[String]) -> Option<usize> {
    row.iter().position(|c| !c.trim().is_empty())
}

/// Resolve each `ColumnMapping` to a column index in `header`.
/// Returns a `canonical -> col_idx` map. Unknown sources or
/// canonicals not in the target's row-schema are skipped; the
/// latter emits a `classify_unknown_canonical` warning.
fn resolve_column_map(
    header: &[String],
    entry: &TableEntry,
    warnings: &mut Vec<IngestWarning>,
) -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    let valid: &[&str] = canonical_fields_for_kind(entry.kind);
    for ColumnMapping { source, canonical } in &entry.column_map {
        // Validate canonical against the target row schema.
        if !valid.is_empty() && !valid.iter().any(|v| *v == canonical) {
            warnings.push(IngestWarning::new(
                "classify_unknown_canonical",
                format!(
                    "table {} maps source `{}` to unknown canonical `{}`",
                    entry.id, source, canonical
                ),
                4,
            ));
            continue;
        }
        // Locate source column, case-insensitively.
        if let Some(idx) = header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(source.trim()))
        {
            out.insert(canonical.clone(), idx);
        }
    }
    out
}

/// Canonical row-schema fields per `TableKind`. Used to validate
/// `ColumnMapping::canonical` references. An empty slice means
/// "any canonical accepted" (e.g. `Unknown` kind).
fn canonical_fields_for_kind(kind: TableKind) -> &'static [&'static str] {
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

fn pick(cols: &std::collections::HashMap<String, usize>, key: &str, row: &[String]) -> String {
    cols.get(key)
        .and_then(|idx| row.get(*idx))
        .cloned()
        .unwrap_or_default()
}

/// Build the typed records for one matched table per its
/// `kind` / `spec_md_target` and append them to `out`.
fn emit_typed_rows(
    entry: &TableEntry,
    _table: &PageTable,
    header: &[String],
    data_rows: &[Vec<String>],
    cols: &std::collections::HashMap<String, usize>,
    out: &mut ClassifyOutputs,
) {
    match (entry.kind, &entry.spec_md_target) {
        (TableKind::SignalTable, TableTarget::BlockSignals { block_name }) => {
            let rows: Vec<spec_md::BlockSignalRow> = data_rows
                .iter()
                .map(|r| spec_md::BlockSignalRow {
                    name: pick(cols, "name", r),
                    direction: normalize_direction(&pick(cols, "direction", r)),
                    peer: pick(cols, "peer", r),
                    description: pick(cols, "description", r),
                    role: spec_md::SignalRole::default(),
                })
                .collect();
            out.block_signals.push(BlockSignalGroup {
                table_id: entry.id.clone(),
                block_name: block_name.clone(),
                source_page: entry.page,
                rows,
            });
        }
        (TableKind::ExternalSignalTable, TableTarget::ExternalSignals) => {
            for r in data_rows {
                out.external_signals.push(spec_md::ExternalSignalRow {
                    name: pick(cols, "name", r),
                    direction: normalize_direction(&pick(cols, "direction", r)),
                    width: pick(cols, "width", r),
                    ty: pick(cols, "type", r),
                    required: pick(cols, "required", r).eq_ignore_ascii_case("true")
                        || pick(cols, "required", r).eq_ignore_ascii_case("yes"),
                    description: pick(cols, "description", r),
                });
            }
        }
        (TableKind::ParameterTable, TableTarget::Parameters) => {
            for r in data_rows {
                out.typed_parameters.push(spec_md::Parameter {
                    name: pick(cols, "name", r),
                    ty: pick(cols, "type", r),
                    default: pick(cols, "default", r),
                    valid_range: pick(cols, "valid_range", r),
                    behavioral_impact: pick(cols, "behavioral_impact", r),
                    source_anchor: format!("table:{}", entry.id),
                });
            }
        }
        (TableKind::CsrTable, TableTarget::Csrs) => {
            for r in data_rows {
                out.csrs.push(spec_md::Csr {
                    address: pick(cols, "address", r),
                    name: pick(cols, "name", r),
                    access: pick(cols, "access", r),
                    reset_value: pick(cols, "reset_value", r),
                    required_privilege: pick(cols, "required_privilege", r),
                    description: pick(cols, "description", r),
                    fields: Vec::new(),
                    source_anchor: format!("table:{}", entry.id),
                });
            }
        }
        (TableKind::CsrFieldTable, TableTarget::CsrFields { csr_name }) => {
            let fields: Vec<spec_md::CsrField> = data_rows
                .iter()
                .map(|r| spec_md::CsrField {
                    bits: pick(cols, "bits", r),
                    name: pick(cols, "name", r),
                    access: pick(cols, "access", r),
                    description: pick(cols, "description", r),
                })
                .collect();
            if let Some(csr) = out.csrs.iter_mut().find(|c| c.name == *csr_name) {
                csr.fields.extend(fields);
            } else {
                // Parent CSR not (yet) seen — stash a placeholder so a
                // later pass can stitch fields without losing them.
                out.csrs.push(spec_md::Csr {
                    name: csr_name.clone(),
                    fields,
                    source_anchor: format!("table:{}", entry.id),
                    ..spec_md::Csr::default()
                });
            }
        }
        (TableKind::MemoryMapTable, TableTarget::MemoryMap) => {
            for r in data_rows {
                out.memory_regions.push(spec_md::MemoryRegion {
                    start: pick(cols, "start", r),
                    end: pick(cols, "end", r),
                    name: pick(cols, "name", r),
                    purpose: pick(cols, "purpose", r),
                    access: pick(cols, "access", r),
                    required_privilege: pick(cols, "required_privilege", r),
                    source_anchor: format!("table:{}", entry.id),
                });
            }
        }
        (TableKind::EncodingTable, TableTarget::Encoding { encoding_name }) => {
            let values: Vec<spec_md::EncodingValue> = data_rows
                .iter()
                .map(|r| spec_md::EncodingValue {
                    value: pick(cols, "value", r),
                    name: pick(cols, "name", r),
                    abbreviation: pick(cols, "abbreviation", r),
                })
                .collect();
            out.typed_encodings.push(EncodingGroup {
                table_id: entry.id.clone(),
                encoding_name: encoding_name.clone(),
                source_page: entry.page,
                values,
            });
        }
        (TableKind::ErrorTable, TableTarget::Errors) => {
            for r in data_rows {
                out.typed_errors.push(spec_md::ErrorEntry {
                    error_type: pick(cols, "error_type", r),
                    detecting_component: pick(cols, "detecting_component", r),
                    detection_behavior: pick(cols, "detection_behavior", r),
                    bus_response: pick(cols, "bus_response", r),
                    master_behavior: pick(cols, "master_behavior", r),
                    software_response: pick(cols, "software_response", r),
                    source_anchor: format!("table:{}", entry.id),
                });
            }
        }
        (TableKind::FsmStateTable, TableTarget::StateMachineStates { fsm_name }) => {
            let states: Vec<spec_md::FsmState> = data_rows
                .iter()
                .map(|r| spec_md::FsmState {
                    name: pick(cols, "name", r),
                    description: pick(cols, "description", r),
                })
                .collect();
            out.fsm_states.push(FsmStateGroup {
                table_id: entry.id.clone(),
                fsm_name: fsm_name.clone(),
                source_page: entry.page,
                states,
            });
        }
        (TableKind::FsmTransitionTable, TableTarget::StateMachineTransitions { fsm_name }) => {
            let transitions: Vec<spec_md::FsmTransition> = data_rows
                .iter()
                .map(|r| spec_md::FsmTransition {
                    from: pick(cols, "from", r),
                    input: pick(cols, "input", r),
                    to: pick(cols, "to", r),
                    output: pick(cols, "output", r),
                })
                .collect();
            out.fsm_transitions.push(FsmTransitionGroup {
                table_id: entry.id.clone(),
                fsm_name: fsm_name.clone(),
                source_page: entry.page,
                transitions,
            });
        }
        (TableKind::LatencyTable, TableTarget::TimingLatency) => {
            for r in data_rows {
                out.latencies.push(spec_md::LatencyRow {
                    operation: pick(cols, "operation", r),
                    best_case: pick(cols, "best_case", r),
                    worst_case: pick(cols, "worst_case", r),
                    notes: pick(cols, "notes", r),
                });
            }
        }
        (TableKind::PmuEventTable, TableTarget::PmuEvents) => {
            for r in data_rows {
                out.pmu_events.push(spec_md::PmuEvent {
                    id: pick(cols, "id", r),
                    name: pick(cols, "name", r),
                    description: pick(cols, "description", r),
                    csr_address: pick(cols, "csr_address", r),
                });
            }
        }
        (TableKind::ConnectivityTable, TableTarget::ConnectivityNodes) => {
            for r in data_rows {
                out.connectivity_nodes.push(spec_md::Node {
                    id: pick(cols, "id", r),
                    ty: pick(cols, "type", r),
                    coordinate: pick(cols, "coordinate", r),
                    role: pick(cols, "role", r),
                });
            }
        }
        (TableKind::ConnectivityTable, TableTarget::ConnectivityEdges) => {
            for r in data_rows {
                out.connectivity_edges.push(spec_md::Edge {
                    from: pick(cols, "from", r),
                    to: pick(cols, "to", r),
                    channel: pick(cols, "channel", r),
                    source_anchor: format!("table:{}", entry.id),
                });
            }
        }
        // Unknown or unmatched (kind, target) pair — emit raw rows.
        _ => {
            let rows: Vec<UnknownRow> = data_rows
                .iter()
                .map(|r| UnknownRow { cells: r.clone() })
                .collect();
            out.unknown_tables.push(UnknownTable {
                table_id: entry.id.clone(),
                source_page: entry.page,
                header_row: header.to_vec(),
                rows,
            });
        }
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "section".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_ingest::stages::parse::parse_markdown;

    fn section_with_body(body: &str) -> Section {
        Section {
            heading: "Sec".into(),
            level: 1,
            breadcrumb: vec!["Sec".into()],
            body: body.into(),
            page_range: (1, 1),
            children: Vec::new(),
            kind: SectionKind::Prose,
            contained_signal_tables: Vec::new(),
            contained_parameter_tables: Vec::new(),
            contained_error_tables: Vec::new(),
            contained_encoding_tables: Vec::new(),
            contained_fsm_tables: Vec::new(),
            contained_figures: Vec::new(),
            tbd_count: 0,
            stub_hint: None,
        }
    }

    #[test]
    fn detect_stubs_finds_empty_section() {
        let body = "# Empty\n\n## Nonempty\nbody here\n";
        let mut warnings = Vec::new();
        let tree = parse_markdown(body, &mut warnings).unwrap();
        let stubs = detect_stubs(&tree);
        assert!(
            stubs
                .iter()
                .any(|s| s.breadcrumb.last() == Some(&"Empty".to_string()))
        );
    }

    #[test]
    fn detect_tbds_finds_whole_word_tbd() {
        let body = "# X\nthis system has TBD memory\nand TBDish is not a match\n";
        let mut warnings = Vec::new();
        let tree = parse_markdown(body, &mut warnings).unwrap();
        let tbds = detect_tbds(&tree);
        assert_eq!(tbds.len(), 1);
        assert!(tbds[0].context.contains("TBD memory"));
    }

    #[test]
    fn signal_table_extracted_with_alternate_ordering() {
        let mut s = section_with_body(
            "Some prose.\n\n| Signal | Direction | To/From | Description |\n| --- | --- | --- | --- |\n| if_nxt_pc | out | Bus | next addr |\n| parcel_pc | in | Bus | fetch addr |\n\n",
        );
        let tables = extract_signal_tables(&s);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[0].rows[0].name, "if_nxt_pc");
        assert_eq!(tables[0].rows[0].direction, "out");
        let _ = &mut s;

        // Alternate ordering: Direction | Signal | Description | To/From.
        let s2 = section_with_body(
            "| Direction | Signal | Description | To/From |\n| --- | --- | --- | --- |\n| in | clk | system clock | external |\n",
        );
        let t2 = extract_signal_tables(&s2);
        assert_eq!(t2.len(), 1);
        assert_eq!(t2[0].rows[0].name, "clk");
    }

    #[test]
    fn parameter_table_extracted() {
        let s = section_with_body(
            "| Name | Default | Comment |\n| --- | --- | --- |\n| FOO | 3 | scales mesh |\n| BAR | 64B |  |\n",
        );
        let t = extract_parameter_tables(&s);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].rows.len(), 2);
        assert_eq!(t[0].rows[0].name, "FOO");
        assert_eq!(t[0].rows[0].default, "3");
    }

    #[test]
    fn error_table_extracted() {
        let s = section_with_body(
            "| Error Type | Detecting Component | Detecting Behavior | Bus Response | Master Behavior | Software Response |\n| --- | --- | --- | --- | --- | --- |\n| Wrong addr | NoC | Log | Bus error | Abort | Interrupt |\n",
        );
        let t = extract_error_tables(&s);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].rows[0].error_type, "Wrong addr");
    }

    #[test]
    fn encoding_table_extracted() {
        let s = section_with_body(
            "| Value | Name | Abbreviation |\n| --- | --- | --- |\n| 00 | User | U |\n| 01 | Supervisor | S |\n",
        );
        let t = extract_encoding_tables(&s);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].rows.len(), 2);
        assert_eq!(t[0].rows[0].abbreviation, "U");
    }

    #[test]
    fn pdf_text_signal_table_extracted_with_continuations() {
        let body = "Some intro.\n\nSignal Direction To/From Description\nif_nxt_pc to Bus\nInterface\nNext address to fetch parcel from\nparcel_pc from Bus\nInterface\nFetch parcel address\n\nOther prose.\n\nSignal Direction To/From Description\nif_pc from IF Instruction Fetch program counter\nif_instr from IF Instruction Fetch instruction\n";
        let s = section_with_body(body);
        let tables = extract_signal_tables(&s);
        assert_eq!(tables.len(), 2, "expected two PDF-text signal tables");
        assert!(
            tables[0].rows.iter().any(|r| r.name == "if_nxt_pc"),
            "missing if_nxt_pc"
        );
        assert!(
            tables[1].rows.iter().any(|r| r.name == "if_pc"),
            "missing if_pc"
        );
    }

    #[test]
    fn fsm_table_extracted() {
        let s = section_with_body(
            "| From | Input | To | Output |\n| --- | --- | --- | --- |\n| IDLE | power_on | RESET_HOLD | assert nReset |\n| RESET_HOLD | stability_timer_done | RESET_RELEASE | deassert nReset |\n",
        );
        let t = extract_fsm_tables(&s);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].transitions.len(), 2);
        assert_eq!(t[0].transitions[0].from, "IDLE");
    }

    #[test]
    fn reassemble_drops_repeated_header_across_page() {
        let mut s = section_with_body(
            "| Signal | Dir | To/From | Description |\n| --- | --- | --- | --- |\n| a | in | x | first row |\n\n| Signal | Dir | To/From | Description |\n| --- | --- | --- | --- |\n| b | out | y | second row |\n",
        );
        reassemble_tables(&mut s);
        let tables = extract_signal_tables(&s);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 2);
    }

    // ---------------------------------------------------------------
    // Phase 9 milestone 9.9: format-driven classify tests.
    //
    // Synthetic LoadedSource + FormatJson fixtures exercise the
    // wrap_strategy / column_map / dispatch table without going
    // through pdf_oxide.
    // ---------------------------------------------------------------

    use super::super::super::format::{
        ColumnMapping, FontWeight as FmtFontWeight, FormatJson, TableEntry, TableKind, TableTarget,
        ValidationBlock, WrapStrategy,
    };
    use super::super::super::pipeline::SourceKind;
    use super::super::loading::{BBox, LoadedSource, PageLayout, PageTable};
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    /// Build a single-page LoadedSource carrying one PageTable with
    /// the given header + data rows. The PageTable's bbox.y is set to
    /// `first_line_y` so `locate_page_table` matches by Y proximity.
    fn loaded_source_with_one_table(
        first_line_y: f32,
        header: Vec<String>,
        data: Vec<Vec<String>>,
    ) -> LoadedSource {
        let mut rows = Vec::with_capacity(data.len() + 1);
        rows.push(header.clone());
        rows.extend(data);
        let table = PageTable {
            bbox: BBox {
                x: 0.0,
                y: first_line_y,
                w: 400.0,
                h: 200.0,
            },
            row_count: rows.len() as u32,
            col_count: header.len() as u32,
            has_header: true,
            header_row: header,
            rows,
        };
        LoadedSource {
            kind: SourceKind::Pdf,
            pages: vec![PageLayout {
                page_number: 1,
                spans: Vec::new(),
                lines: Vec::new(),
                tables: vec![table],
                path_count: 0,
                image_count: 0,
                flat_text: String::new(),
            }],
            pdf: None,
        }
    }

    /// Minimal FormatJson scaffold. Caller appends the TableEntry it
    /// cares about.
    fn empty_format() -> FormatJson {
        FormatJson {
            schema_version: 1,
            model: "test".into(),
            prompt_version: "test".into(),
            source_sha256: "0".into(),
            discovered_at: chrono::Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap(),
            section_roles: Vec::new(),
            tables: Vec::new(),
            figures: Vec::new(),
            glossary: Vec::new(),
            chrome: Vec::new(),
            validation: ValidationBlock::default(),
        }
    }
    // Silence dead_code on the FontWeight re-import; tests do not
    // construct a span in the synthetic fixture so the loading
    // FontWeight alias here is unused yet kept for symmetry with the
    // descriptor side.
    #[allow(dead_code)]
    type _FmtFontWeightUnused = FmtFontWeight;

    fn signal_columns() -> Vec<ColumnMapping> {
        vec![
            ColumnMapping {
                source: "Signal".into(),
                canonical: "name".into(),
            },
            ColumnMapping {
                source: "Direction".into(),
                canonical: "direction".into(),
            },
            ColumnMapping {
                source: "To/From".into(),
                canonical: "peer".into(),
            },
            ColumnMapping {
                source: "Description".into(),
                canonical: "description".into(),
            },
        ]
    }

    fn signal_header() -> Vec<String> {
        vec![
            "Signal".into(),
            "Direction".into(),
            "To/From".into(),
            "Description".into(),
        ]
    }

    #[test]
    fn format_driven_signal_table_emits_block_rows() {
        let loaded = loaded_source_with_one_table(
            17.0,
            signal_header(),
            vec![
                vec![
                    "if_nxt_pc".into(),
                    "out".into(),
                    "Bus".into(),
                    "next addr".into(),
                ],
                vec![
                    "parcel_pc".into(),
                    "in".into(),
                    "Bus".into(),
                    "fetch addr".into(),
                ],
                vec![
                    "if_pc".into(),
                    "out".into(),
                    "Decode".into(),
                    "current pc".into(),
                ],
            ],
        );
        let mut fmt = empty_format();
        fmt.tables.push(TableEntry {
            id: "tbl_001".into(),
            page: 1,
            first_line: 17,
            row_count: 4,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "IF".into(),
            },
            column_map: signal_columns(),
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert_eq!(out.block_signals.len(), 1);
        let group = &out.block_signals[0];
        assert_eq!(group.block_name, "IF");
        assert_eq!(group.rows.len(), 3);
        assert_eq!(group.rows[0].name, "if_nxt_pc");
        assert_eq!(group.rows[0].direction, "out");
        assert_eq!(group.rows[0].peer, "Bus");
        assert_eq!(group.rows[0].description, "next addr");
        assert_eq!(group.rows[2].name, "if_pc");
        // No format-driven warnings expected on a clean fixture.
        assert!(
            !warnings
                .iter()
                .any(|w| w.code.starts_with("wrap_strategy") || w.code.starts_with("classify_")),
            "unexpected warnings: {warnings:?}"
        );
    }

    #[test]
    fn format_driven_signal_table_merges_continuation_rows() {
        // Row 1 + Row 2 (continuation: first column blank, second col
        // also blank but the "peer" column carries a wrapped fragment
        // belonging to row 1).
        let loaded = loaded_source_with_one_table(
            42.0,
            signal_header(),
            vec![
                vec![
                    "if_nxt_pc".into(),
                    "out".into(),
                    "Bus".into(),
                    "next addr".into(),
                ],
                vec![
                    String::new(),
                    String::new(),
                    "Interface".into(),
                    "to fetch".into(),
                ],
                vec![
                    "parcel_pc".into(),
                    "in".into(),
                    "Bus".into(),
                    "fetch addr".into(),
                ],
            ],
        );
        let mut fmt = empty_format();
        fmt.tables.push(TableEntry {
            id: "tbl_002".into(),
            page: 1,
            first_line: 42,
            row_count: 4,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "IF".into(),
            },
            column_map: signal_columns(),
            wrap_strategy: WrapStrategy::MergeContinuationRows,
            rationale: String::new(),
        });
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert_eq!(out.block_signals.len(), 1);
        let group = &out.block_signals[0];
        assert_eq!(
            group.rows.len(),
            2,
            "continuation row should merge into row 1"
        );
        assert_eq!(group.rows[0].name, "if_nxt_pc");
        assert!(
            group.rows[0].peer.contains("Bus") && group.rows[0].peer.contains("Interface"),
            "merged peer was {:?}",
            group.rows[0].peer
        );
        assert!(
            group.rows[0].description.contains("next addr")
                && group.rows[0].description.contains("to fetch"),
            "merged description was {:?}",
            group.rows[0].description
        );
        assert_eq!(group.rows[1].name, "parcel_pc");
        assert!(
            !warnings
                .iter()
                .any(|w| w.code == "wrap_strategy_zero_merges"),
            "should not warn when merges happened",
        );
    }

    #[test]
    fn format_driven_wrap_strategy_zero_merges_warns() {
        // Same SignalTable layout but no continuation rows. With
        // wrap_strategy=merge_continuation_rows we expect the zero-
        // merges warning.
        let loaded = loaded_source_with_one_table(
            55.0,
            signal_header(),
            vec![
                vec!["a".into(), "in".into(), "Bus".into(), "first".into()],
                vec!["b".into(), "out".into(), "Bus".into(), "second".into()],
            ],
        );
        let mut fmt = empty_format();
        fmt.tables.push(TableEntry {
            id: "tbl_003".into(),
            page: 1,
            first_line: 55,
            row_count: 3,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "IF".into(),
            },
            column_map: signal_columns(),
            wrap_strategy: WrapStrategy::MergeContinuationRows,
            rationale: String::new(),
        });
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert_eq!(out.block_signals.len(), 1);
        assert_eq!(out.block_signals[0].rows.len(), 2);
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "wrap_strategy_zero_merges" && w.message.contains("tbl_003")),
            "expected wrap_strategy_zero_merges warning, got {warnings:?}",
        );
    }

    #[test]
    fn format_driven_unknown_canonical_warns_and_skips() {
        let loaded = loaded_source_with_one_table(
            70.0,
            signal_header(),
            vec![vec![
                "clk".into(),
                "in".into(),
                "external".into(),
                "system clock".into(),
            ]],
        );
        let mut fmt = empty_format();
        // Inject an extra mapping with an unknown canonical.
        let mut cols = signal_columns();
        cols.push(ColumnMapping {
            source: "Description".into(),
            canonical: "made_up_field".into(),
        });
        fmt.tables.push(TableEntry {
            id: "tbl_004".into(),
            page: 1,
            first_line: 70,
            row_count: 2,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "IF".into(),
            },
            column_map: cols,
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert_eq!(out.block_signals.len(), 1);
        let row = &out.block_signals[0].rows[0];
        assert_eq!(row.name, "clk");
        // The valid mappings (Signal/Direction/To/From/Description ->
        // name/direction/peer/description) still apply.
        assert_eq!(row.direction, "in");
        assert_eq!(row.peer, "external");
        assert_eq!(row.description, "system clock");
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "classify_unknown_canonical"
                    && w.message.contains("made_up_field")),
            "expected classify_unknown_canonical warning, got {warnings:?}",
        );
    }

    #[test]
    fn format_driven_missing_table_location_warns() {
        // PageLayout has zero tables on page 1; descriptor still
        // references one. Expect classify_table_location_missed.
        let loaded = LoadedSource {
            kind: SourceKind::Pdf,
            pages: vec![PageLayout {
                page_number: 1,
                spans: Vec::new(),
                lines: Vec::new(),
                tables: Vec::new(),
                path_count: 0,
                image_count: 0,
                flat_text: String::new(),
            }],
            pdf: None,
        };
        let mut fmt = empty_format();
        fmt.tables.push(TableEntry {
            id: "tbl_999".into(),
            page: 1,
            first_line: 17,
            row_count: 4,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "IF".into(),
            },
            column_map: signal_columns(),
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert!(out.block_signals.is_empty());
        assert!(
            warnings.iter().any(
                |w| w.code == "classify_table_location_missed" && w.message.contains("tbl_999")
            ),
            "expected classify_table_location_missed warning, got {warnings:?}",
        );
    }

    #[test]
    fn format_none_path_keeps_heuristic_signal_extractor() {
        // Smoke test: the existing PDF-text heuristic still produces
        // signal tables when format=None.
        let body =
            "Some intro.\n\nSignal Direction To/From Description\nif_pc from IF current pc\n";
        let s = section_with_body(body);
        let mut tree = SectionTree::default();
        tree.roots.push(s);
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(None, &mut tree, &cfg, None, &mut warnings);
        assert!(
            !out.signals.is_empty(),
            "expected heuristic to extract at least one SignalTable",
        );
        assert!(
            out.block_signals.is_empty(),
            "format-driven block_signals must stay empty on the None path",
        );
    }

    #[test]
    fn format_driven_unknown_kind_emits_raw_rows() {
        // Catch-all for kinds the dispatch table does not handle —
        // they round-trip into ClassifyOutputs.unknown_tables so DM0
        // can ask the user.
        let loaded = loaded_source_with_one_table(
            12.0,
            vec!["A".into(), "B".into()],
            vec![vec!["x".into(), "y".into()]],
        );
        let mut fmt = empty_format();
        fmt.tables.push(TableEntry {
            id: "tbl_unknown".into(),
            page: 1,
            first_line: 12,
            row_count: 2,
            col_count: 2,
            kind: TableKind::Unknown,
            spec_md_target: TableTarget::Unknown,
            column_map: Vec::new(),
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });
        // Silence an unused import warning when nothing references
        // BTreeMap in this test module.
        let _ = BTreeMap::<String, u32>::new();
        let mut tree = SectionTree::default();
        let cfg = crate::session::spec_ingest::pipeline::IngestConfig::default();
        let mut warnings = Vec::new();
        let out = classify_with_format(Some(&loaded), &mut tree, &cfg, Some(&fmt), &mut warnings);
        assert_eq!(out.unknown_tables.len(), 1);
        assert_eq!(out.unknown_tables[0].table_id, "tbl_unknown");
        assert_eq!(out.unknown_tables[0].rows.len(), 1);
        assert_eq!(
            out.unknown_tables[0].rows[0].cells,
            vec!["x".to_string(), "y".to_string()]
        );
    }
}
