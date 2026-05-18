//! Parser for the `## Timing, Latency, and Throughput` section
//! (Chapter 2 §2.3.13). H3 subsections: `Latency` (table) plus
//! `Throughput` and `Stall and backpressure` (prose).

use super::SpecMdParseError;
use super::section_util::{collect_prose, split_h3};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{LatencyRow, TimingAndThroughput};

pub(crate) fn parse_timing(body: &str) -> Result<TimingAndThroughput, SpecMdParseError> {
    let mut t = TimingAndThroughput::default();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        match sub.heading.to_ascii_lowercase().as_str() {
            "latency" => {
                let tables = MarkdownTable::parse_all(&sub.body)?;
                if let Some(tbl) = tables.first() {
                    let idxs = tbl.require_columns(&[
                        (CanonicalColumn::Operation, "Operation"),
                        (CanonicalColumn::BestCase, "Best-case"),
                        (CanonicalColumn::WorstCase, "Worst-case"),
                    ])?;
                    let notes_idx = tbl.optional_column(CanonicalColumn::Description);
                    for row in &tbl.rows {
                        let notes = notes_idx
                            .map(|i| tbl.cell(row, i).to_string())
                            .unwrap_or_default();
                        t.latency.push(LatencyRow {
                            operation: tbl.cell(row, idxs[0]).to_string(),
                            best_case: tbl.cell(row, idxs[1]).to_string(),
                            worst_case: tbl.cell(row, idxs[2]).to_string(),
                            notes,
                        });
                    }
                }
            }
            "throughput" => t.throughput = collect_prose(&sub.body),
            "stall and backpressure" => t.stall_and_backpressure = collect_prose(&sub.body),
            _ => {}
        }
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timing() {
        let body = "\
## Timing, Latency, and Throughput

### Latency

| Operation | Best-case | Worst-case | Notes |
| --- | --- | --- | --- |
| Instruction fetch | 2 cycles | N cycles | cache miss stalls |

### Throughput

One instruction per cycle steady-state.

### Stall and backpressure

PD stalls IF on hazard.
";
        let t = parse_timing(body).expect("parses");
        assert_eq!(t.latency.len(), 1);
        assert_eq!(t.latency[0].operation, "Instruction fetch");
        assert_eq!(t.latency[0].best_case, "2 cycles");
        assert!(t.throughput.contains("steady-state"));
        assert!(t.stall_and_backpressure.contains("hazard"));
    }
}
