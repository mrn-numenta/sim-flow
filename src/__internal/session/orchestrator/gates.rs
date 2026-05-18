//! Gate evaluation, critique parsing, and finding-block extraction.
//!
//! Three related concerns the turn loop calls into during auto mode
//! and on every critique salvage path:
//!
//! - `evaluate_structural_gate` runs the step's gate skipping the
//!   `CritiqueClean` check (critique sessions can't fix critique
//!   cleanliness mid-flight).
//! - `salvage_critique_json` / `scan_balanced_json` recover a
//!   critique JSON body that the agent emitted with the wrong
//!   fence (or no fence at all) so we don't burn a turn forcing a
//!   re-emit.
//! - The `FindingKind` enum + matchers (`line_kind`,
//!   `parse_blocker_lines`, `extract_blocker_blocks`,
//!   `extract_gate_finding_blocks`) walk a critique markdown body
//!   pulling out gate-relevant findings; the JSON-first
//!   `retry_gate_finding_blocks` is the canonical entry point for
//!   the retry-inline path and falls back to the markdown helpers
//!   here when no JSON sibling is on disk.

use std::path::Path;

use crate::Result;
use crate::client::SessionKind;
use crate::gate::{self, GateCheck, GateReport};
use crate::session::tools;
use crate::steps::StepDescriptor;

use super::artifacts::extract_artifacts;

/// Evaluate the step's gate but skip the `CritiqueClean` checks.
/// Used by auto-mode work sessions to decide whether the structural
/// part of the gate is clean -- the critique-clean part can only
/// pass after the separate critique session runs.
///
/// `walk_scope` controls which check list is used. During a
/// milestone walk the wind-down decision only needs to know whether
/// the agent's *current piece of work* meets the quality bar -- the
/// expensive integration checks (`cargo test --test elaboration`,
/// the cross-module symbol greps, `milestones_all_implemented`)
/// can't possibly pass until the LAST milestone lands, so running
/// them on every no-artifact turn just burns cargo time and surfaces
/// confusing failures to the agent. When `walk_scope = true` AND
/// the step defines a non-empty `walk_gate_checks`, evaluate THAT
/// list instead. Otherwise fall back to the full `gate_checks`,
/// preserving existing behavior for non-walking steps and for steps
/// that haven't opted into the split.
pub(super) fn evaluate_structural_gate(
    project_dir: &Path,
    step: &StepDescriptor,
    walk_scope: bool,
) -> Result<GateReport> {
    let source = if walk_scope && !step.walk_gate_checks.is_empty() {
        &step.walk_gate_checks
    } else {
        &step.gate_checks
    };
    let filtered: Vec<GateCheck> = source
        .iter()
        .filter(|c| !matches!(c, GateCheck::CritiqueClean { .. }))
        .cloned()
        .collect();
    gate::evaluate(project_dir, &filtered)
}

