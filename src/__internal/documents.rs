//! Project-documents enumeration for the dashboard.
//!
//! Closes the last MVP-audit item: the dashboard used to walk
//! `STEP_ARTIFACTS`-named directories and parse markdown tables
//! directly from TypeScript (state/documents.ts, ~420 lines); now
//! the orchestrator owns the walker and emits JSON via
//! `sim-flow documents --json`.
//!
//! The shape mirrors the extension's `DocumentEntry` interface
//! verbatim (camelCase via serde rename_all) so the existing
//! dashboard renderers consume it without conversion. Per-file
//! previews fall into two kinds:
//!
//!   - `tableSection`: parse the first markdown table under a named
//!     `## <heading>` and ship it as `{ kind: "table", caption,
//!     headers, rows }`.
//!   - `markdown`: ship the file body capped at 8 KB as
//!     `{ kind: "markdown", body }`.
//!
//! Step artifact paths must stay in sync with the Rust step
//! descriptors in `steps/dm.rs` / `ds.rs` / `sv.rs`; the table
//! below is the dashboard's view of "what each step produces" and
//! the gate's view is the source of truth.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const WALK_FILE_CAP: usize = 200;
const PREVIEW_FULL_CAP_BYTES: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentEntry {
    /// Absolute path the host can pass to a "show document" action.
    pub abs_path: String,
    /// Project-relative path for display.
    pub rel_path: String,
    /// Bucket for grouping in the UI.
    pub category: DocumentCategory,
    /// Step id this document is associated with, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// File size in bytes, or null when the file does not exist.
    pub bytes: Option<u64>,
    /// Last modification time as ISO-8601 UTC, or null when missing.
    pub modified_at: Option<String>,
    /// True when the file is on disk (false rows are placeholders
    /// for expected outputs that haven't been written yet).
    pub exists: bool,
    /// Structured inline preview (table or markdown body). Omitted
    /// when no preview rule matches or the rule yielded no content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previews: Option<Vec<ArtifactPreview>>,
    /// Line count for code files (currently `.rs` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentCategory {
    WorkArtifact,
    Critique,
    SourceSpec,
    SpecPage,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ArtifactPreview {
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Markdown {
        body: String,
    },
}

// Step-artifact table. Mirror of state/documents.ts STEP_ARTIFACTS;
// must stay aligned with `steps/dm.rs` / `ds.rs`.
const STEP_ARTIFACTS: &[(&str, &[&str])] = &[
    ("DM0", &["docs/spec.md"]),
    ("DM1", &["docs/targets.md", "docs/testbench.md"]),
    (
        "DM2a",
        &[
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
    ),
    ("DM2b", &["docs/analysis/pipeline-mapping.md"]),
    ("DM2c", &["docs/impl-plan/"]),
    ("DM2cd", &["docs/impl-plan/"]),
    ("DM2d", &["src/", "tests/", "Cargo.toml"]),
    ("DM3a", &["docs/test-plan/"]),
    ("DM3ad", &["docs/test-plan/"]),
    ("DM3b", &["tests/"]),
    ("DM3c", &["tests/"]),
    ("DM4a", &["docs/perf-plan/"]),
    ("DM4ad", &["docs/perf-plan/"]),
    ("DM4b", &["docs/analysis/"]),
    ("DS0", &["docs/spec.md"]),
    ("DS1", &["docs/targets.md", "docs/testbench.md"]),
    (
        "DS2",
        &[
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
    ),
    ("DS3", &["docs/analysis/pipeline-mapping.md"]),
    ("DS4", &["docs/analysis/screening.md"]),
    ("DS5a", &["docs/analysis/prototype.md"]),
    ("DS5b", &["docs/analysis/smoke.md"]),
    ("DS5c", &["docs/analysis/full.md"]),
    ("DS6", &["docs/analysis/results.md"]),
];

const DM_FLOW: &[&str] = &[
    "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd", "DM2d", "DM3a", "DM3ad", "DM3b", "DM3c", "DM4a",
    "DM4ad", "DM4b",
];

const DS_FLOW: &[&str] = &[
    "DS0", "DS1", "DS2", "DS3", "DS4", "DS5a", "DS5b", "DS5c", "DS6",
];

fn step_order(flow: &str) -> &'static [&'static str] {
    match flow {
        "direct-modeling" => DM_FLOW,
        "design-study" => DS_FLOW,
        _ => &[],
    }
}

fn artifacts_for(step_id: &str) -> &'static [&'static str] {
    STEP_ARTIFACTS
        .iter()
        .find(|(id, _)| *id == step_id)
        .map(|(_, paths)| *paths)
        .unwrap_or(&[])
}

enum PreviewRule {
    TableSection(&'static str),
    Markdown,
}

fn preview_rule_for(rel_path: &str) -> Option<PreviewRule> {
    match rel_path {
        "docs/targets.md" => Some(PreviewRule::TableSection("Target Summary")),
        "docs/testbench.md" => Some(PreviewRule::Markdown),
        "docs/analysis/decomposition.md" => Some(PreviewRule::TableSection("Operation Summary")),
        "docs/analysis/data-movement.md" => Some(PreviewRule::TableSection("Edge Summary")),
        "docs/analysis/pipeline-mapping.md" => Some(PreviewRule::TableSection("Stage Summary")),
        _ => None,
    }
}

/// Walk the on-disk project layout and return one `DocumentEntry`
/// per expected work artifact (plus per-step critique files and
/// the source spec). Stable order: flow step order, then per-step
/// alphabetical for directory artifacts.
pub fn enumerate_project_documents(project_dir: &Path, flow: &str) -> Vec<DocumentEntry> {
    let mut out = Vec::new();
    for step_id in step_order(flow) {
        for rel in artifacts_for(step_id) {
            push_step_artifact(project_dir, step_id, rel, &mut out);
        }
        let critique_rel = format!("docs/critiques/{step_id}-critique.md");
        let critique_abs = project_dir.join(&critique_rel);
        if let Some(stats) = stat_file(&critique_abs) {
            out.push(DocumentEntry {
                abs_path: critique_abs.to_string_lossy().into_owned(),
                rel_path: critique_rel,
                category: DocumentCategory::Critique,
                step: Some((*step_id).to_string()),
                bytes: Some(stats.size),
                modified_at: Some(stats.modified),
                exists: true,
                previews: None,
                line_count: None,
            });
        }
    }
    push_source_spec(project_dir, &mut out);
    out
}

fn push_step_artifact(project_dir: &Path, step_id: &str, rel: &str, out: &mut Vec<DocumentEntry>) {
    if let Some(stripped) = rel.strip_suffix('/') {
        // Directory artifact: enumerate immediate children (up to
        // WALK_FILE_CAP) and emit one row each. Falls back to a
        // single placeholder when the dir is empty / missing.
        let dir_abs = project_dir.join(stripped);
        let mut added = 0;
        if dir_abs.is_dir() {
            for child in walk_dir_shallow(&dir_abs, WALK_FILE_CAP) {
                let rel_child = format!(
                    "{}/{}",
                    rel.trim_end_matches('/'),
                    child
                        .strip_prefix(&dir_abs)
                        .unwrap_or(&child)
                        .to_string_lossy()
                );
                let stats = stat_file(&child);
                let line_count = if has_code_extension(&child) {
                    count_lines(&child)
                } else {
                    None
                };
                out.push(DocumentEntry {
                    abs_path: child.to_string_lossy().into_owned(),
                    rel_path: rel_child,
                    category: DocumentCategory::WorkArtifact,
                    step: Some(step_id.to_string()),
                    bytes: stats.as_ref().map(|s| s.size),
                    modified_at: stats.as_ref().map(|s| s.modified.clone()),
                    exists: stats.is_some(),
                    previews: None,
                    line_count,
                });
                added += 1;
            }
        }
        if added == 0 {
            out.push(DocumentEntry {
                abs_path: dir_abs.to_string_lossy().into_owned(),
                rel_path: rel.to_string(),
                category: DocumentCategory::WorkArtifact,
                step: Some(step_id.to_string()),
                bytes: None,
                modified_at: None,
                exists: false,
                previews: None,
                line_count: None,
            });
        }
        return;
    }
    let abs = project_dir.join(rel);
    let stats = stat_file(&abs);
    let previews = stats
        .as_ref()
        .and_then(|s| build_previews(rel, &abs, s.size));
    let line_count = if has_code_extension(&abs) && stats.is_some() {
        count_lines(&abs)
    } else {
        None
    };
    out.push(DocumentEntry {
        abs_path: abs.to_string_lossy().into_owned(),
        rel_path: rel.to_string(),
        category: DocumentCategory::WorkArtifact,
        step: Some(step_id.to_string()),
        bytes: stats.as_ref().map(|s| s.size),
        modified_at: stats.as_ref().map(|s| s.modified.clone()),
        exists: stats.is_some(),
        previews,
        line_count,
    });
}

fn push_source_spec(project_dir: &Path, out: &mut Vec<DocumentEntry>) {
    // After the legacy ingest path was retired in favor of
    // `.sim-flow/spec-ingest/` (written by `sim-flow ingest`), the
    // dashboard's source-spec catalogue points at the new corpus's
    // manifest. The manifest summarises chunk / table / figure
    // counts and is the canonical entry point for inspecting what
    // was ingested.
    let dot_dir = project_dir.join(".sim-flow");
    if !dot_dir.exists() {
        return;
    }
    let manifest_abs = dot_dir.join("spec-ingest").join("manifest.toml");
    if let Some(stats) = stat_file(&manifest_abs) {
        out.push(DocumentEntry {
            abs_path: manifest_abs.to_string_lossy().into_owned(),
            rel_path: ".sim-flow/spec-ingest/manifest.toml".to_string(),
            category: DocumentCategory::SourceSpec,
            step: None,
            bytes: Some(stats.size),
            modified_at: Some(stats.modified),
            exists: true,
            previews: None,
            line_count: None,
        });
    }
}

struct FileStats {
    size: u64,
    modified: String,
}

fn stat_file(path: &Path) -> Option<FileStats> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let modified = md.modified().ok().and_then(format_system_time)?;
    Some(FileStats {
        size: md.len(),
        modified,
    })
}

fn format_system_time(t: std::time::SystemTime) -> Option<String> {
    let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    let secs = dur.as_secs() as i64;
    let nanos = dur.subsec_nanos();
    // Minimal ISO-8601 UTC formatter to avoid pulling chrono just
    // for this. Same shape `new Date(ms).toISOString()` produces.
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hours = (rem / 3600) as u32;
    let minutes = ((rem % 3600) / 60) as u32;
    let seconds = (rem % 60) as u32;
    let millis = nanos / 1_000_000;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
    ))
}

