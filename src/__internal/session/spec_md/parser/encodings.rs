//! Parser for the `## Encodings` section (Chapter 2 §2.3.8).
//! Each encoding is `### Encoding: <field>` with a bold-property
//! block (Bit width / Source-anchor) plus a Value / Name /
//! Abbreviation table and an optional reserved / illegal line.

use super::SpecMdParseError;
use super::section_util::{parse_bold_properties, split_h3};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{Encoding, EncodingValue};

pub(crate) fn parse_encodings(body: &str) -> Result<Vec<Encoding>, SpecMdParseError> {
    let mut out: Vec<Encoding> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(field) = sub.heading.strip_prefix("Encoding:") else {
            continue;
        };
        let mut enc = Encoding {
            field: field.trim().to_string(),
            ..Encoding::default()
        };
        for (k, v) in parse_bold_properties(&sub.body) {
            match k.to_ascii_lowercase().as_str() {
                "bit width" => enc.bit_width = v,
                "source-anchor" | "source anchor" => enc.source_anchor = v,
                _ => {}
            }
        }
        let tables = MarkdownTable::parse_all(&sub.body)?;
        if let Some(t) = tables.first() {
            let idxs = t.require_columns(&[
                (CanonicalColumn::Value, "Value"),
                (CanonicalColumn::Signal, "Name"),
                (CanonicalColumn::Abbreviation, "Abbreviation"),
            ])?;
            for row in &t.rows {
                enc.values.push(EncodingValue {
                    value: t.cell(row, idxs[0]).trim_matches('`').to_string(),
                    name: t.cell(row, idxs[1]).to_string(),
                    abbreviation: t.cell(row, idxs[2]).to_string(),
                });
            }
        }
        // Reserved / illegal line.
        for line in sub.body.lines() {
            let trimmed = line.trim();
            let lc = trimmed.to_ascii_lowercase();
            if let Some(rest) = lc
                .strip_prefix("reserved / illegal:")
                .or_else(|| lc.strip_prefix("reserved:"))
                .or_else(|| lc.strip_prefix("reserved/illegal:"))
            {
                // Preserve original case for the value.
                let _ = rest;
                let prefix_len = trimmed.find(':').map(|p| p + 1).unwrap_or(0);
                enc.reserved = trimmed[prefix_len..]
                    .trim()
                    .trim_end_matches('.')
                    .to_string();
                break;
            }
        }
        out.push(enc);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_encoding() {
        let body = "\
## Encodings

### Encoding: Privilege Level

**Bit width:** 2
**Source-anchor:** primary:p5

| Value | Name | Abbreviation |
| --- | --- | --- |
| `00` | User/Application | U |
| `01` | Supervisor | S |
| `10` | Hypervisor | H |
| `11` | Machine | M |

Reserved / illegal: none.
";
        let encs = parse_encodings(body).expect("parses");
        assert_eq!(encs.len(), 1);
        let e = &encs[0];
        assert_eq!(e.field, "Privilege Level");
        assert_eq!(e.bit_width, "2");
        assert_eq!(e.source_anchor, "primary:p5");
        assert_eq!(e.values.len(), 4);
        assert_eq!(e.values[0].value, "00");
        assert_eq!(e.values[0].name, "User/Application");
        assert_eq!(e.values[0].abbreviation, "U");
        assert_eq!(e.reserved, "none");
    }
}
