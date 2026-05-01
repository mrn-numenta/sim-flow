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
}
