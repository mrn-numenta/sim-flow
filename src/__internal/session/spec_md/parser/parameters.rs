//! Parser for the `## Parameters` section (Chapter 2 §2.3.6).
//! A single typed table with columns:
//! Name / Type / Default / Valid range / Behavioral impact /
//! Source-anchor.

use super::SpecMdParseError;
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::Parameter;

pub(crate) fn parse_parameters(body: &str) -> Result<Vec<Parameter>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let idxs = t.require_columns(&[
        (CanonicalColumn::Signal, "Name"),
        (CanonicalColumn::Type, "Type"),
        (CanonicalColumn::Default, "Default"),
        (CanonicalColumn::ValidRange, "Valid range"),
        (CanonicalColumn::BehavioralImpact, "Behavioral impact"),
        (CanonicalColumn::SourceAnchor, "Source-anchor"),
    ])?;
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(Parameter {
            name: t.cell(row, idxs[0]).trim_matches('`').to_string(),
            ty: t.cell(row, idxs[1]).to_string(),
            default: t.cell(row, idxs[2]).to_string(),
            valid_range: t.cell(row, idxs[3]).to_string(),
            behavioral_impact: t.cell(row, idxs[4]).to_string(),
            source_anchor: t.cell(row, idxs[5]).to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_five_parameters() {
        let body = "\
## Parameters

| Name | Type | Default | Valid range | Behavioral impact | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `XLEN` | int | 32 | 32 \\| 64 | width | primary:p3 |
| `HAS_BPU` | bool | true | true \\| false | bpu | primary:p9 |
| `BPU_LOCAL_BITS` | int | 8 | 0..16 | bpu bits | primary:p9 |
| `RESET_VECTOR` | int | 0x80000000 | any | reset PC | primary:p3 |
| `HAS_RVC` | bool | true | true \\| false | compressed | primary:p9 |
";
        let params = parse_parameters(body).expect("parses");
        assert_eq!(params.len(), 5);
        assert_eq!(params[0].name, "XLEN");
        assert_eq!(params[0].ty, "int");
        assert_eq!(params[0].default, "32");
        assert_eq!(params[0].valid_range, "32 | 64");
        assert_eq!(params[0].source_anchor, "primary:p3");
        assert_eq!(params[4].name, "HAS_RVC");
    }

    #[test]
    fn empty_body_yields_empty() {
        let body = "## Parameters\n";
        assert!(parse_parameters(body).expect("parses").is_empty());
    }
}
