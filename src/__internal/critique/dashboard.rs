//! Dashboard-shaped index helpers.
//!
//! The VS Code dashboard (and any other UI surface) consumes
//! critiques through `sim-flow critiques --json`. The shapes below
//! mirror the extension's `CritiqueFile` / `Finding` interfaces
//! verbatim (camelCase via `#[serde(rename_all = ...)]`) so the JSON
//! drops straight into the dashboard's existing renderers.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

use super::{CritiqueJson, FINDING_MARKER_RE, FindingKind, is_fence_delimiter};

const CRITIQUES_DIR: &str = "docs/critiques";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DashboardFindingKind {
    Resolved,
    Unresolved,
    Blocker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardFinding {
    pub kind: DashboardFindingKind,
    /// Text after the `<KIND>:` prefix, trimmed.
    pub text: String,
    /// 1-based line number in the source markdown (or synthetic
    /// index for JSON-sourced critiques where there's no real line).
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CritiqueDashboardEntry {
    /// Absolute path to the critique on disk. Prefers the markdown
    /// view when both forms exist so existing open-in-editor
    /// wiring still routes to the human-readable view.
    pub path: String,
    /// Step id (e.g. `"DM3a"`).
    pub step: String,
    /// Raw markdown body (or the JSON text when only JSON is on disk).
    pub body: String,
    /// Findings in source order.
    pub findings: Vec<DashboardFinding>,
    /// True when at least one finding is `unresolved` or `blocker`.
    pub has_blocking: bool,
}

fn finding_kind_for(kind: FindingKind) -> DashboardFindingKind {
    match kind {
        FindingKind::Blocker => DashboardFindingKind::Blocker,
        FindingKind::Unresolved => DashboardFindingKind::Unresolved,
        FindingKind::Resolved => DashboardFindingKind::Resolved,
    }
}

/// Parse the markdown body line-by-line and emit findings with
/// 1-based line numbers. Mirrors the per-line scan in
/// `Critique::parse`; kept separate so the existing API stays
/// line-oblivious (the gate / auto loop don't need lines).
fn parse_with_lines(text: &str) -> Vec<DashboardFinding> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, raw) in text.lines().enumerate() {
        // Skip fenced code blocks; see Critique::parse for rationale.
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
        let dkind = if kind.starts_with("blocker") {
            DashboardFindingKind::Blocker
        } else if kind.starts_with("unresolved") {
            DashboardFindingKind::Unresolved
        } else if kind.starts_with("resolved") {
            DashboardFindingKind::Resolved
        } else {
            continue;
        };
        out.push(DashboardFinding {
            kind: dkind,
            text: rest,
            line: (idx as u32).saturating_add(1),
        });
    }
    out
}

/// Build a dashboard-shaped entry from JSON text. The JSON has no
/// real line numbers; synthesize them as `idx + 1` so the
/// dashboard's per-finding ordering still works.
fn json_to_findings(parsed: &CritiqueJson) -> (Vec<DashboardFinding>, bool) {
    let mut findings = Vec::with_capacity(parsed.findings.len());
    let mut has_blocking = false;
    for (idx, f) in parsed.findings.iter().enumerate() {
        let kind = finding_kind_for(f.kind);
        let text = f.title.trim().to_string();
        findings.push(DashboardFinding {
            kind,
            text,
            line: (idx as u32).saturating_add(1),
        });
        if matches!(f.kind, FindingKind::Blocker | FindingKind::Unresolved) {
            has_blocking = true;
        }
    }
    (findings, has_blocking)
}

/// Read one step's critique into the dashboard-shaped form. Returns
/// `Ok(None)` when neither the JSON nor the markdown is on disk.
pub fn read_critique_entry(
    project_dir: &Path,
    step_id: &str,
) -> Result<Option<CritiqueDashboardEntry>> {
    let dir = project_dir.join(CRITIQUES_DIR);
    let json_path = dir.join(format!("{step_id}-critique.json"));
    let md_path = dir.join(format!("{step_id}-critique.md"));
    let json_text = read_if_exists(&json_path)?;
    let md_text = read_if_exists(&md_path)?;
    if let Some(text) = &json_text {
        match serde_json::from_str::<CritiqueJson>(text) {
            Ok(parsed) => {
                let (findings, has_blocking) = json_to_findings(&parsed);
                let surfaced_path = if md_text.is_some() {
                    md_path.to_string_lossy().into_owned()
                } else {
                    json_path.to_string_lossy().into_owned()
                };
                let body = md_text.clone().unwrap_or_else(|| text.clone());
                return Ok(Some(CritiqueDashboardEntry {
                    path: surfaced_path,
                    step: step_id.to_string(),
                    body,
                    findings,
                    has_blocking,
                }));
            }
            Err(err) => {
                // Match the gate's posture: the gate path
                // (Critique::load -> from_json) refuses to advance
                // on malformed JSON. Previously the dashboard fell
                // through to parse the markdown body silently,
                // which produced "Findings: 0, gate clean" while
                // the gate kept refusing to advance -- the user
                // couldn't see where the disagreement came from.
                // Now surface a synthetic Blocker finding so the
                // dashboard panel shows the actual parse error and
                // has_blocking aligns with the gate's refusal.
                // See orchestrator audit #18 (2026-05-16).
                let surfaced_path = json_path.to_string_lossy().into_owned();
                let body = md_text.clone().unwrap_or_else(|| text.clone());
                return Ok(Some(CritiqueDashboardEntry {
                    path: surfaced_path,
                    step: step_id.to_string(),
                    body,
                    findings: vec![DashboardFinding {
                        kind: DashboardFindingKind::Blocker,
                        text: format!("malformed critique JSON: {err}"),
                        line: 1,
                    }],
                    has_blocking: true,
                }));
            }
        }
    }
    if let Some(body) = md_text {
        let findings = parse_with_lines(&body);
        let has_blocking = findings.iter().any(|f| {
            matches!(
                f.kind,
                DashboardFindingKind::Blocker | DashboardFindingKind::Unresolved
            )
        });
        return Ok(Some(CritiqueDashboardEntry {
            path: md_path.to_string_lossy().into_owned(),
            step: step_id.to_string(),
            body,
            findings,
            has_blocking,
        }));
    }
    Ok(None)
}

/// Walk `docs/critiques/` and return one entry per step that has
/// either a `.json` or `.md` critique on disk. Sorted by step id
/// so the dashboard receives a stable order.
pub fn list_critique_entries(project_dir: &Path) -> Result<Vec<CritiqueDashboardEntry>> {
    let dir = project_dir.join(CRITIQUES_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(Error::Io { path: dir, source }),
    };
    let mut step_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if let Some(stem) = name.strip_suffix("-critique.json") {
            step_ids.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix("-critique.md") {
            step_ids.insert(stem.to_string());
        }
    }
    let mut out = Vec::with_capacity(step_ids.len());
    for step in step_ids {
        if let Some(entry) = read_critique_entry(project_dir, &step)? {
            out.push(entry);
        }
    }
    Ok(out)
}

fn read_if_exists(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
