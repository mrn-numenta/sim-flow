//! Critique-file parsing.
//!
//! Critiques have a canonical JSON form (`<step>-critique.json`) that
//! the agent emits and the orchestrator parses. The orchestrator
//! renders a human-readable markdown view (`<step>-critique.md`) from
//! the JSON each pass; the markdown is a derived artifact, not a
//! source. The agent never writes the markdown directly.
//!
//! The legacy markdown-with-`BLOCKER:`-line-markers form is still
//! parsable so projects mid-flight (where critiques landed before
//! this migration) keep working. `Critique::load` resolves a
//! `<step>-critique.md` path by checking for the JSON sibling first,
//! falling back to the markdown body's regex parse only when no JSON
//! exists.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

mod dashboard;
mod render;

#[cfg(test)]
mod tests;

pub use dashboard::{
    CritiqueDashboardEntry, DashboardFinding, DashboardFindingKind, list_critique_entries,
    read_critique_entry,
};
pub use render::{
    is_critique_json_path, json_sibling, render_critique_markdown_to_disk, render_markdown,
};

/// Canonical JSON shape for a critique. The agent emits this; the
/// orchestrator parses it. `serde(deny_unknown_fields)` is
/// intentional -- a typo in a field name should fail loud at parse
/// time, not silently drop content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CritiqueJson {
    /// Step id this critique covers (e.g. `"DM3a"`). Lets the
    /// orchestrator sanity-check that the agent didn't write the
    /// wrong step's critique into the wrong file.
    pub step: String,
    /// One-paragraph summary of the critique outcome. Rendered at
    /// the top of the markdown view; the gate ignores it.
    #[serde(default)]
    pub summary: String,
    /// The findings, in the order the agent produced them. Rendered
    /// section-by-section in the markdown view; gate-relevant for
    /// `BLOCKER` and `UNRESOLVED` entries.
    pub findings: Vec<CritiqueFinding>,
    /// Optional free-form trailing prose for things that don't fit a
    /// finding (questions for the user, design observations, etc.).
    /// Rendered as-is at the bottom of the markdown view.
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    Blocker,
    Unresolved,
    Resolved,
}

impl FindingKind {
    pub(super) fn as_label(self) -> &'static str {
        match self {
            FindingKind::Blocker => "BLOCKER",
            FindingKind::Unresolved => "UNRESOLVED",
            FindingKind::Resolved => "RESOLVED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CritiqueFinding {
    pub kind: FindingKind,
    /// Free-form section name (e.g. `"Milestone Completeness"`).
    /// Used only by the markdown renderer to group findings under
    /// `## <Section>` headings; the gate ignores it.
    #[serde(default)]
    pub section: String,
    /// Short one-line summary of the finding. Rendered as the
    /// `BLOCKER:` line's body in markdown so existing eyeball
    /// conventions still apply.
    pub title: String,
    /// Optional multi-line markdown body explaining the finding,
    /// quoting offending lines, listing remediation, etc. Rendered
    /// after `title` in the markdown view.
    #[serde(default)]
    pub body: String,
}

/// Parsed view of a critique. Conceptually a list of `Finding`
/// values plus optional summary / notes prose. Constructed from
/// either a JSON document (`from_json`) or a legacy markdown body
/// (`parse`); call sites generally use `Critique::load` and don't
/// care which form was on disk.
#[derive(Debug, Default, Clone)]
pub struct Critique {
    pub findings: Vec<Finding>,
    pub summary: String,
    pub notes: String,
}

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

    /// True iff this finding should BLOCK step advancement and
    /// count toward the auto-loop's no-progress / retry caps.
    /// Both `Blocker` and `Unresolved` qualify -- the critic's
    /// distinction is severity / origin (Blocker = new issue,
    /// Unresolved = previously-flagged issue still present), not
    /// gate semantics. The gate (`gate.rs::CritiqueClean`) and the
    /// auto loop's no-progress detector must agree on this set or
    /// the agent advances past unaddressed findings.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Finding::Unresolved(_) | Finding::Blocker(_))
    }
}