/// Heuristic: did this turn's response contain any artifact-write
/// fenced block? Used to detect "agent is stalling without producing
/// output" turns in auto mode.
/// Mirror of the critique-session fallback in `run_session`: returns
/// false (i.e. "an artifact was produced") whenever a fenced
/// artifact-write block extracted OR the session is a critique with
/// substantive body content and no tool calls. Used at the
/// auto-iteration cap check so a turn that wrote the critique file
/// Attempt to salvage a critique JSON object embedded in
/// `assistant_text` when the agent's output has structurally valid
/// content but the wrong fence shape (e.g. ` ```json ` instead of
/// ` ```docs/critiques/<step>-critique.json `, or just bare prose
/// wrapping a JSON literal).
///
/// Strategy: scan for every `{` and try to balance braces (string-
/// aware) until we find a slice that parses as `CritiqueJson` with
/// the matching `step` id. Returns the salvaged JSON body (the
/// bytes we'll write to disk verbatim) or `None` if nothing
/// recognizable was found.
///
/// This is a recovery path; the canonical contract is still that
/// the agent emits a fenced block whose info-string is the path.
pub(super) fn salvage_critique_json(text: &str, step_id: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let Some(end) = scan_balanced_json(bytes, start) else {
            continue;
        };
        let candidate = &text[start..end];
        let Ok(parsed) = serde_json::from_str::<crate::critique::CritiqueJson>(candidate) else {
            continue;
        };
        if parsed.step != step_id {
            // A JSON literal with the right shape but a different
            // step id is not OUR critique; skip rather than risk
            // mis-attributing.
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

/// String-aware brace-balanced scan from a `{` at `start`. Returns
/// the byte offset just past the matching `}`, or `None` if the
/// braces are unbalanced or a string literal runs to EOF. Tracks
/// `\\`-escaped quotes inside strings.
fn scan_balanced_json(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// via the fallback doesn't get counted as "no artifact" and re-
/// trigger the cap.
pub(super) fn effective_artifacts_empty(response_text: &str, kind: SessionKind) -> bool {
    if !extract_artifacts(response_text).is_empty() {
        return false;
    }
    if kind == SessionKind::Critique
        && tools::extract_tool_calls(response_text).is_empty()
        && !response_text.trim().is_empty()
    {
        return false;
    }
    true
}

/// JSON-first gate-finding extractor for the retry-inline path. When
/// `<step>-critique.json` exists, parse it and return one
/// formatted block per gate-failing finding (header line +
/// body, mirroring the markdown shape so the agent's retry context
/// reads naturally). Falls back to the legacy markdown regex
/// (`extract_gate_finding_blocks`) when no JSON sibling is on disk so
/// projects mid-flight before the migration keep working.
pub(super) fn retry_gate_finding_blocks(project_dir: &Path, step_id: &str) -> Vec<String> {
    let json_rel = format!("docs/critiques/{step_id}-critique.json");
    let json_abs = project_dir.join(&json_rel);
    if let Ok(text) = std::fs::read_to_string(&json_abs)
        && let Ok(parsed) = serde_json::from_str::<crate::critique::CritiqueJson>(&text)
    {
        return parsed
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f.kind,
                    crate::critique::FindingKind::Blocker
                        | crate::critique::FindingKind::Unresolved
                )
            })
            .map(|f| {
                let label = match f.kind {
                    crate::critique::FindingKind::Blocker => "BLOCKER",
                    crate::critique::FindingKind::Unresolved => "UNRESOLVED",
                    crate::critique::FindingKind::Resolved => {
                        unreachable!("filter excludes resolved")
                    }
                };
                if f.body.trim().is_empty() {
                    format!("**{label}: {}**", f.title.trim())
                } else {
                    format!("**{label}: {}**\n\n{}", f.title.trim(), f.body.trim())
                }
            })
            .collect();
    }
    let md_abs = project_dir.join(format!("docs/critiques/{step_id}-critique.md"));
    let body = std::fs::read_to_string(&md_abs).unwrap_or_default();
    extract_gate_finding_blocks(&body)
}

pub(super) fn can_auto_wind_down_clean_work_session(
    work_retry_has_prior_blockers: bool,
    session_persisted_writes: bool,
) -> bool {
    !work_retry_has_prior_blockers || session_persisted_writes
}

