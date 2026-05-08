//! Gate validation primitives.
//!
//! Each step's gate is a sequence of [`GateCheck`]s. The orchestrator
//! evaluates every check and collects failures so the user sees the full
//! list, not just the first blocker.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde::Serialize;

use crate::critique::Critique;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub enum GateCheck {
    /// The given path (relative to the project dir) must exist and be
    /// non-empty.
    FileExists { path: PathBuf, description: String },
    /// The given file must contain a match for the regex.
    FileMatches {
        path: PathBuf,
        pattern: String,
        description: String,
    },
    /// Run `cmd` in the project dir; success is exit 0.
    Shell {
        cmd: String,
        args: Vec<String>,
        description: String,
    },
    /// The critique file at `path` must not contain any `BLOCKER:`
    /// lines. `UNRESOLVED:` lines are informational and do not block.
    CritiqueClean { path: PathBuf, description: String },
    /// The experiments index at `.sim-flow/experiments.db` must contain
    /// at least one row in the `runs` table. Used by DM4 to confirm that
    /// tracking captured a simulation run before the analysis gate
    /// passes.
    ExperimentsRecorded { description: String },
    /// At least one of the listed paths must exist and be non-empty.
    /// Each path may be a file (matched directly) or a directory
    /// (treated as "any non-empty `*.md` inside, excluding
    /// `.gitkeep` / `README.md`"). Used to accept either of two
    /// canonical artifact layouts -- e.g. the generated spec can
    /// land as a single `docs/spec.md` (small designs) or as a
    /// directory of section files under `docs/spec/` (large
    /// designs paginated like the input spec).
    AnyExists {
        paths: Vec<PathBuf>,
        description: String,
    },
    /// Like `FileMatches`, but the pattern needs to match in at
    /// least ONE of the listed paths. Each path may be a file or
    /// a directory of `*.md` (same expansion rule as `AnyExists`).
    /// Used so DM0's frequency / tech-node regexes still pass when
    /// the spec is paginated and the matching string lives in a
    /// section file rather than the top-level `docs/spec.md`.
    AnyMatches {
        paths: Vec<PathBuf>,
        pattern: String,
        description: String,
    },
    /// Every milestone file under `dir` matching one of
    /// `<file_prefix>NN-*.md` (for any `<file_prefix>` in
    /// `file_prefixes`) must be resolved.
    ///
    /// Two resolution modes, matching `MilestoneWalkConfig`:
    /// - When `placeholder_marker` is `None` (execution-step gate
    ///   for DM2d / DM3b / DM3c / DM4b): every `- [ ]` row must be
    ///   resolved (`- [x]` or `- [-]`). Prevents the step from
    ///   advancing while any milestone still has pending rows.
    /// - When `placeholder_marker` is `Some(s)` (planning-detail
    ///   gate for DM2cd / DM3ad / DM4ad): no milestone body may
    ///   contain `s`. The detail step replaces the outline's stubs
    ///   with full task lists; this gate fails until every stub has
    ///   been replaced, ignoring `- [ ]` row counts (since the
    ///   detail step's whole purpose is to LAND `- [ ]` rows for
    ///   the downstream execution step to walk).
    MilestonesAllResolved {
        dir: PathBuf,
        file_prefixes: Vec<String>,
        placeholder_marker: Option<String>,
        description: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct GateFailure {
    pub description: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GateReport {
    pub failures: Vec<GateFailure>,
}

impl GateReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

pub fn evaluate(project_dir: &Path, checks: &[GateCheck]) -> Result<GateReport> {
    let mut report = GateReport::default();
    for check in checks {
        if let Some(failure) = evaluate_one(project_dir, check)? {
            report.failures.push(failure);
        }
    }
    Ok(report)
}

fn evaluate_one(project_dir: &Path, check: &GateCheck) -> Result<Option<GateFailure>> {
    match check {
        GateCheck::FileExists { path, description } => {
            let full = project_dir.join(path);
            if !full.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("file missing: {}", full.display()),
                }));
            }
            let meta = std::fs::metadata(&full).map_err(|source| Error::Io {
                path: full.clone(),
                source,
            })?;
            if meta.len() == 0 {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("file is empty: {}", full.display()),
                }));
            }
            Ok(None)
        }
        GateCheck::FileMatches {
            path,
            pattern,
            description,
        } => {
            let full = project_dir.join(path);
            let text = match std::fs::read_to_string(&full) {
                Ok(t) => t,
                Err(source) => {
                    return Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("cannot read {}: {}", full.display(), source),
                    }));
                }
            };
            let regex = Regex::new(pattern).map_err(|e| {
                Error::Gate(format!("invalid regex in gate check {pattern:?}: {e}"))
            })?;
            if !regex.is_match(&text) {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("pattern {pattern:?} not found in {}", full.display()),
                }));
            }
            Ok(None)
        }
        GateCheck::AnyExists { paths, description } => {
            let candidates = expand_candidate_files(project_dir, paths);
            for full in &candidates {
                if let Ok(meta) = std::fs::metadata(full)
                    && meta.is_file()
                    && meta.len() > 0
                {
                    return Ok(None);
                }
            }
            Ok(Some(GateFailure {
                description: description.clone(),
                reason: format!(
                    "no non-empty file found in any of: {}",
                    paths
                        .iter()
                        .map(|p| project_dir.join(p).display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }))
        }
        GateCheck::AnyMatches {
            paths,
            pattern,
            description,
        } => {
            let regex = Regex::new(pattern).map_err(|e| {
                Error::Gate(format!("invalid regex in gate check {pattern:?}: {e}"))
            })?;
            let candidates = expand_candidate_files(project_dir, paths);
            if candidates.is_empty() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "no candidate files exist for any of: {}",
                        paths
                            .iter()
                            .map(|p| project_dir.join(p).display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }));
            }
            for full in &candidates {
                if let Ok(text) = std::fs::read_to_string(full)
                    && regex.is_match(&text)
                {
                    return Ok(None);
                }
            }
            Ok(Some(GateFailure {
                description: description.clone(),
                reason: format!(
                    "pattern {pattern:?} not found in any of: {}",
                    candidates
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }))
        }
        GateCheck::Shell {
            cmd,
            args,
            description,
        } => {
            let output = Command::new(cmd)
                .args(args)
                .current_dir(project_dir)
                .output();
            match output {
                Ok(out) if out.status.success() => Ok(None),
                Ok(out) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "{} {} failed: exit {:?}: {}",
                        cmd,
                        args.join(" "),
                        out.status.code(),
                        String::from_utf8_lossy(&out.stderr).trim(),
                    ),
                })),
                Err(err) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("failed to spawn {cmd}: {err}"),
                })),
            }
        }
        GateCheck::ExperimentsRecorded { description } => {
            let db = project_dir.join(".sim-flow").join("experiments.db");
            if !db.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("experiments.db missing: {}", db.display()),
                }));
            }
            match crate::tracking::index::ExperimentIndex::open_path(&db) {
                Ok(index) => match index.count_runs() {
                    Ok(0) => Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: "experiments.db has no recorded runs".to_string(),
                    })),
                    Ok(_) => Ok(None),
                    Err(err) => Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("experiments.db query failed: {err}"),
                    })),
                },
                Err(err) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("cannot open experiments.db: {err}"),
                })),
            }
        }
        GateCheck::MilestonesAllResolved {
            dir,
            file_prefixes,
            placeholder_marker,
            description,
        } => {
            let full_dir = project_dir.join(dir);
            let entries = match std::fs::read_dir(&full_dir) {
                Ok(e) => e,
                Err(err) => {
                    return Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("milestone dir missing: {}: {err}", full_dir.display()),
                    }));
                }
            };
            let mut milestones: Vec<(String, PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.ends_with(".md") {
                    continue;
                }
                let Some(prefix) = file_prefixes.iter().find(|p| name.starts_with(p.as_str()))
                else {
                    continue;
                };
                let rest = &name[prefix.len()..];
                if !rest
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                {
                    continue;
                }
                milestones.push((name.to_string(), path));
            }
            if milestones.is_empty() {
                let prefixes_display = file_prefixes
                    .iter()
                    .map(|p| format!("`{p}NN-*.md`"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "no {} files found under `{}`",
                        prefixes_display,
                        full_dir.display()
                    ),
                }));
            }
            let mut pending: Vec<String> = Vec::new();
            for (name, path) in &milestones {
                let body = match std::fs::read_to_string(path) {
                    Ok(b) => b,
                    Err(err) => {
                        return Ok(Some(GateFailure {
                            description: description.clone(),
                            reason: format!("read {}: {err}", path.display()),
                        }));
                    }
                };
                match placeholder_marker {
                    Some(marker) => {
                        if body.contains(marker) {
                            pending.push(format!(
                                "  - `{name}`: still contains placeholder marker (stub not yet detailed)"
                            ));
                        }
                    }
                    None => {
                        let pending_count = body
                            .lines()
                            .filter(|line| line.trim_start().starts_with("- [ ]"))
                            .count();
                        if pending_count > 0 {
                            pending
                                .push(format!("  - `{name}`: {pending_count} unresolved row(s)"));
                        }
                    }
                }
            }
            if pending.is_empty() {
                Ok(None)
            } else {
                let label = if placeholder_marker.is_some() {
                    "milestone stubs not yet detailed:"
                } else {
                    "milestone files still have unresolved rows:"
                };
                Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("{label}\n{}", pending.join("\n")),
                }))
            }
        }
        GateCheck::CritiqueClean { path, description } => {
            let full = project_dir.join(path);
            if !full.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("critique missing: {}", full.display()),
                }));
            }
            let critique = Critique::load(&full)?;
            if critique.has_blocking() {
                let summary = critique
                    .blocking()
                    .into_iter()
                    .map(|f| format!("  - {}: {}", marker(f), f.text()))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("critique has blocking findings:\n{summary}"),
                }));
            }
            Ok(None)
        }
    }
}

