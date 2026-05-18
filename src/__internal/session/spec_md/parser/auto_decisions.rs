//! Parser for the `## Auto-decisions` section (Chapter 2 §2.3.21).
//! Bullet list of `decided <decision>; rationale: <one sentence>`
//! entries. The leading `decided` / `Decided` token is stripped to
//! match the TOML shape `(decision, rationale)`.

use super::section_util::collect_top_level_bullets;
use crate::session::spec_md::types::AutoDecision;

pub(crate) fn parse_auto_decisions(body: &str) -> Vec<AutoDecision> {
    collect_top_level_bullets(body)
        .into_iter()
        .map(parse_decision_bullet)
        .filter(|d| !d.decision.is_empty() || !d.rationale.is_empty())
        .collect()
}

fn parse_decision_bullet(bullet: String) -> AutoDecision {
    let raw = bullet.trim();
    // Strip the leading "decided " / "Decided " prefix when present.
    let body = match raw.to_ascii_lowercase().strip_prefix("decided ") {
        Some(_) => raw["decided ".len()..].trim(),
        None => raw,
    };
    // Split on `; rationale:` (case-insensitive).
    let lc = body.to_ascii_lowercase();
    if let Some(pos) = lc.find("; rationale:") {
        let decision = body[..pos].trim().to_string();
        let rationale = body[pos + "; rationale:".len()..]
            .trim()
            .trim_end_matches('.')
            .to_string();
        AutoDecision {
            decision,
            rationale,
        }
    } else if let Some(pos) = lc.find("rationale:") {
        let decision = body[..pos].trim_end_matches(';').trim().to_string();
        let rationale = body[pos + "rationale:".len()..]
            .trim()
            .trim_end_matches('.')
            .to_string();
        AutoDecision {
            decision,
            rationale,
        }
    } else {
        AutoDecision {
            decision: body.to_string(),
            rationale: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auto_decisions() {
        let body = "\
## Auto-decisions

- Decided XLEN default = 32; rationale: source spec lists 32 and 64; embedded default is 32.
- Decided BPU enabled by default; rationale: source p3 lists BPU as default-on.
";
        let ds = parse_auto_decisions(body);
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].decision, "XLEN default = 32");
        assert!(ds[0].rationale.starts_with("source spec lists"));
        assert_eq!(ds[1].decision, "BPU enabled by default");
    }
}