/// Pull each `BLOCKER:` block out of a critique markdown file as a
/// MULTI-LINE string covering the line that opens with `BLOCKER:`
/// (after stripping list-markers / bold) plus every following line
/// until the next finding marker (`BLOCKER:` / `UNRESOLVED:` /
/// `RESOLVED:`), a markdown heading, a horizontal rule, or EOF. The
/// header line is included so the agent sees the prefix verbatim;
/// sub-bullets and explanatory prose that follow stay attached.
///
/// `extract_gate_finding_blocks().len()` is the gate-relevant count of
/// findings and replaces the older single-line `parse_blocker_lines`
/// helper. Whole blocks (rather than just header lines) are what we
/// inline into a focused critique-retry: a multi-bullet BLOCKER
/// describing three sub-gaps loses all the actionable detail if
/// only the first line survives.
fn extract_gate_finding_blocks(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if matches!(
            line_kind(lines[i]),
            Some(FindingKind::Blocker | FindingKind::Unresolved)
        ) {
            let start = i;
            let mut j = i + 1;
            while j < lines.len() && !is_block_terminator(lines[j]) {
                j += 1;
            }
            // Trim trailing blank lines so blocks read cleanly when
            // joined back together.
            let mut end = j;
            while end > start + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            out.push(lines[start..end].join("\n"));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn extract_blocker_blocks(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if line_kind(lines[i]) == Some(FindingKind::Blocker) {
            let start = i;
            let mut j = i + 1;
            while j < lines.len() && !is_block_terminator(lines[j]) {
                j += 1;
            }
            let mut end = j;
            while end > start + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            out.push(lines[start..end].join("\n"));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum FindingKind {
    Blocker,
    Unresolved,
    Resolved,
}

/// Match a finding marker at the start of a line. The recognized
/// shapes (after lenient prefix-stripping) are:
///
/// - `BLOCKER:` / `BLOCKERS:` / case-variants
/// - `UNRESOLVED:` / `UNRESOLVEDS:` / case-variants
/// - `RESOLVED:` / `RESOLVEDS:` / case-variants
///
/// The leading prefix-strip allows: list markers (`-`, `*`, `+`),
/// markdown headings (`#`+), whitespace, bold/underline (`**` /
/// `__`), and one stray non-alphanumeric "decoration" character
/// (emoji like `❌`, dingbats, checkmarks). Today's qwen run emitted
/// `### ❌ BLOCKER: ...` as a heading-with-emoji and the prior
/// strict-list-only matcher silently passed the gate; this is the
/// fix.
static FINDING_MARKER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        // MUST stay in sync with `__internal/critique.rs::FINDING_MARKER_RE`.
        // See that comment for the prefix ordering rationale.
        r"^[\s\-\*\+#>]*(?:\d+\.\s+)?(?:\*\*|__)?\s*[^\w\s]*\s*(?P<kind>(?i)blockers?|unresolveds?|resolveds?):"
    )
    .expect("finding-marker regex compiles")
});

pub(super) fn line_kind(line: &str) -> Option<FindingKind> {
    let m = FINDING_MARKER_RE.captures(line)?;
    let kind = m.name("kind")?.as_str().to_ascii_lowercase();
    if kind.starts_with("blocker") {
        Some(FindingKind::Blocker)
    } else if kind.starts_with("unresolved") {
        Some(FindingKind::Unresolved)
    } else if kind.starts_with("resolved") {
        Some(FindingKind::Resolved)
    } else {
        None
    }
}

fn is_block_terminator(line: &str) -> bool {
    if line_kind(line).is_some() {
        return true;
    }
    // A heading is a terminator unless `line_kind` already claimed
    // it as a finding (handled above): `### Section header` ends a
    // prior block; `### ❌ BLOCKER: ...` IS a block-start, not a
    // terminator.
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return true;
    }
    let only_dashes = line.trim();
    if (only_dashes.starts_with("---") || only_dashes.starts_with("***"))
        && only_dashes
            .chars()
            .all(|c| c == '-' || c == '*' || c == ' ')
    {
        return true;
    }
    false
}

