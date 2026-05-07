//! Critique-file parsing.
//!
//! The critique file is free-form markdown. The gate contract is that
//! any line whose first non-whitespace token is `BLOCKER:` fails the
//! gate. `UNRESOLVED:` lines are informational notes the model wants
//! to flag but does not consider blocking; `RESOLVED:` lines are
//! purely historical. Only `BLOCKER:` prevents advancement.

use std::path::Path;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    Resolved(String),
    Unresolved(String),
    Blocker(String),
}

impl Finding {
    pub fn text(&self) -> &str {
        match self {
            Finding::Resolved(t) | Finding::Unresolved(t) | Finding::Blocker(t) => t,
        }
    }

    pub fn is_blocking(&self) -> bool {
        matches!(self, Finding::Blocker(_))
    }
}

#[derive(Debug, Default, Clone)]
pub struct Critique {
    pub findings: Vec<Finding>,
}

/// Lenient finding-marker regex. Matches a line whose first
/// non-decoration token is `BLOCKER:` / `RESOLVED:` /
/// `UNRESOLVED:` (case-insensitive, optional plural). Allowed
/// prefixes (any combination, in any order, before the keyword):
///
/// - whitespace
/// - list markers: `-`, `*`, `+`, `>` (blockquote)
/// - heading markers: `#`+
/// - bold / underline wrapping: `**` or `__`
/// - one decoration glyph (emoji, dingbat) -- e.g. `❌` `✅`
///
/// All these forms are recognized as findings:
///
/// ```text
/// BLOCKER: foo
/// - BLOCKER: foo
/// **BLOCKER:** foo
/// ## BLOCKER: foo
/// ### ❌ BLOCKER: foo
/// > BLOCKER: foo
/// BLOCKERS: two open
/// blocker: lower-case
/// ```
///
/// What does NOT match (intentionally):
///
/// - `### BLOCKER 1 - title` -- heading describing a blocker;
///   no colon-immediately-after-keyword; agents use this as a
///   prose section title, not a finding.
/// - mid-sentence mentions: "the BLOCKER: convention is...".
/// - Status-field labels: `Status: BLOCKER` (BLOCKER not at the
///   start of the keyword position).
///
/// This regex MUST stay in sync with
/// `orchestrator.rs::FINDING_MARKER_RE`. Both parsers exist
/// because they answer different questions (gate-side
/// `has_blocking()` vs. orchestrator-side `extract_blocker_blocks`
/// for focused-retry inlining), but they MUST agree on what
/// counts as a finding -- otherwise the gate advances past
/// blockers the orchestrator's retry path correctly identifies,
/// or vice versa. Today's regression: the gate-side parser only
/// stripped `- ` / `* ` and missed `## BLOCKER:`, letting DM3b
/// advance with 5 real heading-style blockers ticked off as
/// "complete".
static FINDING_MARKER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        // Order in the prefix matters:
        // 1. `[\s\-\*\+#>]*` -- whitespace + simple list/heading/blockquote markers
        // 2. `(?:\d+\.\s+)?` -- numbered list marker (`1. `, `12. `)
        // 3. `(?:\*\*|__)?` -- bold / underline open
        // 4. `[^\w\s]*` -- one decoration glyph (emoji, dingbat)
        // The trailing `\s*` and `(?i)blockers?:` then claim the keyword.
        r"^[\s\-\*\+#>]*(?:\d+\.\s+)?(?:\*\*|__)?\s*[^\w\s]*\s*(?P<kind>(?i)blockers?|unresolveds?|resolveds?):(?P<rest>.*)$"
    )
    .expect("finding-marker regex compiles")
});