/// Lenient finding-marker regex used by `Critique::parse` for the
/// legacy markdown form. New critiques are JSON; this regex stays
/// in place only so projects that already have `.md` critiques on
/// disk (no JSON sibling) keep parsing.
///
/// Matches a line whose first non-decoration token is `BLOCKER:` /
/// `RESOLVED:` / `UNRESOLVED:` (case-insensitive, optional plural).
/// Allowed prefixes (in any order, before the keyword): whitespace,
/// list markers (`-`, `*`, `+`, `>`), heading markers (`#`+),
/// bold / underline (`**` / `__`), one decoration glyph (emoji,
/// dingbat).
///
/// What does NOT match (intentionally):
///
/// - `### BLOCKER 1 - title` -- heading describing a blocker; no
///   colon-immediately-after-keyword.
/// - mid-sentence mentions: "the BLOCKER: convention is...".
/// - Status-field labels: `Status: BLOCKER`.
///
/// MUST stay in sync with
/// `orchestrator.rs::FINDING_MARKER_RE`.
pub(super) static FINDING_MARKER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(
    || {
        regex::Regex::new(
        r"^[\s\-\*\+#>]*(?:\d+\.\s+)?(?:\*\*|__)?\s*[^\w\s]*\s*(?P<kind>(?i)blockers?|unresolveds?|resolveds?):(?P<rest>.*)$"
    )
    .expect("finding-marker regex compiles")
    },
);

impl Critique {
    /// Parse a JSON critique body into a `Critique`. Returns
    /// `Err(Error::State)` on malformed JSON so the gate can surface
    /// the parse error instead of silently treating a broken file as
    /// "no findings".
    pub fn from_json(text: &str) -> Result<Self> {
        let parsed: CritiqueJson = serde_json::from_str(text)
            .map_err(|err| Error::State(format!("malformed critique JSON: {err}")))?;
        let findings = parsed
            .findings
            .iter()
            .map(|f| {
                let label = if f.body.trim().is_empty() {
                    f.title.clone()
                } else {
                    format!("{}\n{}", f.title.trim(), f.body.trim())
                };
                match f.kind {
                    FindingKind::Blocker => Finding::Blocker(label),
                    FindingKind::Unresolved => Finding::Unresolved(label),
                    FindingKind::Resolved => Finding::Resolved(label),
                }
            })
            .collect();
        Ok(Self {
            findings,
            summary: parsed.summary,
            notes: parsed.notes,
        })
    }

    /// Parse a markdown body using the legacy line-marker regex.
    /// Public so tests and the orchestrator's retry-inlining path
    /// can opt in explicitly; new code should prefer
    /// `Critique::load` which handles JSON-first resolution.
    pub fn parse(text: &str) -> Self {
        let mut findings = Vec::new();
        let mut in_fence = false;
        for raw in text.lines() {
            // Track fenced code blocks so a `- BLOCKER:` inside a
            // sample (the critique author quoting placeholder text
            // back at the reader, etc.) doesn't fire as a real
            // finding and dirty the gate. Per CommonMark: a line
            // whose first non-whitespace run is ``` (or more) opens
            // or closes a fenced code block. We accept ``` or ~~~,
            // any leading whitespace, and optional info string.
            // See orchestrator audit #4 (2026-05-16).
            if is_fence_delimiter(raw) {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
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
        Self {
            findings,
            summary: String::new(),
            notes: String::new(),
        }
    }

    /// Load a critique from disk. Resolves the canonical JSON form
    /// first: given `<step>-critique.md`, look for
    /// `<step>-critique.json` alongside and parse THAT if it
    /// exists. Falls back to the markdown body's regex parse only
    /// when no JSON sibling is present (legacy projects).
    pub fn load(path: &Path) -> Result<Self> {
        if let Some(json_path) = json_sibling(path)
            && json_path.exists()
        {
            let text = std::fs::read_to_string(&json_path).map_err(|source| Error::Io {
                path: json_path.clone(),
                source,
            })?;
            return Self::from_json(&text);
        }
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

/// `true` if `raw` is a CommonMark fenced-code-block opener or
/// closer (``` or ~~~, with any leading whitespace and an
/// optional info string). Used to gate finding-marker matching
/// so `- BLOCKER: example` quoted inside a code block doesn't
/// fire as a real finding. Shared with `dashboard::parse_with_lines`.
pub(super) fn is_fence_delimiter(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.starts_with(b"```") || bytes.starts_with(b"~~~") {
        return true;
    }
    false
}
