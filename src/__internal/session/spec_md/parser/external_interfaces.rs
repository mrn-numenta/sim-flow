//! Parser for the `## External Interfaces` section (Chapter 2 §2.3.4).
//!
//! Each interface lives under `### Interface: <name>`. The body has
//! a bold-property block (Direction / Protocol / Clock domain /
//! Connected peer), an H4 `#### Signals` table (six-column form),
//! H4 prose subsections for transaction-semantics / timing+flow-
//! control / error behavior, and an H4 `#### Source-spec anchors`
//! bullet list.

use super::SpecMdParseError;
use super::section_util::{
    collect_prose, collect_top_level_bullets, parse_bold_properties, split_h3, split_h4,
};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{ExternalInterface, ExternalSignalRow};

pub(crate) fn parse_external_interfaces(
    body: &str,
) -> Result<Vec<ExternalInterface>, SpecMdParseError> {
    let mut out: Vec<ExternalInterface> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Interface:") else {
            continue;
        };
        let mut iface = ExternalInterface {
            name: name.trim().to_string(),
            ..ExternalInterface::default()
        };
        // Properties live before the first H4.
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "direction" => iface.direction = v,
                "protocol" => iface.protocol = v,
                "clock domain" => iface.clock_domain = v,
                "connected peer" | "connected peer(s)" => iface.peer = v,
                _ => {}
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "signals" => {
                    let tables = MarkdownTable::parse_all(&h4.body)?;
                    if let Some(t) = tables.first() {
                        iface.signals = parse_signal_rows(t)?;
                    }
                }
                "transaction semantics" => {
                    iface.transaction_semantics = collect_prose(&h4.body);
                }
                "timing and flow control" | "timing / flow control" => {
                    iface.timing_and_flow_control = collect_prose(&h4.body);
                }
                "error and exceptional behavior" | "error behavior" => {
                    iface.error_behavior = collect_prose(&h4.body);
                }
                "source-spec anchors" => {
                    iface.source_anchors = parse_anchor_list(&h4.body);
                }
                _ => {}
            }
        }
        out.push(iface);
    }
    Ok(out)
}

fn parse_signal_rows(t: &MarkdownTable) -> Result<Vec<ExternalSignalRow>, SpecMdParseError> {
    let idxs = t.require_columns(&[
        (CanonicalColumn::Signal, "Signal"),
        (CanonicalColumn::Direction, "Direction"),
        (CanonicalColumn::Width, "Width"),
        (CanonicalColumn::Type, "Type"),
        (CanonicalColumn::Required, "Required"),
        (CanonicalColumn::Description, "Description"),
    ])?;
    let mut rows = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        let required = match t.cell(row, idxs[4]).trim().to_ascii_lowercase().as_str() {
            "yes" | "y" | "true" => true,
            "" | "no" | "n" | "false" => false,
            other => {
                return Err(SpecMdParseError::MalformedTable {
                    message: format!("Required column expects yes/no, got `{other}`"),
                    line: 0,
                    column: 0,
                });
            }
        };
        rows.push(ExternalSignalRow {
            name: strip_backticks(t.cell(row, idxs[0])).to_string(),
            direction: t.cell(row, idxs[1]).to_string(),
            width: strip_backticks(t.cell(row, idxs[2])).to_string(),
            ty: strip_backticks(t.cell(row, idxs[3])).to_string(),
            required,
            description: t.cell(row, idxs[5]).to_string(),
        });
    }
    Ok(rows)
}

/// Bullets of the form `primary:p2 (Product Brief block diagram)` --
/// keep just the leading anchor token.
pub(crate) fn parse_anchor_list(body: &str) -> Vec<String> {
    collect_top_level_bullets(body)
        .into_iter()
        .map(|line| {
            let head = line.split_once(' ').map(|(a, _)| a).unwrap_or(&line);
            head.trim_end_matches([',', ';']).to_string()
        })
        .collect()
}

fn strip_backticks(s: &str) -> &str {
    s.trim().trim_matches('`')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_interfaces() {
        let body = "\
## External Interfaces

### Interface: Instruction Interface

**Direction:** bidirectional
**Protocol:** AHB
**Clock domain:** core
**Connected peer:** instruction memory bus

#### Signals

| Signal | Direction | Width | Type | Required | Description |
| --- | --- | --- | --- | --- | --- |
| `inst_addr` | out | XLEN | logic | yes | Instruction address |
| `inst_data` | in | XLEN | logic | yes | Fetched instruction |

#### Transaction semantics

Master initiates a fetch; slave returns a parcel.

#### Source-spec anchors

- primary:p2 (Product Brief block diagram)
- primary:p11 (RV12 Execution Pipeline overview)

### Interface: Data Interface

**Direction:** bidirectional
**Protocol:** AHB

#### Signals

| Signal | Direction | Width | Type | Required | Description |
| --- | --- | --- | --- | --- | --- |
| `data_addr` | out | XLEN | logic | yes | Load/store address |
";
        let ifaces = parse_external_interfaces(body).expect("parses");
        assert_eq!(ifaces.len(), 2);
        let a = &ifaces[0];
        assert_eq!(a.name, "Instruction Interface");
        assert_eq!(a.direction, "bidirectional");
        assert_eq!(a.protocol, "AHB");
        assert_eq!(a.signals.len(), 2);
        assert_eq!(a.signals[0].name, "inst_addr");
        assert!(a.signals[0].required);
        assert_eq!(a.signals[0].width, "XLEN");
        assert!(a.transaction_semantics.contains("Master initiates"));
        assert_eq!(a.source_anchors, vec!["primary:p2", "primary:p11"]);
        let b = &ifaces[1];
        assert_eq!(b.name, "Data Interface");
        assert_eq!(b.signals.len(), 1);
    }

    #[test]
    fn no_interfaces_yields_empty() {
        let body = "## External Interfaces\n";
        assert!(parse_external_interfaces(body).expect("parses").is_empty());
    }
}
