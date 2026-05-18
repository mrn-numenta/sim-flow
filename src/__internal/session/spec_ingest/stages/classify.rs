//! Stage 4: structural classification.
//!
//! Annotates the section tree with `kind`, detects stubs and TBDs,
//! and extracts structured tables (signal, parameter, error,
//! encoding, FSM). Per architecture §1.4 stage 4 each kind has its
//! own header-row matcher; mismatches stay as markdown.

use super::super::pipeline::{IngestConfig, IngestWarning};
use super::parse::{Section, SectionKind, SectionTree};

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
}

/// Entry point. Walks the tree, mutating each section to record its
/// kind / extracted-table refs / tbd counts and returning the
/// aggregated classify outputs.
pub fn classify(
    tree: &mut SectionTree,
    _config: &IngestConfig,
    _warnings: &mut Vec<IngestWarning>,
) -> ClassifyOutputs {
    let mut out = ClassifyOutputs::default();
    classify_sections(&mut tree.roots, &mut out);
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
}
