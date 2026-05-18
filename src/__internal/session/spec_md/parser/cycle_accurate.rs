//! Parser for the `## Cycle-Accurate Behavior` section
//! (Chapter 2 §2.3.16). Each scenario is `### Scenario: <name>`
//! containing one cycle-by-cycle table with dynamic stage columns
//! plus an optional `**Source-anchor:**` line.

use super::SpecMdParseError;
use super::section_util::{parse_bold_properties, split_h3};
use super::table::MarkdownTable;
use crate::session::spec_md::types::{CycleAccurateRow, CycleAccurateScenario};

pub(crate) fn parse_cycle_accurate(
    body: &str,
) -> Result<Vec<CycleAccurateScenario>, SpecMdParseError> {
    let mut out: Vec<CycleAccurateScenario> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Scenario:") else {
            continue;
        };
        let mut scenario = CycleAccurateScenario {
            name: name.trim().to_string(),
            ..CycleAccurateScenario::default()
        };
        let tables = MarkdownTable::parse_all(&sub.body)?;
        if let Some(t) = tables.first() {
            scenario.columns = t.headers.clone();
            scenario.rows = t
                .rows
                .iter()
                .map(|r| CycleAccurateRow { cells: r.clone() })
                .collect();
        }
        for (k, v) in parse_bold_properties(&sub.body) {
            if k.eq_ignore_ascii_case("source-anchor") || k.eq_ignore_ascii_case("source anchor") {
                scenario.source_anchor = v;
            }
        }
        out.push(scenario);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_scenario() {
        let body = "\
## Cycle-Accurate Behavior

### Scenario: 6 instructions in flight

| Cycle | IF | PD | ID |
| --- | --- | --- | --- |
| 1 | I_A | -- | -- |
| 2 | I_B | I_A | -- |
| 3 | I_C | I_B | I_A |

**Source-anchor:** primary:p7
";
        let scenes = parse_cycle_accurate(body).expect("parses");
        assert_eq!(scenes.len(), 1);
        let s = &scenes[0];
        assert_eq!(s.name, "6 instructions in flight");
        assert_eq!(s.columns, vec!["Cycle", "IF", "PD", "ID"]);
        assert_eq!(s.rows.len(), 3);
        assert_eq!(s.rows[2].cells, vec!["3", "I_C", "I_B", "I_A"]);
        assert_eq!(s.source_anchor, "primary:p7");
    }
}
