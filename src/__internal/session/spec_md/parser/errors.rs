//! Parser for the `## Error Handling` section (Chapter 2 §2.3.11).
//! Single typed table with columns:
//! Error type / Detecting component / Detection behavior /
//! Bus response / Master behavior / Software response / Source-anchor.

use super::SpecMdParseError;
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::ErrorEntry;

pub(crate) fn parse_errors(body: &str) -> Result<Vec<ErrorEntry>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let idxs = t.require_columns(&[
        (CanonicalColumn::ErrorType, "Error type"),
        (CanonicalColumn::DetectingComponent, "Detecting component"),
        (CanonicalColumn::DetectionBehavior, "Detection behavior"),
        (CanonicalColumn::BusResponse, "Bus response"),
        (CanonicalColumn::MasterBehavior, "Master behavior"),
        (CanonicalColumn::SoftwareResponse, "Software response"),
        (CanonicalColumn::SourceAnchor, "Source-anchor"),
    ])?;
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(ErrorEntry {
            error_type: t.cell(row, idxs[0]).to_string(),
            detecting_component: t.cell(row, idxs[1]).to_string(),
            detection_behavior: t.cell(row, idxs[2]).to_string(),
            bus_response: t.cell(row, idxs[3]).to_string(),
            master_behavior: t.cell(row, idxs[4]).to_string(),
            software_response: t.cell(row, idxs[5]).to_string(),
            source_anchor: t.cell(row, idxs[6]).to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_error_table() {
        let body = "\
## Error Handling

| Error type | Detecting component | Detection behavior | Bus response | Master behavior | Software response | Source-anchor |
| --- | --- | --- | --- | --- | --- | --- |
| Wrong address | NoC | Log Error | Bus error | Abort | Interrupt | primary:p28 |
";
        let errs = parse_errors(body).expect("parses");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].error_type, "Wrong address");
        assert_eq!(errs[0].source_anchor, "primary:p28");
    }
}
