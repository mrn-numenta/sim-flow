//! Parser for the `## CSRs` section (Chapter 7 §7.7).
//!
//! Each CSR is `### CSR: <name>` with a bold-property block
//! (Address / Access / Reset value / Required privilege /
//! Source-anchor) plus optional H4 subsections:
//! `#### Description` (prose), `#### Fields` (Bits / Name /
//! Access / Description table).

use super::SpecMdParseError;
use super::section_util::{collect_prose, parse_bold_properties, split_h3, split_h4};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{Csr, CsrField};

pub(crate) fn parse_csrs(body: &str) -> Result<Vec<Csr>, SpecMdParseError> {
    let mut out: Vec<Csr> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("CSR:") else {
            continue;
        };
        let mut csr = Csr {
            name: name.trim().to_string(),
            ..Csr::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "address" => csr.address = v,
                "access" => csr.access = v,
                "reset value" | "reset" => csr.reset_value = v,
                "required privilege" | "privilege" => csr.required_privilege = v,
                "source-anchor" | "source anchor" => csr.source_anchor = v,
                _ => {}
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "description" => {
                    csr.description = collect_prose(&h4.body);
                }
                "fields" => {
                    let tables = MarkdownTable::parse_all(&h4.body)?;
                    if let Some(t) = tables.first() {
                        csr.fields = parse_csr_field_rows(t)?;
                    }
                }
                _ => {}
            }
        }
        out.push(csr);
    }
    Ok(out)
}

fn parse_csr_field_rows(t: &MarkdownTable) -> Result<Vec<CsrField>, SpecMdParseError> {
    let bits_idx = t
        .headers
        .iter()
        .position(|h| {
            let lc = h.to_ascii_lowercase();
            lc == "bits" || lc == "bit" || lc == "field"
        })
        .ok_or_else(|| SpecMdParseError::MalformedTable {
            message: format!(
                "missing required column `Bits`; headers were [{}]",
                t.headers.join(", ")
            ),
            line: 0,
            column: 0,
        })?;
    let name_idx = t.require_columns(&[(CanonicalColumn::Signal, "Name")])?;
    let access_idx = t.optional_column(CanonicalColumn::Access);
    let desc_idx = t.column_index(CanonicalColumn::Description);
    let mut rows = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        rows.push(CsrField {
            bits: t.cell(row, bits_idx).trim_matches('`').to_string(),
            name: t.cell(row, name_idx[0]).trim_matches('`').to_string(),
            access: access_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
            description: desc_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csr_with_fields() {
        let body = "\
## CSRs

### CSR: mstatus

**Address:** 0x300
**Access:** RW
**Reset value:** 0x0
**Required privilege:** M
**Source-anchor:** primary:p43

#### Description

Machine status register.

#### Fields

| Bits | Name | Access | Description |
| --- | --- | --- | --- |
| 3 | MIE | RW | Machine interrupt enable |
| 7 | MPIE | RW | Prior interrupt enable |
";
        let csrs = parse_csrs(body).expect("parses");
        assert_eq!(csrs.len(), 1);
        let c = &csrs[0];
        assert_eq!(c.name, "mstatus");
        assert_eq!(c.address, "0x300");
        assert_eq!(c.access, "RW");
        assert_eq!(c.reset_value, "0x0");
        assert_eq!(c.required_privilege, "M");
        assert_eq!(c.source_anchor, "primary:p43");
        assert!(c.description.contains("Machine status"));
        assert_eq!(c.fields.len(), 2);
        assert_eq!(c.fields[0].bits, "3");
        assert_eq!(c.fields[0].name, "MIE");
        assert_eq!(c.fields[0].access, "RW");
        assert_eq!(c.fields[1].name, "MPIE");
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(parse_csrs("## CSRs\n").expect("parses").is_empty());
    }
}
