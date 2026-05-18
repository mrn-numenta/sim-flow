//! Parser for the `## Functional Behavior` section
//! (Chapter 2 §2.3.12). Three H3 subsections:
//! `End-to-end behavior` (prose), `Operation flow` (numbered list
//! of `id - purpose (anchor: ...)`), and `Data movement` (prose).

use super::SpecMdParseError;
use super::section_util::{collect_prose, collect_top_level_bullets, split_h3};
use crate::session::spec_md::types::{FunctionalBehavior, Operation};

pub(crate) fn parse_functional_behavior(
    body: &str,
) -> Result<FunctionalBehavior, SpecMdParseError> {
    let mut fb = FunctionalBehavior::default();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        match sub.heading.to_ascii_lowercase().as_str() {
            "end-to-end behavior" => fb.end_to_end = collect_prose(&sub.body),
            "operation flow" => {
                fb.operations = collect_top_level_bullets(&sub.body)
                    .into_iter()
                    .map(parse_operation_bullet)
                    .collect();
            }
            "data movement" => fb.data_movement = collect_prose(&sub.body),
            _ => {}
        }
    }
    Ok(fb)
}

/// One bullet of the form `\`Fetch\` - Load instruction parcel from
/// program memory (anchor: primary:p7)`. The opening backtick-quoted
/// token is the id; the parenthesized `anchor: ...` tail is the
/// source anchor; everything else is the purpose.
fn parse_operation_bullet(bullet: String) -> Operation {
    let trimmed = bullet.trim();
    // Pull off the (anchor: ...) tail if present.
    let (head, anchor) = match (trimmed.rfind('('), trimmed.rfind(')')) {
        (Some(o), Some(c)) if c > o => {
            let inner = trimmed[o + 1..c].trim();
            let lc = inner.to_ascii_lowercase();
            if let Some(rest) = lc.strip_prefix("anchor:") {
                let _ = rest;
                let prefix = inner.find(':').map(|p| p + 1).unwrap_or(0);
                let a = inner[prefix..].trim().to_string();
                (trimmed[..o].trim().to_string(), a)
            } else {
                (trimmed.to_string(), String::new())
            }
        }
        _ => (trimmed.to_string(), String::new()),
    };
    // Split id from purpose on the first ` -- `, ` - `, `\u{2014}` or `:`.
    let mut id = String::new();
    let mut purpose = String::new();
    let mut split_done = false;
    for sep in [" -- ", " \u{2014} ", " - ", ": "] {
        if let Some((a, b)) = head.split_once(sep) {
            id = a.trim().trim_matches('`').to_string();
            purpose = b.trim().to_string();
            split_done = true;
            break;
        }
    }
    if !split_done {
        id = head.trim_matches('`').to_string();
    }
    Operation {
        id,
        purpose,
        source_anchor: anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_functional_behavior() {
        let body = "\
## Functional Behavior

### End-to-end behavior

Loads instructions and writes back results.

### Operation flow

1. `Fetch` - Load instruction parcel from program memory (anchor: primary:p7)
2. `Pre-Decode` - Translate 16-bit compressed to 32-bit (anchor: primary:p7-8)
3. `Decode` - Read register file (anchor: primary:p8)

### Data movement

Parcels flow PC-first; registers flow register-first.
";
        let fb = parse_functional_behavior(body).expect("parses");
        assert!(fb.end_to_end.contains("Loads instructions"));
        assert_eq!(fb.operations.len(), 3);
        assert_eq!(fb.operations[0].id, "Fetch");
        assert_eq!(
            fb.operations[0].purpose,
            "Load instruction parcel from program memory"
        );
        assert_eq!(fb.operations[0].source_anchor, "primary:p7");
        assert!(fb.data_movement.contains("PC-first"));
    }
}
