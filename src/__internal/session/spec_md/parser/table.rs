//! Markdown table parsing helper.
//!
//! The per-section parsers all consume `pulldown_cmark` GFM tables.
//! This module centralises:
//!
//! - Extracting the typed `MarkdownTable` (headers + body rows) from
//!   an event stream.
//! - Normalizing column headers against the alias rules in Chapter 2
//!   §2.5.
//! - Helper accessors (`require_columns`, `optional_column`) so
//!   per-section parsers can ask for cells by canonical name without
//!   re-implementing the index math each time.
//!
//! `MarkdownTable` is intentionally string-typed -- the per-section
//! parsers do type coercion (e.g. `Required: yes/no` to `bool`)
//! themselves so each section's coercion rules live next to its
//! struct definition.

// Per-section parsers consume this helper module from M1.4 onward;
// until then the only callers live in the in-module unit tests.
#![allow(dead_code)]

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::SpecMdParseError;
use super::cmark_options;

/// All canonical columns recognized by the spec.md schema. Header
/// aliasing (Chapter 2 §2.5) normalises to one of these so per-section
/// parsers can look up cells by a stable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CanonicalColumn {
    // Signal-table columns (both external interfaces and blocks).
    Signal,
    Direction,
    Width,
    Type,
    Required,
    Peer,
    Description,
    // Parameter table.
    Name,
    Default,
    ValidRange,
    BehavioralImpact,
    SourceAnchor,
    // Encoding table.
    Value,
    Abbreviation,
    // Quantitative table.
    Constraint,
    // Memory map.
    Start,
    End,
    Purpose,
    Access,
    // FSM transitions.
    From,
    Input,
    To,
    Output,
    // Connectivity nodes.
    Id,
    Coordinate,
    Role,
    // Connectivity edges.
    Channel,
    // Error table.
    ErrorType,
    DetectingComponent,
    DetectionBehavior,
    BusResponse,
    MasterBehavior,
    SoftwareResponse,
    // Latency.
    Operation,
    BestCase,
    WorstCase,
    Notes,
    // Source-spec anchor index.
    SectionPath,
    Source,
    ChunkId,
    PageRange,
    // Figure elements.
    Element,
    Kind,
    /// Any header text that did not match a canonical column. The
    /// per-section parser may decide whether to accept (e.g. the
    /// dynamic stage columns in a cycle-accurate table) or reject.
    Other(&'static str),
}

/// A parsed GFM table: headers (original text + canonical form) plus
/// body rows whose cell count matches the header count.
#[derive(Debug, Clone, Default)]
pub(crate) struct MarkdownTable {
    /// Header cells as written in the markdown (post-trim).
    pub headers: Vec<String>,
    /// Canonical column id for each header cell, in the same order
    /// as [`Self::headers`]. Header text not matched by any canonical
    /// column ends up as `CanonicalColumn::Other(...)`. Empty until
    /// `normalize_headers` is called.
    pub canonical: Vec<CanonicalColumn>,
    /// Body rows. Every row has `headers.len()` cells (rows with
    /// fewer cells are right-padded with empty strings; rows with
    /// more cells trigger a `MalformedTable` error at parse time).
    pub rows: Vec<Vec<String>>,
}

impl MarkdownTable {
    /// Extract every GFM table found in `body` (a section body), in
    /// document order. Returns an empty `Vec` if there are no tables.
    pub(crate) fn parse_all(body: &str) -> Result<Vec<MarkdownTable>, SpecMdParseError> {
        let parser = Parser::new_ext(body, cmark_options());
        let mut tables: Vec<MarkdownTable> = Vec::new();
        let mut current: Option<MarkdownTable> = None;
        let mut in_header = false;
        let mut current_row: Vec<String> = Vec::new();
        let mut current_cell = String::new();
        let mut in_cell = false;
        for event in parser {
            match event {
                Event::Start(Tag::Table(_)) => {
                    current = Some(MarkdownTable::default());
                }
                Event::Start(Tag::TableHead) => {
                    in_header = true;
                }
                Event::End(TagEnd::TableHead) => {
                    in_header = false;
                }
                Event::Start(Tag::TableRow) => {
                    current_row.clear();
                }
                Event::End(TagEnd::TableRow) => {
                    if let Some(table) = current.as_mut()
                        && !in_header
                    {
                        // Pad short rows; reject overlong rows.
                        if current_row.len() > table.headers.len() {
                            return Err(SpecMdParseError::MalformedTable {
                                message: format!(
                                    "row has {} cells but table has {} headers",
                                    current_row.len(),
                                    table.headers.len()
                                ),
                                line: 0,
                                column: 0,
                            });
                        }
                        while current_row.len() < table.headers.len() {
                            current_row.push(String::new());
                        }
                        table.rows.push(std::mem::take(&mut current_row));
                    }
                }
                Event::Start(Tag::TableCell) => {
                    current_cell.clear();
                    in_cell = true;
                }
                Event::End(TagEnd::TableCell) => {
                    in_cell = false;
                    let cell = std::mem::take(&mut current_cell).trim().to_string();
                    if in_header {
                        if let Some(table) = current.as_mut() {
                            table.headers.push(cell);
                        }
                    } else {
                        current_row.push(cell);
                    }
                }
                Event::Text(t) | Event::Code(t) if in_cell => {
                    current_cell.push_str(&t);
                }
                Event::End(TagEnd::Table) => {
                    if let Some(mut table) = current.take() {
                        table.normalize_headers();
                        tables.push(table);
                    }
                }
                _ => {}
            }
        }
        Ok(tables)
    }