impl Critique {
    pub fn parse(text: &str) -> Self {
        let mut findings = Vec::new();
        for raw in text.lines() {
            let Some(caps) = FINDING_MARKER_RE.captures(raw) else {
                continue;
            };
            let kind = caps
                .name("kind")
                .map(|m| m.as_str().to_ascii_lowercase())
                .unwrap_or_default();
            let rest = caps
                .name("rest")
                .map(|m| m.as_str().trim().trim_end_matches("**").trim().to_string())
                .unwrap_or_default();
            if kind.starts_with("blocker") {
                findings.push(Finding::Blocker(rest));
            } else if kind.starts_with("unresolved") {
                findings.push(Finding::Unresolved(rest));
            } else if kind.starts_with("resolved") {
                findings.push(Finding::Resolved(rest));
            }
        }
        Self { findings }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self::parse(&text))
    }

    pub fn blocking(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.is_blocking()).collect()
    }

    pub fn has_blocking(&self) -> bool {
        self.findings.iter().any(|f| f.is_blocking())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prefixes() {
        let text = "\
# Critique

## Findings
- RESOLVED: FetchModule needed settle()
- UNRESOLVED: bubble rate higher than expected
- BLOCKER: Scoreboard does not verify ordering
";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 3);
        // Only BLOCKER counts as blocking; UNRESOLVED and RESOLVED are
        // informational.
        assert!(!c.findings[0].is_blocking());
        assert!(!c.findings[1].is_blocking());
        assert!(c.findings[2].is_blocking());
        assert_eq!(c.blocking().len(), 1);
        assert!(c.has_blocking());
    }

    #[test]
    fn unresolved_only_critique_does_not_block() {
        let text = "- UNRESOLVED: minor wording nit\n- UNRESOLVED: future cleanup\n";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 2);
        assert!(!c.has_blocking());
        assert!(c.blocking().is_empty());
    }

    #[test]
    fn ignores_untagged_text() {
        let text = "Body text without markers.";
        let c = Critique::parse(text);
        assert!(c.findings.is_empty());
        assert!(!c.has_blocking());
    }

    #[test]
    fn handles_list_prefixes_and_leading_whitespace() {
        let text = "   - BLOCKER: indented with list marker";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 1);
        assert!(matches!(&c.findings[0], Finding::Blocker(_)));
    }

    #[test]
    fn matches_heading_style_blockers() {
        // The bug this regression-tests: DM3b's critique emitted
        // `## BLOCKER: Milestone 02 -- Scoreboard tasks ticked off
        // but no artifact landed`. The old strict matcher (only
        // `- ` / `* ` prefixes) returned zero findings; the gate
        // advanced past 5 real BLOCKERs. The lenient regex now
        // strips `#` chars + emoji + bold so heading-style and
        // decorated finding lines all count.
        let text = "\
## BLOCKER: Milestone 02 -- artifact missing
### ❌ BLOCKER: scope discipline violation
**BLOCKER:** ambiguous reset semantics
- BLOCKER: missing gate budget
> BLOCKER: blockquote-styled finding
BLOCKERS: plural form
blocker: case-insensitive
";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 7, "got {:?}", c.findings);
        for f in &c.findings {
            assert!(f.is_blocking(), "expected all blockers, got {f:?}");
        }
        assert!(c.has_blocking());
    }

    #[test]
    fn ignores_section_titles_about_blockers() {
        // `### BLOCKER 1 - title` is a heading describing a
        // blocker, NOT a finding line. There's no
        // colon-immediately-after-keyword. Agents use this format
        // as a section title to introduce the blocker discussion;
        // the actual finding line lives elsewhere (typically as a
        // bullet or `## BLOCKER:` heading later).
        let text = "\
### BLOCKER 1 - stress.md target coverage
RESOLVED: stress.md exercises every target.
### BLOCKER 2 - coverage.md incomplete
BLOCKER: numeric threshold missing.
";
        let c = Critique::parse(text);
        // Two findings: the RESOLVED bullet + the BLOCKER bullet.
        // Neither `### BLOCKER N - title` line counts.
        assert_eq!(c.findings.len(), 2, "got {:?}", c.findings);
        assert!(matches!(&c.findings[0], Finding::Resolved(_)));
        assert!(matches!(&c.findings[1], Finding::Blocker(_)));
    }

    #[test]
    fn ignores_inline_blocker_mentions() {
        let text = "We discussed the BLOCKER: marker convention.\nThat's it.";
        let c = Critique::parse(text);
        assert!(
            c.findings.is_empty(),
            "mid-sentence mentions should not match: {:?}",
            c.findings
        );
        assert!(!c.has_blocking());
    }
}