// Howard Hinnant's days-from-civil / civil-from-days. Public-domain
// algorithm; correct for years -32767..32767. Suffices here.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

fn walk_dir_shallow(dir: &Path, cap: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
                if out.len() >= cap {
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

fn has_code_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("rs"))
        .unwrap_or(false)
}

fn count_lines(path: &Path) -> Option<u64> {
    let body = std::fs::read_to_string(path).ok()?;
    if body.is_empty() {
        return Some(0);
    }
    let mut count: u64 = 1;
    for byte in body.bytes() {
        if byte == b'\n' {
            count = count.saturating_add(1);
        }
    }
    if body.ends_with('\n') {
        count = count.saturating_sub(1);
    }
    Some(count)
}

fn build_previews(rel: &str, abs: &Path, size_bytes: u64) -> Option<Vec<ArtifactPreview>> {
    if size_bytes == 0 {
        return None;
    }
    let rule = preview_rule_for(rel)?;
    let body = std::fs::read_to_string(abs).ok()?;
    match rule {
        PreviewRule::Markdown => {
            let truncated = if body.len() > PREVIEW_FULL_CAP_BYTES {
                let mut s = body[..PREVIEW_FULL_CAP_BYTES].to_string();
                s.push_str("\n\n_... (truncated for preview)_");
                s
            } else {
                body
            };
            Some(vec![ArtifactPreview::Markdown { body: truncated }])
        }
        PreviewRule::TableSection(section) => {
            let table = extract_table_under_heading(&body, section)?;
            Some(vec![ArtifactPreview::Table {
                caption: Some(section.to_string()),
                headers: table.0,
                rows: table.1,
            }])
        }
    }
}

