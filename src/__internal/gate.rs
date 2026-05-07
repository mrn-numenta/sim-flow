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
    /// Every milestone file under `dir` matching `<file_prefix>NN-*.md`
    /// must have all `- [ ]` rows resolved (`- [x]` done OR `- [-]`
    /// deferred). Used by DM2d / DM3b / DM3c / DM4b to enforce that
    /// the step's gate only passes after EVERY milestone has been
    /// walked through, not just the first one. Pairs with
    /// `StepDescriptor::milestone_walk`: the orchestrator scopes
    /// each work / critique session to one milestone at a time, and
    /// this check is what prevents the step from advancing while
    /// other milestone files still hold `- [ ]` rows.
    MilestonesAllResolved {
        dir: PathBuf,
        file_prefix: String,
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
            file_prefix,
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
                if !name.starts_with(file_prefix.as_str()) || !name.ends_with(".md") {
                    continue;
                }
                let rest = &name[file_prefix.len()..];
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
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "no `{}NN-*.md` files found under `{}`",
                        file_prefix,
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
                let pending_count = body
                    .lines()
                    .filter(|line| line.trim_start().starts_with("- [ ]"))
                    .count();
                if pending_count > 0 {
                    pending.push(format!("  - `{name}`: {pending_count} unresolved row(s)"));
                }
            }
            if pending.is_empty() {
                Ok(None)
            } else {
                Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "milestone files still have unresolved rows:\n{}",
                        pending.join("\n")
                    ),
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
                file_prefix: "tb-milestone-".into(),
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
                file_prefix: "tb-milestone-".into(),
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
                file_prefix: "tb-milestone-".into(),
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
                file_prefix: "tb-milestone-".into(),
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
}
