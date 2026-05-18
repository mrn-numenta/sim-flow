//! Parser for the `## Assumptions and Constraints` section
//! (Chapter 2 §2.3.3). The section has a `### Quantitative` table
//! plus two prose subsections `### Environmental` and
//! `### Architectural`.

use super::SpecMdParseError;
use super::section_util::{collect_prose, split_h3};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{AssumptionsAndConstraints, QuantitativeRow};

pub(crate) fn parse_assumptions(body: &str) -> Result<AssumptionsAndConstraints, SpecMdParseError> {
    let mut out = AssumptionsAndConstraints::default();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        match sub.heading.to_ascii_lowercase().as_str() {
            "quantitative" => {
                let tables = MarkdownTable::parse_all(&sub.body)?;
                if let Some(t) = tables.first() {
                    let idxs = t.require_columns(&[
                        (CanonicalColumn::Constraint, "Constraint"),
                        (CanonicalColumn::Value, "Value"),
                        (CanonicalColumn::SourceAnchor, "Source-anchor"),
                    ])?;
                    for row in &t.rows {
                        out.quantitative.push(QuantitativeRow {
                            constraint: t.cell(row, idxs[0]).to_string(),
                            value: t.cell(row, idxs[1]).to_string(),
                            source_anchor: t.cell(row, idxs[2]).to_string(),
                        });
                    }
                }
            }
            "environmental" | "environmental assumptions" => {
                out.environmental = collect_prose(&sub.body);
            }
            "architectural" | "architectural constraints" => {
                out.architectural = collect_prose(&sub.body);
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quantitative_and_prose() {
        let body = "\
## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Technology node | 7nm | source primary p3 |
| Clock frequency | 1 GHz | source primary p3 |
| Gate budget per cycle | 50-100 | derived |

### Environmental

The block sits on a 1 GHz core clock.

### Architectural

Six-stage folded pipeline.
";
        let a = parse_assumptions(body).expect("parses");
        assert_eq!(a.quantitative.len(), 3);
        assert_eq!(a.quantitative[1].constraint, "Clock frequency");
        assert_eq!(a.quantitative[1].value, "1 GHz");
        assert!(a.environmental.contains("1 GHz core clock"));
        assert!(a.architectural.contains("Six-stage folded"));
    }

    #[test]
    fn empty_body_is_default() {
        let body = "## Assumptions and Constraints\n";
        let a = parse_assumptions(body).expect("parses");
        assert_eq!(a, AssumptionsAndConstraints::default());
    }
}
