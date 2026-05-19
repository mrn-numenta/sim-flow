//! Parser for the `## Memory Map` section (Chapter 2 §2.3.9).
//! Single typed table with columns:
//! Start / End / Name / Purpose / Access / Source-anchor.

use super::SpecMdParseError;
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::MemoryRegion;

pub(crate) fn parse_memory_map(body: &str) -> Result<Vec<MemoryRegion>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let idxs = t.require_columns(&[
        (CanonicalColumn::Start, "Start"),
        (CanonicalColumn::End, "End"),
        (CanonicalColumn::Signal, "Name"),
        (CanonicalColumn::Purpose, "Purpose"),
        (CanonicalColumn::Access, "Access"),
        (CanonicalColumn::SourceAnchor, "Source-anchor"),
    ])?;
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(MemoryRegion {
            start: t.cell(row, idxs[0]).trim_matches('`').to_string(),
            end: t.cell(row, idxs[1]).trim_matches('`').to_string(),
            name: t.cell(row, idxs[2]).to_string(),
            purpose: t.cell(row, idxs[3]).to_string(),
            access: t.cell(row, idxs[4]).to_string(),
            source_anchor: t.cell(row, idxs[5]).to_string(),
            ..Default::default()
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_map() {
        let body = "\
## Memory Map

| Start | End | Name | Purpose | Access | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `0x0000_0000` | `0x0FFF_FFFF` | BootROM | Initial boot code | R | primary:p10 |
| `0x1000_0000` | `0x1FFF_FFFF` | SRAM | System RAM | RW | primary:p11 |
";
        let m = parse_memory_map(body).expect("parses");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].start, "0x0000_0000");
        assert_eq!(m[0].name, "BootROM");
        assert_eq!(m[1].access, "RW");
    }
}
