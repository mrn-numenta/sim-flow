//! Parser for the `## Connectivity` section (Chapter 2 §2.3.10).
//! Three H3 subsections: `Nodes`, `Edges`, `Routing rules`.

use super::SpecMdParseError;
use super::section_util::{collect_prose, split_h3};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{Connectivity, Edge, Node};

pub(crate) fn parse_connectivity(body: &str) -> Result<Option<Connectivity>, SpecMdParseError> {
    let mut c = Connectivity::default();
    let (_pre, subs) = split_h3(body);
    if subs.is_empty() {
        return Ok(None);
    }
    for sub in subs {
        match sub.heading.to_ascii_lowercase().as_str() {
            "nodes" => {
                let tables = MarkdownTable::parse_all(&sub.body)?;
                if let Some(t) = tables.first() {
                    let idxs = t.require_columns(&[
                        (CanonicalColumn::Id, "Id"),
                        (CanonicalColumn::Type, "Type"),
                        (CanonicalColumn::Coordinate, "Coordinate"),
                        (CanonicalColumn::Role, "Role"),
                    ])?;
                    for row in &t.rows {
                        c.nodes.push(Node {
                            id: t.cell(row, idxs[0]).trim_matches('`').to_string(),
                            ty: t.cell(row, idxs[1]).to_string(),
                            coordinate: t.cell(row, idxs[2]).trim_matches('`').to_string(),
                            role: t.cell(row, idxs[3]).to_string(),
                        });
                    }
                }
            }
            "edges" => {
                let tables = MarkdownTable::parse_all(&sub.body)?;
                if let Some(t) = tables.first() {
                    let idxs = t.require_columns(&[
                        (CanonicalColumn::From, "From"),
                        (CanonicalColumn::To, "To"),
                        (CanonicalColumn::Channel, "Channel"),
                        (CanonicalColumn::SourceAnchor, "Source-anchor"),
                    ])?;
                    for row in &t.rows {
                        c.edges.push(Edge {
                            from: t.cell(row, idxs[0]).trim_matches('`').to_string(),
                            to: t.cell(row, idxs[1]).trim_matches('`').to_string(),
                            channel: t.cell(row, idxs[2]).to_string(),
                            source_anchor: t.cell(row, idxs[3]).to_string(),
                        });
                    }
                }
            }
            "routing rules" => {
                c.routing_rules = collect_prose(&sub.body);
            }
            _ => {}
        }
    }
    Ok(Some(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connectivity() {
        let body = "\
## Connectivity

### Nodes

| Id | Type | Coordinate | Role |
| --- | --- | --- | --- |
| `CE0` | compute | `(1,3)` | Compute Engine |
| `ME0` | memory | `(0,3)` | Memory Engine |

### Edges

| From | To | Channel | Source-anchor |
| --- | --- | --- | --- |
| `CE0` | `ME0` | direct-W2E | primary:p5 |

### Routing rules

XY for remote, YX for sys.
";
        let c = parse_connectivity(body).expect("parses").expect("present");
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.nodes[0].id, "CE0");
        assert_eq!(c.edges.len(), 1);
        assert_eq!(c.edges[0].from, "CE0");
        assert!(c.routing_rules.contains("XY for remote"));
    }

    #[test]
    fn empty_yields_none() {
        let body = "## Connectivity\n";
        assert!(parse_connectivity(body).expect("parses").is_none());
    }
}
