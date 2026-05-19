//! Parser for the `## Blocks` section (Chapter 2 §2.3.5).
//!
//! Every block is a flat `### Block: <name>` entry (parent links are
//! stored in the body, not by markdown nesting depth). Each entry has
//! a bold-property block (Role / Parent / Clock domain /
//! Parameterized by) plus optional H4 subsections: `#### I/O Signals`
//! (four-column table), `#### State` (bullet list), `#### Behavior
//! summary` (prose), `#### Source-spec anchors` (anchor list),
//! `#### Figures` (figure list), `#### Sub-blocks` (links).

use super::SpecMdParseError;
use super::external_interfaces::parse_anchor_list;
use super::section_util::{
    collect_prose, collect_top_level_bullets, parse_bold_properties, split_h3, split_h4,
};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{Block, BlockSignalRow, BlockState, Layer, SignalRole};

pub(crate) fn parse_blocks(body: &str) -> Result<Vec<Block>, SpecMdParseError> {
    let mut out: Vec<Block> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Block:") else {
            continue;
        };
        let mut block = Block {
            name: name.trim().to_string(),
            ..Block::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "role" => block.role = v,
                "parent" => block.parent = v,
                "clock domain" => block.clock_domain = v,
                "power domain" => block.power_domain = v,
                "reset domain" => block.reset_domain = v,
                "layer" => block.layer = parse_layer(&v),
                "parameterized by" => block.parameterized_by = parse_param_list(&v),
                _ => {}
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "i/o signals" | "io signals" | "signals" => {
                    let tables = MarkdownTable::parse_all(&h4.body)?;
                    if let Some(t) = tables.first() {
                        block.signals = parse_block_signal_rows(t)?;
                    }
                }
                "state" => {
                    block.state = collect_top_level_bullets(&h4.body)
                        .into_iter()
                        .map(parse_state_bullet)
                        .collect();
                }
                "behavior summary" => {
                    block.behavior_summary = collect_prose(&h4.body);
                }
                "source-spec anchors" => {
                    block.source_anchors = parse_anchor_list(&h4.body);
                }
                "figures" => {
                    block.figures = parse_figure_links(&h4.body);
                }
                "sub-blocks" | "subblocks" => {
                    block.sub_blocks = parse_sub_blocks(&h4.body);
                }
                _ => {}
            }
        }
        out.push(block);
    }
    Ok(out)
}

fn parse_param_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().trim_matches('`').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_block_signal_rows(t: &MarkdownTable) -> Result<Vec<BlockSignalRow>, SpecMdParseError> {
    let idxs = t.require_columns(&[
        (CanonicalColumn::Signal, "Signal"),
        (CanonicalColumn::Direction, "Direction"),
        (CanonicalColumn::Peer, "Peer"),
        (CanonicalColumn::Description, "Description"),
    ])?;
    // Optional Role column (Phase 9 §7.7). Lookup by header text so
    // we don't collide with CanonicalColumn::Role (used in
    // connectivity nodes) or shift CanonicalColumn semantics.
    let role_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("role"));
    let mut rows = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        rows.push(BlockSignalRow {
            name: t.cell(row, idxs[0]).trim_matches('`').to_string(),
            direction: t.cell(row, idxs[1]).to_string(),
            peer: t.cell(row, idxs[2]).to_string(),
            description: t.cell(row, idxs[3]).to_string(),
            role: role_idx
                .map(|i| parse_signal_role(t.cell(row, i)))
                .unwrap_or_default(),
        });
    }
    Ok(rows)
}

fn parse_layer(value: &str) -> Layer {
    match value.trim().to_ascii_lowercase().as_str() {
        "architectural" => Layer::Architectural,
        "micro" => Layer::Micro,
        "mixed" => Layer::Mixed,
        _ => Layer::Unknown,
    }
}

fn parse_signal_role(value: &str) -> SignalRole {
    match value.trim().to_ascii_lowercase().as_str() {
        "control" => SignalRole::Control,
        "data" => SignalRole::Data,
        "status" => SignalRole::Status,
        _ => SignalRole::Unknown,
    }
}