fn extract_table_under_heading(
    body: &str,
    heading: &str,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let lines: Vec<&str> = body.lines().collect();
    // Locate the first `#{1,6} <heading>` (case-insensitive, with
    // optional trailing adornment like " (3 rows)").
    let mut i = 0usize;
    while i < lines.len() {
        if heading_matches(lines[i], heading) {
            break;
        }
        i += 1;
    }
    if i >= lines.len() {
        return None;
    }
    i += 1;
    while i < lines.len() {
        if is_heading(lines[i]) {
            return None;
        }
        if is_table_row(lines[i]) && i + 1 < lines.len() && is_table_separator(lines[i + 1]) {
            return Some(parse_table(&lines, i));
        }
        i += 1;
    }
    None
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return false;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes)
        && trimmed
            .chars()
            .nth(hashes)
            .is_some_and(|c| c.is_whitespace())
}

fn heading_matches(line: &str, heading: &str) -> bool {
    if !is_heading(line) {
        return false;
    }
    let trimmed = line.trim_start();
    let after_hash = trimmed.trim_start_matches('#').trim_start();
    after_hash
        .to_ascii_lowercase()
        .starts_with(&heading.to_ascii_lowercase())
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.ends_with('|')
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    if !t.starts_with('|') || !t.ends_with('|') {
        return false;
    }
    // Cells contain only `-`, `:`, `|`, whitespace.
    t.chars()
        .all(|c| c == '-' || c == ':' || c == '|' || c.is_whitespace())
}