    fn normalize_headers(&mut self) {
        self.canonical = self.headers.iter().map(|h| normalize_header(h)).collect();
    }

    /// Index of the first column matching `col`, or `None`.
    pub(crate) fn column_index(&self, col: CanonicalColumn) -> Option<usize> {
        self.canonical.iter().position(|c| *c == col)
    }

    /// Assert every column in `required` is present, returning their
    /// indexes in the same order. Errors out on first miss.
    pub(crate) fn require_columns(
        &self,
        required: &[(CanonicalColumn, &'static str)],
    ) -> Result<Vec<usize>, SpecMdParseError> {
        let mut out: Vec<usize> = Vec::new();
        for (col, label) in required {
            match self.column_index(*col) {
                Some(idx) => out.push(idx),
                None => {
                    return Err(SpecMdParseError::MalformedTable {
                        message: format!(
                            "missing required column `{label}`; headers were [{}]",
                            self.headers.join(", ")
                        ),
                        line: 0,
                        column: 0,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Index of a column when it exists (returns `None` otherwise).
    /// Sugar over [`Self::column_index`] for symmetry with
    /// [`Self::require_columns`].
    pub(crate) fn optional_column(&self, col: CanonicalColumn) -> Option<usize> {
        self.column_index(col)
    }

    /// Cell value at `(row, idx)` or `""` if `idx` is out of bounds
    /// for that row (which shouldn't happen because rows are padded).
    pub(crate) fn cell<'a>(&'a self, row: &'a [String], idx: usize) -> &'a str {
        row.get(idx).map(String::as_str).unwrap_or("")
    }
}

/// Map a header string to a canonical column. Recognises the aliases
/// listed in Chapter 2 §2.5. Falls back to `Other(...)` for unknown
/// headers; callers that care emit a warning or hard error.
pub(crate) fn normalize_header(name: &str) -> CanonicalColumn {
    let trimmed = name.trim();
    let normalized = trimmed.to_ascii_lowercase();
    match normalized.as_str() {
        // Signal table.
        "signal" | "name" | "identifier" => CanonicalColumn::Signal,
        "direction" | "dir" => CanonicalColumn::Direction,
        "width" => CanonicalColumn::Width,
        "type" | "type / encoding" | "type/encoding" => CanonicalColumn::Type,
        "required" => CanonicalColumn::Required,
        "peer" | "to/from" | "from/to" | "connected to" => CanonicalColumn::Peer,
        "description" | "notes" | "meaning" => CanonicalColumn::Description,
        // Parameter table.
        "default" => CanonicalColumn::Default,
        "valid range" | "range" | "values" => CanonicalColumn::ValidRange,
        "behavioral impact" | "impact" | "effect" => CanonicalColumn::BehavioralImpact,
        "source-anchor" | "source anchor" => CanonicalColumn::SourceAnchor,
        // Encoding.
        "value" => CanonicalColumn::Value,
        "abbreviation" | "abbr" => CanonicalColumn::Abbreviation,
        // Quantitative.
        "constraint" => CanonicalColumn::Constraint,
        // Memory map.
        "start" => CanonicalColumn::Start,
        "end" => CanonicalColumn::End,
        "purpose" => CanonicalColumn::Purpose,
        "access" => CanonicalColumn::Access,
        // FSM transitions.
        "from" => CanonicalColumn::From,
        "input" | "input/event" => CanonicalColumn::Input,
        "to" => CanonicalColumn::To,
        "output" | "output/action" => CanonicalColumn::Output,
        // Connectivity nodes.
        "id" => CanonicalColumn::Id,
        "coordinate" => CanonicalColumn::Coordinate,
        "role" => CanonicalColumn::Role,
        // Connectivity edges.
        "channel" => CanonicalColumn::Channel,
        // Error table.
        "error type" => CanonicalColumn::ErrorType,
        "detecting component" => CanonicalColumn::DetectingComponent,
        "detection behavior" => CanonicalColumn::DetectionBehavior,
        "bus response" => CanonicalColumn::BusResponse,
        "master behavior" => CanonicalColumn::MasterBehavior,
        "software response" => CanonicalColumn::SoftwareResponse,
        // Latency.
        "operation" => CanonicalColumn::Operation,
        "best-case" | "best case" => CanonicalColumn::BestCase,
        "worst-case" | "worst case" => CanonicalColumn::WorstCase,
        // Source-spec anchor index.
        "spec.md section" => CanonicalColumn::SectionPath,
        "source" => CanonicalColumn::Source,
        "chunk id" => CanonicalColumn::ChunkId,
        "page range" => CanonicalColumn::PageRange,
        // Figure elements.
        "element" => CanonicalColumn::Element,
        "kind" => CanonicalColumn::Kind,
        _ => CanonicalColumn::Other(Box::leak(trimmed.to_string().into_boxed_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signal_table() {
        let body = "\
| Signal | Direction | Width | Type | Required | Description |
| --- | --- | --- | --- | --- | --- |
| `inst_addr` | out | XLEN | logic | yes | Instruction address |
| `inst_data` | in | XLEN | logic | yes | Fetched instruction |
";
        let tables = MarkdownTable::parse_all(body).expect("parses");
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.headers.len(), 6);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0][0], "inst_addr");
        assert_eq!(t.rows[0][4], "yes");
        assert_eq!(t.column_index(CanonicalColumn::Required), Some(4));
        assert_eq!(t.column_index(CanonicalColumn::Description), Some(5));
    }

    #[test]
    fn parses_parameter_table() {
        let body = "\
| Name | Type | Default | Valid range | Behavioral impact | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `XLEN` | int | 32 | 32 \\| 64 | width | primary:p3 |
| `HAS_BPU` | bool | true | true \\| false | enable bpu | primary:p9 |
";
        let tables = MarkdownTable::parse_all(body).expect("parses");
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        let idxs = t
            .require_columns(&[
                (CanonicalColumn::Signal, "Name"),
                (CanonicalColumn::Type, "Type"),
                (CanonicalColumn::Default, "Default"),
                (CanonicalColumn::ValidRange, "Valid range"),
                (CanonicalColumn::BehavioralImpact, "Behavioral impact"),
                (CanonicalColumn::SourceAnchor, "Source-anchor"),
            ])
            .expect("required columns present");
        assert_eq!(idxs, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(t.rows[0][0], "XLEN");
    }

    #[test]
    fn parses_error_table() {
        let body = "\
| Error type | Detecting component | Detection behavior | Bus response | Master behavior | Software response | Source-anchor |
| --- | --- | --- | --- | --- | --- | --- |
| Wrong address | NoC | Log | Bus error | Abort | Interrupt | primary:p28 |
";
        let tables = MarkdownTable::parse_all(body).expect("parses");
        let t = &tables[0];
        assert_eq!(t.headers.len(), 7);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.column_index(CanonicalColumn::ErrorType), Some(0));
        assert_eq!(t.column_index(CanonicalColumn::SourceAnchor), Some(6));
    }

    #[test]
    fn header_aliases_resolve_to_canonical() {
        assert_eq!(normalize_header("Dir"), CanonicalColumn::Direction);
        assert_eq!(normalize_header("Notes"), CanonicalColumn::Description);
        assert_eq!(normalize_header("To/From"), CanonicalColumn::Peer);
        assert_eq!(normalize_header("Range"), CanonicalColumn::ValidRange);
        assert_eq!(normalize_header("Abbr"), CanonicalColumn::Abbreviation);
    }

    #[test]
    fn missing_required_column_errors() {
        let body = "\
| Signal | Direction |
| --- | --- |
| a | in |
";
        let tables = MarkdownTable::parse_all(body).expect("parses");
        let err = tables[0]
            .require_columns(&[(CanonicalColumn::Description, "Description")])
            .expect_err("missing column should error");
        match err {
            SpecMdParseError::MalformedTable { message, .. } => {
                assert!(message.contains("Description"), "{message}");
            }
            _ => panic!("expected MalformedTable"),
        }
    }
}
