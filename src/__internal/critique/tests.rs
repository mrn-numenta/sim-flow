//! Shared tests for the critique module (core parser, render,
//! dashboard helpers). Kept in one file so test fixtures can be
//! reused across the three concerns without re-importing them in
//! each submodule.

use std::path::Path;

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
    // Both Blocker and Unresolved are blocking; Unresolved is
    // a previously-flagged finding still outstanding, treated
    // with the same gate semantics as a fresh Blocker.
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

fn write_critique(dir: &Path, step: &str, ext: &str, body: &str) {
    let path = dir.join(format!("docs/critiques/{step}-critique.{ext}"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
}

#[test]
fn list_critique_entries_empty_when_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let out = list_critique_entries(tmp.path()).expect("ok");
    assert!(out.is_empty());
}

#[test]
fn list_critique_entries_reads_markdown_findings_with_line_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    write_critique(
        tmp.path(),
        "DM3a",
        "md",
        "intro line\nBLOCKER: spec mismatch\n- RESOLVED: stale path\n",
    );
    let out = list_critique_entries(tmp.path()).expect("ok");
    assert_eq!(out.len(), 1);
    let entry = &out[0];
    assert_eq!(entry.step, "DM3a");
    assert_eq!(entry.findings.len(), 2);
    assert_eq!(entry.findings[0].kind, DashboardFindingKind::Blocker);
    assert_eq!(entry.findings[0].text, "spec mismatch");
    assert_eq!(entry.findings[0].line, 2);
    assert_eq!(entry.findings[1].kind, DashboardFindingKind::Resolved);
    assert_eq!(entry.findings[1].line, 3);
    assert!(entry.has_blocking);
}

#[test]
fn list_critique_entries_prefers_json_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    // JSON declares two findings; markdown body has different text
    // but is kept for display (and surfaces as the entry path).
    let json = r#"{
        "step": "DM2c",
        "summary": "from json",
        "findings": [
            { "kind": "blocker", "title": "missing milestone" },
            { "kind": "resolved", "title": "ok" }
        ],
        "notes": ""
    }"#;
    write_critique(tmp.path(), "DM2c", "json", json);
    write_critique(tmp.path(), "DM2c", "md", "# Human view\nBLOCKER: ignored\n");
    let out = list_critique_entries(tmp.path()).expect("ok");
    assert_eq!(out.len(), 1);
    let entry = &out[0];
    // findings come from JSON (two), not the markdown (one)
    assert_eq!(entry.findings.len(), 2);
    assert!(entry.findings[0].text == "missing milestone");
    // surfaced path is the markdown view when present
    assert!(entry.path.ends_with("DM2c-critique.md"));
    // body is the markdown text (preserved for the dashboard UI)
    assert!(entry.body.contains("# Human view"));
    assert!(entry.has_blocking);
}

#[test]
fn list_critique_entries_surfaces_malformed_json_as_blocker() {
    // Behavior change for orchestrator audit #18: previously
    // the dashboard silently fell through to parsing the md
    // body when the JSON was malformed, producing "0 findings,
    // gate clean" while the gate refused to advance. Now we
    // synthesize a Blocker finding naming the parse error so
    // the dashboard's view aligns with the gate's refusal.
    let tmp = tempfile::tempdir().unwrap();
    write_critique(tmp.path(), "DM0", "json", "{ this is not valid JSON");
    write_critique(tmp.path(), "DM0", "md", "BLOCKER: not surfaced from md\n");
    let out = list_critique_entries(tmp.path()).expect("ok");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].findings.len(), 1);
    assert!(matches!(
        out[0].findings[0].kind,
        DashboardFindingKind::Blocker
    ));
    assert!(
        out[0].findings[0]
            .text
            .starts_with("malformed critique JSON:"),
        "got: {}",
        out[0].findings[0].text
    );
    assert!(out[0].has_blocking);
}

#[test]
fn list_critique_entries_sorted_by_step_id() {
    let tmp = tempfile::tempdir().unwrap();
    write_critique(tmp.path(), "DM3a", "md", "BLOCKER: a\n");
    write_critique(tmp.path(), "DM0", "md", "RESOLVED: zero\n");
    write_critique(tmp.path(), "DM2c", "md", "UNRESOLVED: mid\n");
    let out = list_critique_entries(tmp.path()).expect("ok");
    let ids: Vec<&str> = out.iter().map(|e| e.step.as_str()).collect();
    assert_eq!(ids, vec!["DM0", "DM2c", "DM3a"]);
}

#[test]
fn read_critique_entry_returns_none_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs/critiques")).unwrap();
    let entry = read_critique_entry(tmp.path(), "DM4b").expect("ok");
    assert!(entry.is_none());
}

#[test]
fn is_fence_delimiter_recognizes_both_backtick_and_tilde_fences() {
    assert!(is_fence_delimiter("```"));
    assert!(is_fence_delimiter("``` rust"));
    assert!(is_fence_delimiter("~~~"));
    assert!(is_fence_delimiter("   ```text"));
    assert!(!is_fence_delimiter("``"));
    assert!(!is_fence_delimiter("plain line"));
    assert!(!is_fence_delimiter(""));
}