/// Parse a single State bullet of the form:
/// `pc (XLEN-wide register, reset to RESET_VECTOR)` -- pick out name,
/// width (the leading token of the parens, with `-wide` / `register`
/// suffix stripped), and reset value (text after `reset to`).
fn parse_state_bullet(bullet: String) -> BlockState {
    let trimmed = bullet.trim().to_string();
    let (name_part, rest_part) = match trimmed.find('(') {
        Some(open) => {
            let name = trimmed[..open].trim().trim_matches('`').to_string();
            let close = trimmed.rfind(')').unwrap_or(trimmed.len());
            let inner = trimmed[open + 1..close.min(trimmed.len())].to_string();
            (name, inner)
        }
        None => (trimmed.trim_matches('`').to_string(), String::new()),
    };
    let mut width = String::new();
    let mut reset_value = String::new();
    let mut description = String::new();
    if !rest_part.is_empty() {
        // Reset value: text after "reset to ".
        let lc = rest_part.to_ascii_lowercase();
        if let Some(pos) = lc.find("reset to ") {
            let after = &rest_part[pos + "reset to ".len()..];
            let rv = after.split_once(',').map(|(a, _)| a).unwrap_or(after);
            reset_value = rv.trim().to_string();
        }
        // Width: first comma-separated chunk, strip "-wide register"
        // / "wide register" / "register" suffix.
        let first = rest_part.split(',').next().unwrap_or("").trim();
        let mut w = first.to_string();
        for tail in ["-wide register", " wide register", " register"] {
            if let Some(stripped) = w.strip_suffix(tail) {
                w = stripped.trim().to_string();
            }
        }
        width = w;
        // Remainder becomes description.
        description = rest_part.trim().to_string();
    }
    BlockState {
        name: name_part,
        width,
        reset_value,
        description,
    }
}

fn parse_figure_links(body: &str) -> Vec<String> {
    collect_top_level_bullets(body)
        .into_iter()
        .map(|line| {
            // Accept "IF block diagram -> figures/foo.png" /
            // "IF block diagram → figures/foo.png" / bare path.
            let arrow_split = line
                .split_once('\u{2192}')
                .or_else(|| line.split_once("->"));
            match arrow_split {
                Some((_, rhs)) => rhs.trim().to_string(),
                None => line,
            }
        })
        .collect()
}

fn parse_sub_blocks(body: &str) -> Vec<String> {
    collect_top_level_bullets(body)
        .into_iter()
        .map(|line| {
            // Bullets may be markdown links `[Name](#anchor)` -- the
            // text-collector already stripped the link target so we
            // just trim brackets if present.
            line.trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_blocks_two_level_hierarchy() {
        let body = "\
## Blocks

### Block: Execution Pipeline

**Role:** Top-level pipeline orchestrating IF/PD/ID/EX/MEM/WB stages.
**Parent:** (none -- top-level)
**Clock domain:** core
**Parameterized by:** `XLEN`, `HAS_BPU`

#### Source-spec anchors

- primary:p2 (Product Brief)
- primary:p6 (Execution Pipeline overview)

#### Sub-blocks

- Instruction Fetch (IF)
- Pre-Decode (PD)

### Block: Instruction Fetch (IF)

**Role:** Loads instruction parcels from program memory.
**Parent:** Execution Pipeline
**Clock domain:** core

#### I/O Signals

| Signal | Direction | Peer | Description |
| --- | --- | --- | --- |
| `if_nxt_pc` | out | Bus Interface | Next address |
| `parcel` | in | Bus Interface | Fetched parcel |

#### State

- `pc` (XLEN-wide register, reset to RESET_VECTOR)

#### Behavior summary

Fetches parcels at PC, advances on success.

#### Source-spec anchors

- primary:p12-13 (IF section)

#### Figures

- IF block diagram -> figures/page-013.png

### Block: Pre-Decode (PD)

**Role:** Translate compressed parcels.
**Parent:** Execution Pipeline
**Clock domain:** core
";
        let blocks = parse_blocks(body).expect("parses");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].name, "Execution Pipeline");
        assert!(blocks[0].parent.contains("none"));
        assert_eq!(blocks[0].parameterized_by, vec!["XLEN", "HAS_BPU"]);
        assert_eq!(blocks[0].sub_blocks.len(), 2);
        let f = &blocks[1];
        assert_eq!(f.name, "Instruction Fetch (IF)");
        assert_eq!(f.parent, "Execution Pipeline");
        assert_eq!(f.signals.len(), 2);
        assert_eq!(f.signals[0].name, "if_nxt_pc");
        assert_eq!(f.signals[0].peer, "Bus Interface");
        assert_eq!(f.state.len(), 1);
        assert_eq!(f.state[0].name, "pc");
        assert_eq!(f.state[0].width, "XLEN");
        assert_eq!(f.state[0].reset_value, "RESET_VECTOR");
        assert!(f.behavior_summary.contains("Fetches parcels"));
        assert_eq!(f.source_anchors, vec!["primary:p12-13"]);
        assert_eq!(f.figures, vec!["figures/page-013.png"]);
    }
}