/// Backwards-compatible single-line view: each entry is the
/// `BLOCKER:` header line (without its body) for callers that just
/// want a count or a one-line summary. Internally implemented on
/// top of `extract_blocker_blocks` so both helpers agree.
pub(super) fn parse_blocker_lines(body: &str) -> Vec<String> {
    extract_blocker_blocks(body)
        .iter()
        .filter_map(|block| block.lines().next().map(String::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blocker_lines_handles_common_shapes() {
        // The critique format is free-form markdown; agents have
        // emitted blockers as `- BLOCKER: ...`, `* **BLOCKER:** ...`,
        // and `BLOCKER: ...` (bare) across runs. The retry-detection
        // path must recognize all of them so a critique-retry doesn't
        // silently fall back to the full evaluation just because a
        // model preferred bold-font BLOCKER markers.
        let body = "\
# DM0 Critique\n\
\n\
- BLOCKER: missing gate budget\n\
* **BLOCKER:** ambiguous reset semantics\n\
BLOCKER: no examples for stage 2\n\
- UNRESOLVED: layout details\n\
- RESOLVED: clock domain decision\n\
random text BLOCKER: not a heading\n\
";
        let blockers = parse_blocker_lines(body);
        assert_eq!(blockers.len(), 3, "got {blockers:?}");
        assert!(blockers[0].contains("missing gate budget"));
        assert!(blockers[1].contains("ambiguous reset semantics"));
        assert!(blockers[2].contains("no examples for stage 2"));
    }

    #[test]
    fn parse_blocker_lines_returns_empty_when_clean() {
        // A critique that resolved cleanly emits only RESOLVED /
        // UNRESOLVED lines. The retry-detection path keys off "any
        // BLOCKER present" -- empty here means the next critique
        // pass should run the full evaluation, not the focused-retry
        // shortcut.
        let body = "- RESOLVED: clock domain.\n- UNRESOLVED: stage 2 timing.\n";
        assert!(parse_blocker_lines(body).is_empty());
    }

    #[test]
    fn critique_retry_work_requires_a_persisted_write_before_clean_wind_down() {
        assert!(!can_auto_wind_down_clean_work_session(true, false));
        assert!(can_auto_wind_down_clean_work_session(true, true));
        assert!(can_auto_wind_down_clean_work_session(false, false));
    }

    #[test]
    fn can_auto_wind_down_clean_work_session_logic() {
        // No prior blockers AND no writes -> wind-down OK.
        assert!(can_auto_wind_down_clean_work_session(false, false));
        // No prior blockers, writes happened -> wind-down OK.
        assert!(can_auto_wind_down_clean_work_session(false, true));
        // Prior blockers but writes happened -> wind-down OK (the
        // session at least attempted a fix).
        assert!(can_auto_wind_down_clean_work_session(true, true));
        // Prior blockers and NO writes -> wind-down NOT OK (agent
        // bypassed the issues without actually fixing them).
        assert!(!can_auto_wind_down_clean_work_session(true, false));
    }

    #[test]
    fn parse_blocker_lines_ignores_inline_mentions() {
        // Don't trigger on prose that mentions the word BLOCKER
        // mid-sentence; we only care about heading-shaped lines.
        let body = "We discussed the BLOCKER: marker convention earlier.\n";
        assert!(parse_blocker_lines(body).is_empty());
    }

    #[test]
    fn extract_blocker_blocks_captures_multi_line_body() {
        // Real DM3a critiques emit a single BLOCKER followed by
        // sub-bullets and a fix recipe. The whole block must come
        // through so the focused-retry context still contains the
        // actionable detail.
        let body = "\
### BLOCKER 2 - coverage.md incomplete\n\
\n\
BLOCKER: `coverage.md` was partially updated, but gaps persist:\n\
\n\
- **Numeric threshold** - still absent.\n\
- **Exclusions with reasons** - command-line flags are not prose.\n\
- **Report path** - only the directory is named.\n\
\n\
The fix is to update `coverage.md` to add (a) ... (b) ... (c) ...\n\
\n\
### BLOCKER 3 - traceability table\n\
\n\
RESOLVED: traceability section satisfies check 11.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        let block = &blocks[0];
        assert!(block.starts_with("BLOCKER: `coverage.md` was partially updated"));
        assert!(block.contains("Numeric threshold"));
        assert!(block.contains("Exclusions with reasons"));
        assert!(block.contains("Report path"));
        assert!(block.contains("The fix is to update"));
        // Must stop before the next heading.
        assert!(!block.contains("### BLOCKER 3"));
        assert!(!block.contains("RESOLVED: traceability"));
    }

    #[test]
    fn extract_blocker_blocks_terminates_on_finding_marker() {
        // A bare BLOCKER followed by a sibling RESOLVED on the next
        // line should yield exactly the BLOCKER body up to (not
        // including) the RESOLVED line.
        let body = "\
- BLOCKER: missing gate budget. The spec lacks a hard cycle\n\
  bound for the worst-case path through stage 2.\n\
- RESOLVED: clock domain decision recorded.\n\
- UNRESOLVED: stage 2 timing still pending.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("missing gate budget"));
        assert!(blocks[0].contains("worst-case path"));
        assert!(!blocks[0].contains("RESOLVED"));
        assert!(!blocks[0].contains("UNRESOLVED"));
    }

    #[test]
    fn extract_blocker_blocks_terminates_on_horizontal_rule() {
        // Markdown horizontal rules (`---`, `***`) commonly delimit
        // sections in our critique template; they end a BLOCKER body.
        let body = "\
BLOCKER: foo is broken because of bar.\n\
\n\
Fix it by doing X.\n\
\n\
---\n\
\n\
## Carried-Forward Items\n\
\n\
UNRESOLVED: shorthand references.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("foo is broken"));
        assert!(blocks[0].contains("Fix it by doing X"));
        assert!(!blocks[0].contains("---"));
        assert!(!blocks[0].contains("Carried-Forward"));
    }

    #[test]
    fn extract_blocker_blocks_handles_header_shaped_body_lines() {
        // `### BLOCKER 1` is a heading describing a finding, not a
        // finding line itself. The actual `BLOCKER:` marker lives on
        // a later line. The header should NOT be captured as a
        // separate finding, and it terminates the prior block.
        let body = "\
### BLOCKER 1 - stress.md target coverage\n\
\n\
RESOLVED: stress.md exercises every target.\n\
\n\
### BLOCKER 2 - coverage.md\n\
\n\
BLOCKER: numeric threshold missing.\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert!(blocks[0].starts_with("BLOCKER: numeric threshold"));
    }

    #[test]
    fn extract_blocker_blocks_trims_trailing_blank_lines() {
        // Blocks are joined back together when inlined into the
        // retry prompt; trailing blank lines would compound into
        // visual noise. Trim them.
        let body = "\
BLOCKER: thing is wrong.\n\
\n\
Some explanation.\n\
\n\
\n\
\n\
## Next Section\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].ends_with('\n'));
        assert!(blocks[0].ends_with("Some explanation."));
    }

    #[test]
    fn line_kind_matches_heading_with_emoji() {
        // The actual qwen run today emitted `### ❌ BLOCKER:` --
        // markdown H3 + dingbat + finding marker. The strict matcher
        // returned None, the gate saw zero blockers, and the step
        // advanced clean. Heading-style findings MUST match now.
        assert_eq!(
            line_kind("### ❌ BLOCKER: Report output path missing"),
            Some(FindingKind::Blocker),
        );
        assert_eq!(line_kind("# BLOCKER: foo"), Some(FindingKind::Blocker));
        assert_eq!(
            line_kind("## ✅ RESOLVED: clock-domain decision recorded"),
            Some(FindingKind::Resolved),
        );
    }

    #[test]
    fn line_kind_matches_plural_and_case_variants() {
        // Agents drift across forms; the gate parser must be lenient
        // in the FINDING half so blockers aren't silently dropped on
        // a case slip or a stray plural.
        assert_eq!(line_kind("BLOCKERS: two open"), Some(FindingKind::Blocker));
        assert_eq!(line_kind("Blocker: foo"), Some(FindingKind::Blocker));
        assert_eq!(line_kind("blocker: foo"), Some(FindingKind::Blocker));
        assert_eq!(
            line_kind("- **BLOCKER:** ambiguous reset"),
            Some(FindingKind::Blocker),
        );
        assert_eq!(
            line_kind("> BLOCKER: blockquote-styled finding"),
            Some(FindingKind::Blocker),
        );
    }

    #[test]
    fn line_kind_rejects_inline_mentions_and_section_titles() {
        // Section heading discussing a blocker (no colon-after) is
        // NOT a finding -- it's prose. And mid-sentence mentions
        // never count.
        assert_eq!(line_kind("### BLOCKER 1 - stress.md target coverage"), None,);
        assert_eq!(
            line_kind("We discussed the BLOCKER: marker convention earlier."),
            None,
        );
        assert_eq!(line_kind(""), None);
        assert_eq!(line_kind("## Carried-Forward Items"), None);
    }

    #[test]
    fn extract_blocker_blocks_captures_heading_style_finding() {
        // End-to-end: a heading-with-emoji BLOCKER is correctly
        // extracted as a multi-line block (the regression that
        // motivated this change).
        let body = "\
## Prior BLOCKER 1: coverage.md\n\
\n\
### ❌ BLOCKER: Report output path missing\n\
\n\
The run command names a directory, not a file.\n\
\n\
The fix is to add a Report Output section.\n\
\n\
### ✅ RESOLVED: numeric threshold\n\
";
        let blocks = extract_blocker_blocks(body);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert!(blocks[0].contains("Report output path missing"));
        assert!(blocks[0].contains("The run command names a directory"));
        assert!(blocks[0].contains("The fix is to add"));
        assert!(!blocks[0].contains("RESOLVED"));
    }

    #[test]
    fn salvage_critique_json_extracts_from_json_fence() {
        // Common failure mode: agent uses ```json instead of
        // ```docs/critiques/<step>-critique.json. The fallback
        // sees no fenced PATH block, no fenced TOOL block, and no
        // BLOCKER markers. Salvage should recover the JSON.
        let text = r#"Here is the critique:

```json
{"step":"DM1","summary":"one finding","findings":[{"kind":"blocker","title":"x","body":""}],"notes":""}
```

Hope that helps."#;
        let salvaged = salvage_critique_json(text, "DM1").expect("salvaged");
        let parsed: crate::critique::CritiqueJson = serde_json::from_str(&salvaged).unwrap();
        assert_eq!(parsed.step, "DM1");
        assert_eq!(parsed.findings.len(), 1);
    }

    #[test]
    fn salvage_critique_json_extracts_from_bare_prose() {
        // Agent forgot to fence at all but emitted valid JSON
        // inline. Brace-balanced scan should still find it.
        let text = r#"Critique: {"step":"DM0","summary":"","findings":[],"notes":""}. Done."#;
        let salvaged = salvage_critique_json(text, "DM0").expect("salvaged");
        let parsed: crate::critique::CritiqueJson = serde_json::from_str(&salvaged).unwrap();
        assert_eq!(parsed.step, "DM0");
    }

    #[test]
    fn salvage_critique_json_rejects_wrong_step_id() {
        // The matching step id check prevents mis-attributing a
        // critique JSON the agent quoted from a prior step (e.g.
        // pasted DM0's critique into a DM1 turn).
        let text = r#"```json
{"step":"DM0","summary":"","findings":[]}
```"#;
        assert!(salvage_critique_json(text, "DM1").is_none());
    }

    #[test]
    fn salvage_critique_json_returns_none_for_non_critique_json() {
        // Any old JSON object shouldn't trip the salvage; it must
        // parse as the strict critique schema.
        let text = r#"```json
{"foo": "bar", "baz": 42}
```"#;
        assert!(salvage_critique_json(text, "DM0").is_none());
    }

    #[test]
    fn scan_balanced_json_handles_strings_with_braces() {
        // A `}` inside a string literal must not close the
        // outer object early.
        let text = r#"{"x": "a } b", "y": 1}"#;
        let end = scan_balanced_json(text.as_bytes(), 0).expect("balanced");
        assert_eq!(end, text.len());
    }

    #[test]
    fn salvage_critique_json_finds_first_well_formed_object_matching_step() {
        let text = r#"
            preamble blah blah
            ```json
            {"step":"DM0","summary":"good","findings":[]}
            ```
            and some other braces { unrelated: true }
        "#;
        let got = salvage_critique_json(text, "DM0");
        assert!(got.is_some(), "got {got:?}");
        let inner = got.unwrap();
        assert!(inner.contains("\"step\":\"DM0\""));
    }

    #[test]
    fn salvage_critique_json_skips_objects_with_wrong_step_id() {
        let text = r#"{"step":"DM1","summary":"x","findings":[]}"#;
        assert!(salvage_critique_json(text, "DM0").is_none());
    }

    #[test]
    fn salvage_critique_json_returns_none_when_no_object_matches() {
        assert!(salvage_critique_json("no json here", "DM0").is_none());
        assert!(salvage_critique_json("{", "DM0").is_none());
    }

    #[test]
    fn scan_balanced_json_handles_nested_braces_and_escapes() {
        let body = br#"{"a": {"b": 1}, "s": "with \"quoted\" and {brace} chars"}"#;
        let end = scan_balanced_json(body, 0);
        assert_eq!(end, Some(body.len()));
        // Unbalanced -> None.
        let body = br#"{"a": 1"#;
        assert!(scan_balanced_json(body, 0).is_none());
        // String with no closing quote runs to EOF -> None.
        let body = br#"{"a": "never closed"#;
        assert!(scan_balanced_json(body, 0).is_none());
    }

    #[test]
    fn effective_artifacts_empty_false_when_response_has_artifact_fence() {
        let body = "Here is the file.\n\n```docs/spec.md\n# spec\n```\n";
        assert!(!effective_artifacts_empty(
            body,
            crate::client::SessionKind::Work
        ));
    }

    #[test]
    fn effective_artifacts_empty_critique_lenient_when_response_has_prose() {
        // Critique with prose + no tool calls + no artifacts -> non-empty.
        let prose_only = "Here is my finding.\n\nThe code is fine.\n";
        assert!(!effective_artifacts_empty(
            prose_only,
            crate::client::SessionKind::Critique
        ));
        // Same prose in Work mode is still empty (Work requires an
        // actual artifact write).
        assert!(effective_artifacts_empty(
            prose_only,
            crate::client::SessionKind::Work
        ));
    }

    #[test]
    fn effective_artifacts_empty_true_when_only_a_tool_call_and_no_prose() {
        let body = "```tool:read_file\nsrc/lib.rs\n```\n";
        assert!(effective_artifacts_empty(
            body,
            crate::client::SessionKind::Work
        ));
        // For critique, the lenient rule requires NO tool call to flip
        // to "non-empty", so this is still empty.
        assert!(effective_artifacts_empty(
            body,
            crate::client::SessionKind::Critique
        ));
    }
}
