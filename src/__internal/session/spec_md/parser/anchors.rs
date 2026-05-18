//! Parser for the `## Source-Spec Anchors` index table
//! (Chapter 2 §2.3.19). Four columns:
//! spec.md section / Source / Chunk id / Page range.

use super::SpecMdParseError;
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::AnchorIndexEntry;

pub(crate) fn parse_anchors(body: &str) -> Result<Vec<AnchorIndexEntry>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let idxs = t.require_columns(&[
        (CanonicalColumn::SectionPath, "spec.md section"),
        (CanonicalColumn::Source, "Source"),
        (CanonicalColumn::ChunkId, "Chunk id"),
        (CanonicalColumn::PageRange, "Page range"),
    ])?;
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(AnchorIndexEntry {
            section_path: t.cell(row, idxs[0]).to_string(),
            source: t.cell(row, idxs[1]).to_string(),
            chunk_id: t.cell(row, idxs[2]).to_string(),
            page_range: t.cell(row, idxs[3]).to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anchor_index() {
        let body = "\
## Source-Spec Anchors

| spec.md section | Source | Chunk id | Page range |
| --- | --- | --- | --- |
| External Interfaces > Instruction Interface | primary | chunk-0042 | 2-3 |
| Blocks > Instruction Fetch (IF) | primary | chunk-0118 | 12-14 |
";
        let xs = parse_anchors(body).expect("parses");
        assert_eq!(xs.len(), 2);
        assert!(xs[0].section_path.contains("Instruction Interface"));
        assert_eq!(xs[0].source, "primary");
        assert_eq!(xs[0].chunk_id, "chunk-0042");
        assert_eq!(xs[0].page_range, "2-3");
    }
}
