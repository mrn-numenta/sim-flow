//! Parser for the `## Glossary` section (Chapter 7 §7.7).
//!
//! Single typed table with columns: Term / Expansion / Scope /
//! Used in / Source-anchor. The optional "Used in" column is a
//! comma-separated list of block names.

use super::SpecMdParseError;
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::GlossaryEntry;

pub(crate) fn parse_glossary(body: &str) -> Result<Vec<GlossaryEntry>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let term_idx = t
        .headers
        .iter()
        .position(|h| {
            let lc = h.to_ascii_lowercase();
            lc == "term" || lc == "acronym"
        })
        .ok_or_else(|| missing("Term", &t.headers))?;
    let expansion_idx = t
        .headers
        .iter()
        .position(|h| {
            let lc = h.to_ascii_lowercase();
            lc == "expansion" || lc == "definition" || lc == "meaning"
        })
        .ok_or_else(|| missing("Expansion", &t.headers))?;
    let scope_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "scope"
    });
    let used_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "used in" || lc == "used in blocks"
    });
    let anchor_idx = t.optional_column(CanonicalColumn::SourceAnchor);
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(GlossaryEntry {
            term: t.cell(row, term_idx).to_string(),
            expansion: t.cell(row, expansion_idx).to_string(),
            scope: scope_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
            used_in_blocks: used_idx
                .map(|i| split_used_in(t.cell(row, i)))
                .unwrap_or_default(),
            source_anchor: anchor_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

fn missing(label: &str, headers: &[String]) -> SpecMdParseError {
    SpecMdParseError::MalformedTable {
        message: format!(
            "missing required column `{label}`; headers were [{}]",
            headers.join(", ")
        ),
        line: 0,
        column: 0,
    }
}

fn split_used_in(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_glossary_table() {
        let body = "\
## Glossary

| Term | Expansion | Scope | Used in | Source-anchor |
| --- | --- | --- | --- | --- |
| IF | Instruction Fetch | spec | Instruction Fetch (IF) | primary:p11 |
| CSR | Control and Status Register | spec | mstatus, mtvec | primary:p43 |
";
        let g = parse_glossary(body).expect("parses");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].term, "IF");
        assert_eq!(g[0].expansion, "Instruction Fetch");
        assert_eq!(g[0].scope, "spec");
        assert_eq!(g[0].used_in_blocks, vec!["Instruction Fetch (IF)"]);
        assert_eq!(g[0].source_anchor, "primary:p11");
        assert_eq!(g[1].term, "CSR");
        assert_eq!(g[1].used_in_blocks, vec!["mstatus", "mtvec"]);
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(parse_glossary("## Glossary\n").expect("parses").is_empty());
    }
}