fn marker(finding: &crate::critique::Finding) -> &'static str {
    match finding {
        crate::critique::Finding::Resolved(_) => "RESOLVED",
        crate::critique::Finding::Unresolved(_) => "UNRESOLVED",
        crate::critique::Finding::Blocker(_) => "BLOCKER",
    }
}

/// Expand a list of candidate paths (used by `AnyExists` /
/// `AnyMatches`) into the concrete files to inspect. File entries
/// are kept as-is; directory entries are walked one level deep and
/// every `*.md` inside is included EXCEPT scaffolding markers
/// (`.gitkeep`) and the auto-generated index files
/// (`README.md`, `_toc.md`) -- those don't carry the actual
/// content the gate cares about. Missing paths are silently
/// dropped; the caller surfaces a "no candidate files" failure
/// when the resulting list is empty.
fn expand_candidate_files(project_dir: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for rel in paths {
        let abs = project_dir.join(rel);
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue;
        };
        if meta.is_file() {
            out.push(abs);
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&abs) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }
            // Skip scaffolding + auto-generated index files. The
            // index summarizes section content; the actual
            // numbers / patterns the gate looks for live in the
            // section files themselves.
            if matches!(name, ".gitkeep" | "README.md" | "_toc.md" | "index.md") {
                continue;
            }
            if let Ok(file_meta) = path.metadata()
                && file_meta.is_file()
            {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_exists_fails_when_missing() {
        let dir = tempdir().unwrap();
        let report = evaluate(
            dir.path(),
            &[GateCheck::FileExists {
                path: PathBuf::from("spec.md"),
                description: "spec.md exists".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn file_exists_passes_when_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("spec.md"), "hello").unwrap();
        let report = evaluate(
            dir.path(),
            &[GateCheck::FileExists {
                path: PathBuf::from("spec.md"),
                description: "spec.md exists".into(),
            }],
        )
        .unwrap();
        assert!(report.is_clean());
    }

    #[test]
    fn file_matches_fails_when_pattern_absent() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("spec.md"), "no frequency here").unwrap();
        let report = evaluate(
            dir.path(),
            &[GateCheck::FileMatches {
                path: PathBuf::from("spec.md"),
                pattern: r"\d+\s*(MHz|GHz)".into(),
                description: "spec has frequency".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn critique_clean_fails_on_blocker() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("crit.md"),
            "- BLOCKER: missing test for X\n",
        )
        .unwrap();
        let report = evaluate(
            dir.path(),
            &[GateCheck::CritiqueClean {
                path: PathBuf::from("crit.md"),
                description: "critique clean".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn milestones_all_resolved_passes_when_every_row_is_checked() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir.join("tb-milestone-01.md"),
            "- [x] done\n- [-] deferred\n  - defer reason: skipped\n",
        );
        write(&dir.join("tb-milestone-02.md"), "- [x] done\n");
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/test-plan/"),
                file_prefixes: vec!["tb-milestone-".into()],
                placeholder_marker: None,
                description: "every tb-milestone resolved".into(),
            }],
        )
        .unwrap();
        assert!(report.is_clean(), "got failures: {:?}", report.failures);
    }

    #[test]
    fn milestones_all_resolved_fails_when_any_pending_row_remains() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("tb-milestone-01.md"), "- [x] done\n");
        write(
            &dir.join("tb-milestone-02.md"),
            "- [ ] still pending\n- [x] done\n",
        );
        write(&dir.join("tb-milestone-03.md"), "- [ ] also pending\n");
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/test-plan/"),
                file_prefixes: vec!["tb-milestone-".into()],
                placeholder_marker: None,
                description: "every tb-milestone resolved".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
        let reason = &report.failures[0].reason;
        assert!(reason.contains("tb-milestone-02"), "reason: {reason}");
        assert!(reason.contains("tb-milestone-03"), "reason: {reason}");
        assert!(!reason.contains("tb-milestone-01"));
    }

    #[test]
    fn milestones_all_resolved_fails_when_no_milestone_files_exist() {
        // Configuration error case: the planning step (DM3a) didn't
        // produce any milestone files, but the dir exists.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs/test-plan")).unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/test-plan/"),
                file_prefixes: vec!["tb-milestone-".into()],
                placeholder_marker: None,
                description: "every tb-milestone resolved".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].reason.contains("no `tb-milestone-NN-"));
    }

    #[test]
    fn milestones_all_resolved_isolates_to_one_file_prefix() {
        // Same dir holds DM3b's tb-milestone-* AND DM3c's
        // test-milestone-* files. The check must only inspect the
        // files matching its own prefix, so DM3b's gate doesn't
        // fail because DM3c hasn't started yet (or vice versa).
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("tb-milestone-01.md"), "- [x] done\n");
        write(&dir.join("test-milestone-01.md"), "- [ ] DM3c pending\n");
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/test-plan/"),
                file_prefixes: vec!["tb-milestone-".into()],
                placeholder_marker: None,
                description: "tb only".into(),
            }],
        )
        .unwrap();
        assert!(
            report.is_clean(),
            "DM3b's gate should NOT see DM3c's pending rows: {:?}",
            report.failures
        );
    }

    #[test]
    fn milestones_all_resolved_placeholder_mode_passes_when_no_marker_left() {
        // Detail-step gate: every stub has had its placeholder
        // marker removed (the agent wrote real task lists). Real
        // `- [ ]` rows in those task lists are FOR the downstream
        // execution step; the planning-detail gate must ignore
        // them.
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir.join("milestone-01-payloads.md"),
            "# Milestone 01\n\n## Tasks\n- [ ] real task here\n",
        );
        write(
            &dir.join("milestone-02-skeletons.md"),
            "# Milestone 02\n\n## Tasks\n- [ ] another task\n- [ ] and another\n",
        );
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/impl-plan/"),
                file_prefixes: vec!["milestone-".into()],
                placeholder_marker: Some("<!-- detail-pending -->".into()),
                description: "every stub detailed".into(),
            }],
        )
        .unwrap();
        assert!(
            report.is_clean(),
            "placeholder-mode gate should ignore `- [ ]` rows: {:?}",
            report.failures
        );
    }

    #[test]
    fn milestones_all_resolved_placeholder_mode_fails_when_any_stub_still_has_marker() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir.join("milestone-01-payloads.md"),
            "# Milestone 01\n\n## Tasks\n- [ ] real task\n",
        );
        write(
            &dir.join("milestone-02-skeletons.md"),
            "# Milestone 02\n\n## Tasks\n<!-- detail-pending -->\n",
        );
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/impl-plan/"),
                file_prefixes: vec!["milestone-".into()],
                placeholder_marker: Some("<!-- detail-pending -->".into()),
                description: "every stub detailed".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
        let reason = &report.failures[0].reason;
        assert!(reason.contains("milestone-02"), "reason: {reason}");
        assert!(reason.contains("placeholder"), "reason: {reason}");
        assert!(!reason.contains("milestone-01"));
    }

    #[test]
    fn milestones_all_resolved_walks_multiple_prefixes_in_one_check() {
        // DM3ad walks BOTH tb-milestone-* and test-milestone-*
        // files in `docs/test-plan/` -- one gate, two prefixes.
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir.join("tb-milestone-01-payloads.md"),
            "# tb-Milestone 01\n## Tasks\n- [ ] task\n",
        );
        write(
            &dir.join("test-milestone-01-smoke.md"),
            "# test-Milestone 01\n## Tasks\n<!-- detail-pending -->\n",
        );
        let report = evaluate(
            tmp.path(),
            &[GateCheck::MilestonesAllResolved {
                dir: PathBuf::from("docs/test-plan/"),
                file_prefixes: vec!["tb-milestone-".into(), "test-milestone-".into()],
                placeholder_marker: Some("<!-- detail-pending -->".into()),
                description: "all detailed".into(),
            }],
        )
        .unwrap();
        // Only test-milestone-01 has the marker -- the gate should
        // fail on it but pass tb-milestone-01.
        assert_eq!(report.failures.len(), 1);
        let reason = &report.failures[0].reason;
        assert!(reason.contains("test-milestone-01"), "reason: {reason}");
        assert!(!reason.contains("tb-milestone-01"));
    }

    #[test]
    fn any_exists_passes_for_single_file_layout() {
        // Legacy / small-spec layout: docs/spec.md is the spec.
        // The directory candidate is missing; the gate still
        // passes because the file candidate exists with content.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/spec.md"), "hi").unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyExists {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                description: "spec exists".into(),
            }],
        )
        .unwrap();
        assert!(report.is_clean(), "failures: {:?}", report.failures);
    }

    #[test]
    fn any_exists_passes_for_paginated_layout() {
        // New paginated layout: docs/spec.md absent, sections live
        // under docs/spec/. Gate passes via the directory branch.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
        std::fs::write(tmp.path().join("docs/spec/01-overview.md"), "x").unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyExists {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                description: "spec exists".into(),
            }],
        )
        .unwrap();
        assert!(report.is_clean(), "failures: {:?}", report.failures);
    }

    #[test]
    fn any_exists_fails_when_neither_form_present() {
        let tmp = tempdir().unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyExists {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                description: "spec exists".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn any_exists_skips_empty_files() {
        // Empty file fails the "non-empty" requirement; gate falls
        // through to the directory candidate, which is also empty
        // -- so the overall gate fails.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/spec.md"), "").unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyExists {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                description: "spec non-empty".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn any_matches_finds_pattern_in_paginated_section() {
        // Clock frequency lives in section 04, not in any
        // top-level docs/spec.md. The gate must scan section
        // files to satisfy the regex.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
        std::fs::write(
            tmp.path().join("docs/spec/01-overview.md"),
            "# Overview\nA pipeline.\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("docs/spec/04-timing.md"),
            "# Timing\nClock frequency: 1 GHz.\n",
        )
        .unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyMatches {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                pattern: r"\d+\s*(MHz|GHz)".into(),
                description: "spec has frequency".into(),
            }],
        )
        .unwrap();
        assert!(report.is_clean(), "failures: {:?}", report.failures);
    }

    #[test]
    fn any_matches_skips_index_files_when_scanning_directory() {
        // The auto-generated `README.md` index typically just
        // links to section files and shouldn't be a substitute
        // for the section content; the gate's expansion rule
        // excludes it. Pattern lives only in README.md here, so
        // the gate must fail.
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
        std::fs::write(
            tmp.path().join("docs/spec/README.md"),
            "# Spec\nClock: 1 GHz\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("docs/spec/01-overview.md"),
            "# Overview\nNo numbers here.\n",
        )
        .unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyMatches {
                paths: vec![PathBuf::from("docs/spec/")],
                pattern: r"\d+\s*GHz".into(),
                description: "spec has frequency".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn any_matches_fails_with_helpful_message_when_no_candidates() {
        let tmp = tempdir().unwrap();
        let report = evaluate(
            tmp.path(),
            &[GateCheck::AnyMatches {
                paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
                pattern: r"\d+\s*GHz".into(),
                description: "spec has frequency".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].reason.contains("no candidate files"));
    }
}
