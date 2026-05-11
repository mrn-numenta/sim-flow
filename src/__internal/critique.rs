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
    fn as_label(self) -> &'static str {
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
static FINDING_MARKER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^[\s\-\*\+#>]*(?:\d+\.\s+)?(?:\*\*|__)?\s*[^\w\s]*\s*(?P<kind>(?i)blockers?|unresolveds?|resolveds?):(?P<rest>.*)$"
    )
    .expect("finding-marker regex compiles")
});

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

/// Resolve the JSON sibling for a markdown critique path. Given
/// `docs/critiques/DM3a-critique.md`, returns
/// `docs/critiques/DM3a-critique.json`. Returns `None` for paths
/// that don't end in `.md` (already a JSON path, or some other
/// extension we don't expect).
pub fn json_sibling(md_path: &Path) -> Option<std::path::PathBuf> {
    let ext = md_path.extension().and_then(|e| e.to_str())?;
    if !ext.eq_ignore_ascii_case("md") {
        return None;
    }
    Some(md_path.with_extension("json"))
}

/// True iff `rel_path` looks like a critique JSON artifact path
/// (`docs/critiques/<step>-critique.json`). Used by the
/// orchestrator to decide whether to render a markdown view after
/// the agent writes the file.
pub fn is_critique_json_path(rel_path: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    normalized.starts_with("docs/critiques/")
        && normalized.ends_with("-critique.json")
        && !normalized.contains("..")
}

/// Render a freshly-written critique JSON into its markdown
/// sibling. Idempotent: re-runs overwrite the previous render.
/// Errors are wrapped so the orchestrator can surface "agent
/// emitted malformed critique JSON" as a clear failure rather
/// than silently leaving a stale `.md` on disk.
///
/// Returns the markdown bytes that were written so the caller can
/// emit an `ArtifactWritten` event for the rendered file.
pub fn render_critique_markdown_to_disk(
    project_dir: &Path,
    json_rel_path: &str,
) -> Result<Vec<u8>> {
    let json_abs = project_dir.join(json_rel_path);
    let body = std::fs::read_to_string(&json_abs).map_err(|source| Error::Io {
        path: json_abs.clone(),
        source,
    })?;
    let parsed: CritiqueJson = serde_json::from_str(&body).map_err(|err| {
        Error::State(format!(
            "render_critique_markdown_to_disk: malformed critique JSON at {}: {err}",
            json_abs.display()
        ))
    })?;
    let md = render_markdown(&parsed);
    let md_rel = json_rel_path
        .strip_suffix(".json")
        .map(|s| format!("{s}.md"));
    let md_abs = match md_rel {
        Some(rel) => project_dir.join(rel),
        None => {
            return Err(Error::State(format!(
                "render_critique_markdown_to_disk: expected .json suffix on {json_rel_path}"
            )));
        }
    };
    if let Some(parent) = md_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&md_abs, md.as_bytes()).map_err(|source| Error::Io {
        path: md_abs.clone(),
        source,
    })?;
    Ok(md.into_bytes())
}

