//! Parser for the `## State Machines` section (Chapter 2 §2.3.7).
//! Each FSM is `### FSM: <name>` with a bold-property block plus
//! `#### States` (bullet list `state - description`) and
//! `#### Transitions` (table From / Input / To / Output).

use super::SpecMdParseError;
use super::section_util::{collect_top_level_bullets, parse_bold_properties, split_h3, split_h4};
use super::table::{CanonicalColumn, MarkdownTable};
use crate::session::spec_md::types::{FsmState, FsmTransition, StateMachine};

pub(crate) fn parse_state_machines(body: &str) -> Result<Vec<StateMachine>, SpecMdParseError> {
    let mut out: Vec<StateMachine> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("FSM:") else {
            continue;
        };
        let mut fsm = StateMachine {
            name: name.trim().to_string(),
            ..StateMachine::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "reset state" => fsm.reset_state = v.trim_matches('`').to_string(),
                "source-spec anchor" | "source anchor" => fsm.source_anchor = v,
                _ => {}
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "states" => {
                    fsm.states = collect_top_level_bullets(&h4.body)
                        .into_iter()
                        .map(parse_state_bullet)
                        .collect();
                }
                "transitions" => {
                    let tables = MarkdownTable::parse_all(&h4.body)?;
                    if let Some(t) = tables.first() {
                        fsm.transitions = parse_transitions(t)?;
                    }
                }
                _ => {}
            }
        }
        out.push(fsm);
    }
    Ok(out)
}

fn parse_state_bullet(bullet: String) -> FsmState {
    // Form: `IDLE - pre-power-on; waiting for power valid + Refclk`
    // or `IDLE \u{2014} description`. Try common dash separators.
    let trimmed = bullet.trim();
    for sep in [" -- ", " \u{2014} ", " - ", " : "] {
        if let Some((a, b)) = trimmed.split_once(sep) {
            return FsmState {
                name: a.trim().trim_matches('`').to_string(),
                description: b.trim().to_string(),
            };
        }
    }
    FsmState {
        name: trimmed.trim_matches('`').to_string(),
        description: String::new(),
    }
}

fn parse_transitions(t: &MarkdownTable) -> Result<Vec<FsmTransition>, SpecMdParseError> {
    let idxs = t.require_columns(&[
        (CanonicalColumn::From, "From"),
        (CanonicalColumn::Input, "Input/Event"),
        (CanonicalColumn::To, "To"),
        (CanonicalColumn::Output, "Output/Action"),
    ])?;
    let mut rows = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        rows.push(FsmTransition {
            from: t.cell(row, idxs[0]).trim_matches('`').to_string(),
            input: t.cell(row, idxs[1]).to_string(),
            to: t.cell(row, idxs[2]).trim_matches('`').to_string(),
            output: t.cell(row, idxs[3]).to_string(),
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_fsm() {
        let body = "\
## State Machines

### FSM: Boot FSM

**Reset state:** IDLE
**Source-spec anchor:** primary:p8-9

#### States

- `IDLE` - pre-power-on; waiting for power valid
- `RESET_HOLD` - nReset asserted, awaiting stability
- `BP_RUN` - Boot Processor running BootROM code

#### Transitions

| From | Input/Event | To | Output/Action |
| --- | --- | --- | --- |
| `IDLE` | power_on | `RESET_HOLD` | assert nReset |
| `RESET_HOLD` | stability_timer_done | `BP_RUN` | deassert reset |
";
        let fsms = parse_state_machines(body).expect("parses");
        assert_eq!(fsms.len(), 1);
        let f = &fsms[0];
        assert_eq!(f.name, "Boot FSM");
        assert_eq!(f.reset_state, "IDLE");
        assert_eq!(f.source_anchor, "primary:p8-9");
        assert_eq!(f.states.len(), 3);
        assert_eq!(f.states[0].name, "IDLE");
        assert!(f.states[0].description.contains("pre-power-on"));
        assert_eq!(f.transitions.len(), 2);
        assert_eq!(f.transitions[0].from, "IDLE");
        assert_eq!(f.transitions[0].input, "power_on");
        assert_eq!(f.transitions[0].to, "RESET_HOLD");
    }
}
