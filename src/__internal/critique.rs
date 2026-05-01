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

impl Critique {
    pub fn parse(text: &str) -> Self {
        let mut findings = Vec::new();
        for raw in text.lines() {
            let trimmed = raw.trim_start();
            // Also strip common markdown list markers so `- UNRESOLVED: ...`
            // parses the same as `UNRESOLVED: ...`.
            let stripped = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .unwrap_or(trimmed);
            if let Some(rest) = stripped.strip_prefix("UNRESOLVED:") {
                findings.push(Finding::Unresolved(rest.trim().to_string()));
            } else if let Some(rest) = stripped.strip_prefix("BLOCKER:") {
                findings.push(Finding::Blocker(rest.trim().to_string()));
            } else if let Some(rest) = stripped.strip_prefix("RESOLVED:") {
                findings.push(Finding::Resolved(rest.trim().to_string()));
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
}