/// Render a JSON critique into deterministic markdown. The
/// orchestrator runs this each pass after the agent writes the
/// JSON so humans get a stable view without the agent having to
/// emit two artifacts. Output shape:
///
/// ```text
/// # <step> Critique
///
/// <summary>
///
/// ## <section-1>
///
/// - **BLOCKER:** title-1
///   body-1
/// - **UNRESOLVED:** title-2
///   body-2
///
/// ## <section-2>
/// ...
///
/// ## Notes
///
/// <notes>
/// ```
///
/// Sections are emitted in the order they FIRST appear in
/// `findings` (so the agent controls grouping); findings without a
/// section are bucketed under `## Findings`.
pub fn render_markdown(json: &CritiqueJson) -> String {
    let mut out = format!("# {} Critique\n\n", json.step);
    if !json.summary.trim().is_empty() {
        out.push_str(json.summary.trim());
        out.push_str("\n\n");
    }

    // Group findings by section, preserving first-appearance order.
    let mut sections: Vec<(String, Vec<&CritiqueFinding>)> = Vec::new();
    for f in &json.findings {
        let section_key = if f.section.trim().is_empty() {
            "Findings".to_string()
        } else {
            f.section.trim().to_string()
        };
        match sections.iter_mut().find(|(k, _)| k == &section_key) {
            Some(entry) => entry.1.push(f),
            None => sections.push((section_key, vec![f])),
        }
    }

    for (section, findings) in &sections {
        out.push_str(&format!("## {section}\n\n"));
        for f in findings {
            out.push_str(&format!(
                "- **{}:** {}\n",
                f.kind.as_label(),
                f.title.trim()
            ));
            let body = f.body.trim();
            if !body.is_empty() {
                for line in body.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        }
        out.push('\n');
    }

    if !json.notes.trim().is_empty() {
        out.push_str("## Notes\n\n");
        out.push_str(json.notes.trim());
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_critique_classifies_findings() {
        let body = r#"{
            "step": "DM3a",
            "summary": "two findings",
            "findings": [
                {"kind": "blocker", "section": "S1", "title": "missing", "body": "details"},
                {"kind": "unresolved", "section": "S2", "title": "minor nit", "body": ""}
            ],
            "notes": ""
        }"#;
        let c = Critique::from_json(body).unwrap();
        assert_eq!(c.findings.len(), 2);
        assert!(c.findings[0].is_blocking());
        assert!(c.findings[1].is_blocking());
        assert!(c.has_blocking());
        assert_eq!(c.blocking().len(), 2);
    }

    #[test]
    fn json_critique_rejects_unknown_fields() {
        // Schema is strict so a typo (e.g. `"finding"` singular)
        // surfaces as a parse error instead of dropping content.
        let body = r#"{
            "step": "DM3a",
            "finding": [{"kind": "blocker", "title": "x"}]
        }"#;
        assert!(Critique::from_json(body).is_err());
    }

    #[test]
    fn json_critique_rejects_unknown_finding_kind() {
        let body = r#"{
            "step": "DM3a",
            "findings": [{"kind": "warning", "title": "x"}]
        }"#;
        assert!(Critique::from_json(body).is_err());
    }

    #[test]
    fn render_markdown_groups_findings_by_section() {
        let json = CritiqueJson {
            step: "DM3a".into(),
            summary: "summary text".into(),
            findings: vec![
                CritiqueFinding {
                    kind: FindingKind::Blocker,
                    section: "Section A".into(),
                    title: "first".into(),
                    body: "body of first".into(),
                },
                CritiqueFinding {
                    kind: FindingKind::Unresolved,
                    section: "Section B".into(),
                    title: "second".into(),
                    body: "".into(),
                },
                CritiqueFinding {
                    kind: FindingKind::Resolved,
                    section: "Section A".into(),
                    title: "third".into(),
                    body: "body of third".into(),
                },
            ],
            notes: "free-form prose".into(),
        };
        let md = render_markdown(&json);
        assert!(md.starts_with("# DM3a Critique"));
        assert!(md.contains("summary text"));
        assert!(md.contains("## Section A"));
        assert!(md.contains("## Section B"));
        // Section-A findings appear in order; Section B sandwiched
        // between them in input is gathered into its own block.
        let section_a = md.find("## Section A").unwrap();
        let section_b = md.find("## Section B").unwrap();
        assert!(section_a < section_b, "Section A first, B second");
        assert!(md.contains("- **BLOCKER:** first"));
        assert!(md.contains("  body of first"));
        assert!(md.contains("- **UNRESOLVED:** second"));
        assert!(md.contains("- **RESOLVED:** third"));
        assert!(md.contains("## Notes"));
        assert!(md.contains("free-form prose"));
    }

    #[test]
    fn render_markdown_sectionless_findings_get_findings_heading() {
        let json = CritiqueJson {
            step: "DM0".into(),
            summary: "".into(),
            findings: vec![CritiqueFinding {
                kind: FindingKind::Blocker,
                section: "".into(),
                title: "x".into(),
                body: "".into(),
            }],
            notes: "".into(),
        };
        let md = render_markdown(&json);
        assert!(md.contains("## Findings"));
        assert!(md.contains("- **BLOCKER:** x"));
    }

    #[test]
    fn json_sibling_converts_md_path() {
        let p = Path::new("docs/critiques/DM3a-critique.md");
        let sibling = json_sibling(p).unwrap();
        assert_eq!(sibling, Path::new("docs/critiques/DM3a-critique.json"));
    }

    #[test]
    fn json_sibling_returns_none_for_non_md() {
        assert!(json_sibling(Path::new("docs/critiques/DM3a-critique.json")).is_none());
        assert!(json_sibling(Path::new("docs/critiques/DM3a")).is_none());
    }

    #[test]
    fn is_critique_json_path_recognizes_canonical_shape() {
        assert!(is_critique_json_path("docs/critiques/DM3a-critique.json"));
        assert!(is_critique_json_path("docs/critiques/DM2cd-critique.json"));
        // Wrong directory.
        assert!(!is_critique_json_path("docs/notes/DM3a-critique.json"));
        // Wrong suffix.
        assert!(!is_critique_json_path("docs/critiques/DM3a-critique.md"));
        // Path traversal is rejected -- defense in depth even though
        // write_artifact already checks `is_safe_relative_path`.
        assert!(!is_critique_json_path("docs/critiques/../escape.json"));
    }

    #[test]
    fn render_critique_markdown_to_disk_produces_md_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let json_rel = "docs/critiques/DM3a-critique.json";
        let json_abs = tmp.path().join(json_rel);
        std::fs::create_dir_all(json_abs.parent().unwrap()).unwrap();
        let json_body = r#"{
            "step": "DM3a",
            "summary": "two findings",
            "findings": [
                {"kind": "blocker", "section": "Section A", "title": "first", "body": "details"}
            ],
            "notes": ""
        }"#;
        std::fs::write(&json_abs, json_body).unwrap();
        let written = render_critique_markdown_to_disk(tmp.path(), json_rel).unwrap();
        let md_abs = tmp.path().join("docs/critiques/DM3a-critique.md");
        assert!(md_abs.exists());
        let on_disk = std::fs::read(&md_abs).unwrap();
        assert_eq!(on_disk, written);
        let md = String::from_utf8(written).unwrap();
        assert!(md.contains("# DM3a Critique"));
        assert!(md.contains("- **BLOCKER:** first"));
        assert!(md.contains("  details"));
    }

    #[test]
    fn render_critique_markdown_to_disk_surfaces_malformed_json_as_state_error() {
        let tmp = tempfile::tempdir().unwrap();
        let json_rel = "docs/critiques/DM3a-critique.json";
        let json_abs = tmp.path().join(json_rel);
        std::fs::create_dir_all(json_abs.parent().unwrap()).unwrap();
        std::fs::write(&json_abs, "{not json").unwrap();
        let err = render_critique_markdown_to_disk(tmp.path(), json_rel).unwrap_err();
        assert!(matches!(err, Error::State(_)));
    }

    #[test]
    fn load_resolves_json_sibling_when_md_path_passed() {
        // Existing call sites pass the markdown path
        // (`<step>-critique.md`) -- the gate, the auto driver, etc.
        // `Critique::load` resolves the JSON sibling first so those
        // call sites keep working without each one knowing about
        // the migration.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("DM3a-critique.json");
        std::fs::write(
            &json_path,
            r#"{
                "step": "DM3a",
                "findings": [
                    {"kind": "blocker", "title": "from json"}
                ]
            }"#,
        )
        .unwrap();
        // Also write a stale .md with NO blockers; the loader must
        // ignore it because the JSON sibling exists.
        let md_path = dir.join("DM3a-critique.md");
        std::fs::write(&md_path, "no markers in this body\n").unwrap();
        let c = Critique::load(&md_path).unwrap();
        assert_eq!(c.findings.len(), 1);
        assert!(c.has_blocking());
        assert!(c.findings[0].text().starts_with("from json"));
    }

    #[test]
    fn load_falls_back_to_md_when_no_json_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("DM3a-critique.md");
        std::fs::write(&md_path, "- BLOCKER: legacy markdown finding\n").unwrap();
        let c = Critique::load(&md_path).unwrap();
        assert_eq!(c.findings.len(), 1);
        assert!(c.has_blocking());
    }

    // Legacy markdown parser is preserved for projects that landed
    // critiques before the JSON migration; the regex tests below
    // exercise the same shapes the gate must keep parsing.

    #[test]
    fn markdown_classifies_prefixes() {
        let text = "\
# Critique

## Findings
- RESOLVED: FetchModule needed settle()
- UNRESOLVED: bubble rate higher than expected
- BLOCKER: Scoreboard does not verify ordering
";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 3);
        assert!(!c.findings[0].is_blocking());
        assert!(c.findings[1].is_blocking());
        assert!(c.findings[2].is_blocking());
        assert_eq!(c.blocking().len(), 2);
        assert!(c.has_blocking());
    }

    #[test]
    fn markdown_unresolved_only_blocks() {
        let text = "- UNRESOLVED: minor wording nit\n- UNRESOLVED: future cleanup\n";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 2);
        assert!(c.has_blocking());
        assert_eq!(c.blocking().len(), 2);
    }

    #[test]
    fn markdown_ignores_untagged_text() {
        let text = "Body text without markers.";
        let c = Critique::parse(text);
        assert!(c.findings.is_empty());
        assert!(!c.has_blocking());
    }

    #[test]
    fn markdown_handles_list_prefixes_and_leading_whitespace() {
        let text = "   - BLOCKER: indented with list marker";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 1);
        assert!(matches!(&c.findings[0], Finding::Blocker(_)));
    }

    #[test]
    fn markdown_matches_heading_style_blockers() {
        let text = "\
## BLOCKER: Milestone 02 -- artifact missing
### \u{274c} BLOCKER: scope discipline violation
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
    fn markdown_ignores_section_titles_about_blockers() {
        let text = "\
### BLOCKER 1 - stress.md target coverage
RESOLVED: stress.md exercises every target.
### BLOCKER 2 - coverage.md incomplete
BLOCKER: numeric threshold missing.
";
        let c = Critique::parse(text);
        assert_eq!(c.findings.len(), 2, "got {:?}", c.findings);
        assert!(matches!(&c.findings[0], Finding::Resolved(_)));
        assert!(matches!(&c.findings[1], Finding::Blocker(_)));
    }

    #[test]
    fn markdown_ignores_inline_blocker_mentions() {
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