fn split_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

fn parse_table(lines: &[&str], start: usize) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = split_row(lines[start]);
    let mut rows = Vec::new();
    let mut i = start + 2; // skip header + separator
    while i < lines.len() && is_table_row(lines[i]) {
        let mut cells = split_row(lines[i]);
        while cells.len() < headers.len() {
            cells.push(String::new());
        }
        cells.truncate(headers.len());
        rows.push(cells);
        i += 1;
    }
    (headers, rows)
}

/// Validate that `flow` is a known flow id. The TS layer used to
/// silently fall back to "no steps"; we return an explicit error
/// so a typo in the flow name lands as a clear CLI message.
pub fn validate_flow(flow: &str) -> Result<()> {
    if step_order(flow).is_empty() {
        return Err(Error::Config(format!(
            "unknown flow `{flow}`; expected `direct-modeling` or `design-study`"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn enumerate_emits_placeholders_for_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        // Every DM step has at least one expected artifact.
        let dm0_spec = docs
            .iter()
            .find(|d| d.rel_path == "docs/spec.md")
            .expect("DM0 spec row");
        assert!(!dm0_spec.exists);
        assert!(dm0_spec.bytes.is_none());
        assert_eq!(dm0_spec.step.as_deref(), Some("DM0"));
    }

    #[test]
    fn enumerate_reads_present_files_and_tags_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/spec.md"), "# Spec\nbody\n");
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let dm0_spec = docs
            .iter()
            .find(|d| d.rel_path == "docs/spec.md")
            .expect("DM0 spec row");
        assert!(dm0_spec.exists);
        assert!(dm0_spec.bytes.unwrap() > 0);
        assert!(dm0_spec.modified_at.is_some());
    }

    #[test]
    fn directory_artifacts_enumerate_children() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/impl-plan/milestone-01-foo.md"),
            "- [ ] task\n",
        );
        write(
            &tmp.path().join("docs/impl-plan/milestone-02-bar.md"),
            "- [x] done\n",
        );
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let impl_rows: Vec<_> = docs
            .iter()
            .filter(|d| d.rel_path.starts_with("docs/impl-plan/"))
            .collect();
        // Each child file appears under DM2c AND DM2cd (same dir
        // in both step rows), so 2 milestones × 2 steps = 4.
        assert!(impl_rows.len() >= 4);
        for row in &impl_rows {
            assert!(row.exists);
        }
    }

    #[test]
    fn rs_files_get_line_counts() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("src/main.rs"), "fn main() {}\n\n\n");
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let main_rs = docs
            .iter()
            .find(|d| d.rel_path.ends_with("main.rs"))
            .expect("main.rs row");
        assert_eq!(main_rs.line_count, Some(3));
    }

    #[test]
    fn markdown_preview_caps_at_8kb() {
        // docs/testbench.md has rule kind=markdown.
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(10_000);
        write(&tmp.path().join("docs/testbench.md"), &big);
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let tb = docs
            .iter()
            .find(|d| d.rel_path == "docs/testbench.md")
            .expect("testbench row");
        let previews = tb.previews.as_ref().expect("markdown preview");
        assert_eq!(previews.len(), 1);
        match &previews[0] {
            ArtifactPreview::Markdown { body } => {
                assert!(body.len() < 9_000);
                assert!(body.ends_with("_... (truncated for preview)_"));
            }
            other => panic!("expected markdown preview, got {other:?}"),
        }
    }

    #[test]
    fn table_section_preview_extracts_first_table_under_heading() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "# Targets\n\n## Target Summary\n\n| Name | Value |\n|------|-------|\n| t1 | 5 |\n| t2 | 10 |\n\n## Other\n";
        write(&tmp.path().join("docs/targets.md"), body);
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let targets = docs
            .iter()
            .find(|d| d.rel_path == "docs/targets.md")
            .expect("targets row");
        let previews = targets.previews.as_ref().expect("table preview");
        assert_eq!(previews.len(), 1);
        match &previews[0] {
            ArtifactPreview::Table {
                caption,
                headers,
                rows,
            } => {
                assert_eq!(caption.as_deref(), Some("Target Summary"));
                assert_eq!(headers, &vec!["Name".to_string(), "Value".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][0], "t1");
            }
            other => panic!("expected table preview, got {other:?}"),
        }
    }

    #[test]
    fn table_preview_none_when_heading_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/targets.md"), "# Targets\nfreeform\n");
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let targets = docs
            .iter()
            .find(|d| d.rel_path == "docs/targets.md")
            .expect("targets row");
        assert!(targets.previews.is_none());
    }

    #[test]
    fn critique_rows_emit_with_correct_step() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/critiques/DM0-critique.md"),
            "BLOCKER: x\n",
        );
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        let critique = docs
            .iter()
            .find(|d| d.rel_path == "docs/critiques/DM0-critique.md")
            .expect("critique row");
        assert_eq!(critique.category, DocumentCategory::Critique);
        assert_eq!(critique.step.as_deref(), Some("DM0"));
    }

    #[test]
    fn spec_ingest_manifest_surfaced_as_source_spec() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join(".sim-flow/spec-ingest/manifest.toml"),
            "schema_version = 1\nsource_kind = \"pdf\"\n",
        );
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        assert!(
            docs.iter()
                .any(|d| d.rel_path == ".sim-flow/spec-ingest/manifest.toml"
                    && d.category == DocumentCategory::SourceSpec),
            "manifest not surfaced as SourceSpec: {docs:?}"
        );
    }

    #[test]
    fn legacy_source_spec_files_no_longer_surfaced() {
        // Belt-and-braces: even if a project still has the legacy
        // tree on disk from a pre-deprecation run, the documents
        // catalogue points at the new manifest instead.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join(".sim-flow/source-spec-toc.md"), "# TOC\n");
        write(
            &tmp.path().join(".sim-flow/source-spec.md"),
            "# Spec\nbody\n",
        );
        let docs = enumerate_project_documents(tmp.path(), "direct-modeling");
        assert!(
            !docs
                .iter()
                .any(|d| d.rel_path.starts_with(".sim-flow/source-spec")),
            "legacy source-spec files still surfaced: {docs:?}"
        );
    }

    #[test]
    fn validate_flow_rejects_unknown() {
        assert!(validate_flow("direct-modeling").is_ok());
        assert!(validate_flow("design-study").is_ok());
        assert!(validate_flow("bogus").is_err());
    }
}
