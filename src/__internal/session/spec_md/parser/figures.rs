//! Parser for the `## Figures` section (Chapter 2 §2.3.17).
//! Each figure is `### Figure: <name>` with a bold-property block
//! (Source page / Raster / Role / Referenced blocks), an H4
//! `#### Caption` prose subsection, and an H4 `#### Elements
//! depicted` table.

use super::SpecMdParseError;
use super::section_util::{collect_prose, parse_bold_properties, split_h3, split_h4};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{FigureElement, FigureEntry};

pub(crate) fn parse_figures(body: &str) -> Result<Vec<FigureEntry>, SpecMdParseError> {
    let mut out: Vec<FigureEntry> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Figure:") else {
            continue;
        };
        let mut fig = FigureEntry {
            name: name.trim().to_string(),
            ..FigureEntry::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "source page" => fig.source_page = v,
                "raster" => fig.raster = strip_link_target(&v),
                "role" => fig.role = v,
                "referenced blocks" => {
                    fig.referenced_blocks = v
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "caption" => fig.caption = collect_prose(&h4.body),
                "elements depicted" => {
                    let tables = MarkdownTable::parse_all(&h4.body)?;
                    if let Some(t) = tables.first() {
                        let idxs = t.require_columns(&[
                            (CanonicalColumn::Element, "Element"),
                            (CanonicalColumn::Kind, "Kind"),
                            (CanonicalColumn::Description, "Notes"),
                        ])?;
                        for row in &t.rows {
                            fig.elements.push(FigureElement {
                                name: t.cell(row, idxs[0]).trim_matches('`').to_string(),
                                kind: t.cell(row, idxs[1]).to_string(),
                                notes: t.cell(row, idxs[2]).to_string(),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(fig);
    }
    Ok(out)
}

/// `[figures/page-013.png](figures/page-013.png)` -> `figures/page-013.png`.
/// Falls back to the raw value when the link form doesn't match.
fn strip_link_target(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(close_bracket) = rest.find(']')
    {
        let label = rest[..close_bracket].trim();
        return label.to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_figure() {
        let body = "\
## Figures

### Figure: IF block diagram

**Source page:** 13
**Raster:** [figures/page-013.png](figures/page-013.png)
**Role:** Instruction Fetch internal diagram
**Referenced blocks:** Instruction Fetch (IF), Bus Interface

#### Caption

The IF stage selects the next PC.

#### Elements depicted

| Element | Kind | Notes |
| --- | --- | --- |
| `if_nxt_pc` | signal | output of mux |
| Mux (4:1) | block | next-PC selector |
";
        let figs = parse_figures(body).expect("parses");
        assert_eq!(figs.len(), 1);
        let f = &figs[0];
        assert_eq!(f.name, "IF block diagram");
        assert_eq!(f.source_page, "13");
        assert_eq!(f.raster, "figures/page-013.png");
        assert_eq!(f.role, "Instruction Fetch internal diagram");
        assert_eq!(
            f.referenced_blocks,
            vec!["Instruction Fetch (IF)", "Bus Interface"]
        );
        assert!(f.caption.contains("next PC"));
        assert_eq!(f.elements.len(), 2);
        assert_eq!(f.elements[0].name, "if_nxt_pc");
        assert_eq!(f.elements[0].kind, "signal");
    }
}
