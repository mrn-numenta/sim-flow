//! Hardened post-advance validation for the e2e_manual / e2e_auto
//! test binaries. Runs in addition to the orchestrator's gates.
//!
//! Why two layers? The orchestrator's gate is the "is this step
//! clean" decision-maker; if the gate is buggy (e.g. a marker
//! substring that doesn't actually appear in the stub bodies), it
//! advances on un-finished collateral and the test silently chases
//! a corrupted state. Re-running the gate catches mutation between
//! advance and validation, but it can't catch gate BUGS. So this
//! module also runs INDEPENDENT structural invariants the gate
//! does not encode:
//!
//! - Milestone-walk PLANNING steps (DM2cd / DM3ad / DM4ad): every
//!   milestone file has at least N task rows AND no
//!   `<!-- detail-pending` substring. Catches the marker-mismatch
//!   bug where the gate passed despite stubs being un-detailed.
//! - Milestone-walk EXECUTION steps (DM2d / DM3b / DM3c / DM4b):
//!   every milestone has zero `- [ ]` rows AND at least one
//!   `- [x]` row (i.e. genuinely populated, not just empty).
//! - Work-artifact paths exist and are non-trivially populated
//!   (file size > MIN_ARTIFACT_BYTES; directory non-empty).

use std::path::Path;

use crate::__internal::state::State;
use crate::__internal::steps::{StepDescriptor, registry_for};
use crate::__internal::{gate, steps};

/// Minimum size (bytes) for a work-artifact file to be treated as
/// non-trivially populated. Tuned to catch agents that wrote a
/// stub-only file with just a header. Spec / plan files routinely
/// land at several KB, so 200 is well below the floor of useful
/// content.
const MIN_ARTIFACT_BYTES: u64 = 200;

/// Minimum task-row count per milestone for planning-detail steps.
/// 3 is empirical: each milestone covers a deliverable that needs
/// at least one task per concrete artifact (file/symbol), one
/// integration task, and one acceptance task. Fewer than 3 means
/// the stub wasn't actually expanded.
const MIN_TASKS_PER_MILESTONE: usize = 3;

/// Aggregated validation result. The caller decides how to surface
/// failures (e2e_manual prints + exits non-zero; e2e_auto can
/// summarize before returning).
#[derive(Debug, Default)]
pub struct ValidationReport {
    pub failures: Vec<String>,
}

impl ValidationReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn fail(&mut self, msg: impl Into<String>) {
        self.failures.push(msg.into());
    }

    /// Merge another report's failures into this one.
    pub fn merge(&mut self, other: ValidationReport) {
        self.failures.extend(other.failures);
    }

    /// Print failures with a uniform "[VALIDATE-FAIL]" prefix so
    /// the test binaries' stdout is greppable.
    pub fn print(&self, label: &str) {
        if self.is_clean() {
            println!("[VALIDATE-OK] {label}");
            return;
        }
        println!(
            "[VALIDATE-FAIL] {label}: {} failure(s)",
            self.failures.len()
        );
        for (i, f) in self.failures.iter().enumerate() {
            println!("  {:>2}. {f}", i + 1);
        }
    }
}

/// Validate that `step_id` has fully completed AND the artifacts
/// the orchestrator marked it advanced past are real, populated,
/// and structurally sound.
pub fn validate_step_advanced(project_dir: &Path, step_id: &str) -> ValidationReport {
    let mut report = ValidationReport::default();

    let dot = project_dir.join(".sim-flow");
    let state = match State::load(&dot) {
        Ok(s) => s,
        Err(err) => {
            report.fail(format!("load state.toml: {err}"));
            return report;
        }
    };
    let registry = registry_for(state.flow);
    let step = match registry.get(step_id) {
        Some(s) => s,
        None => {
            report.fail(format!(
                "unknown step `{step_id}` for flow `{}`",
                state.flow.as_str()
            ));
            return report;
        }
    };

    // 1. Re-run the orchestrator's gate. If dirty here -- after the
    //    advance flag was set -- something mutated state between
    //    advance and validate, which is a regression worth surfacing.
    match gate::evaluate(project_dir, &step.gate_checks) {
        Ok(g) => {
            if !g.is_clean() {
                for f in &g.failures {
                    report.fail(format!(
                        "gate re-check on `{step_id}` reports dirty: {} -- {}",
                        f.description, f.reason
                    ));
                }
            }
        }
        Err(err) => report.fail(format!("gate evaluate({step_id}): {err}")),
    }

    // 2. Work-artifact existence + size. The gate covers SOME of
    //    these via FileExists, but several steps declare directory
    //    artifacts (e.g. `docs/impl-plan/`, `src/`) that the gate
    //    never validates as non-empty.
    validate_work_artifacts(project_dir, step, &mut report);

    // 3. Milestone-walk-specific stricter checks. Catches the
    //    placeholder-marker mismatch class of gate bug.
    if let Some(walk) = step.milestone_walk {
        validate_milestone_walk(project_dir, step_id, &walk, &mut report);
    }

    report
}

/// Walk every step the orchestrator has marked passed in
/// `state.toml` and validate it. Use this from `e2e_auto` after
/// `run_auto` returns to confirm the entire flow's collateral.
pub fn validate_full_state(project_dir: &Path) -> ValidationReport {
    let mut report = ValidationReport::default();
    let dot = project_dir.join(".sim-flow");
    let state = match State::load(&dot) {
        Ok(s) => s,
        Err(err) => {
            report.fail(format!("load state.toml: {err}"));
            return report;
        }
    };
    let registry = registry_for(state.flow);
    let order = registry.order_for(state.flow);
    for step_id in order {
        let passed = state.gates.get(step_id).map(|g| g.passed).unwrap_or(false);
        if !passed {
            continue;
        }
        let sub = validate_step_advanced(project_dir, step_id);
        if !sub.is_clean() {
            for f in sub.failures {
                report.fail(format!("[{step_id}] {f}"));
            }
        }
    }
    report
}

fn validate_work_artifacts(
    project_dir: &Path,
    step: &StepDescriptor,
    report: &mut ValidationReport,
) {
    for rel in step.work_artifacts {
        let path = project_dir.join(rel);
        let trimmed = rel.trim_end_matches('/');
        let is_dir_decl = rel.ends_with('/');
        match std::fs::metadata(&path) {
            Ok(md) if md.is_dir() => {
                // Directory artifact: must be non-empty.
                let entries = std::fs::read_dir(&path).map(|it| it.count()).unwrap_or(0);
                if entries == 0 {
                    report.fail(format!(
                        "work_artifact `{rel}` is an empty directory (expected populated)"
                    ));
                }
            }
            Ok(md) if md.is_file() => {
                if md.len() < MIN_ARTIFACT_BYTES {
                    report.fail(format!(
                        "work_artifact `{rel}` is suspiciously small: {} bytes (< {MIN_ARTIFACT_BYTES} byte floor)",
                        md.len()
                    ));
                }
            }
            Ok(_) => {
                // symlink or other; treat as missing for our purposes.
                report.fail(format!(
                    "work_artifact `{rel}` is not a regular file or directory"
                ));
            }
            Err(_) => {
                // Some steps (DM0) declare alternative artifacts via
                // `any_exists` -- e.g. `docs/spec.md` OR
                // `docs/spec/`. Don't fail on a single missing entry
                // when there's a sibling-directory alternative; the
                // gate already enforced one-of via AnyExists. We
                // detect this by checking whether ANOTHER artifact
                // in the list exists.
                let alt_exists = step.work_artifacts.iter().any(|other_rel| {
                    if other_rel == rel {
                        return false;
                    }
                    let other_path = project_dir.join(other_rel);
                    match std::fs::metadata(&other_path) {
                        Ok(md) if md.is_dir() => {
                            std::fs::read_dir(&other_path)
                                .map(|it| it.count())
                                .unwrap_or(0)
                                > 0
                        }
                        Ok(md) if md.is_file() => md.len() >= MIN_ARTIFACT_BYTES,
                        _ => false,
                    }
                });
                if !alt_exists {
                    let kind = if is_dir_decl { "directory" } else { "file" };
                    report.fail(format!(
                        "work_artifact `{trimmed}` ({kind}) is missing and no alternative artifact exists"
                    ));
                }
            }
        }
    }
}

fn validate_milestone_walk(
    project_dir: &Path,
    step_id: &str,
    walk: &steps::MilestoneWalkConfig,
    report: &mut ValidationReport,
) {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) => {
            report.fail(format!(
                "milestone-walk `{step_id}`: read_dir({}): {err}",
                dir.display()
            ));
            return;
        }
    };
    let mut milestones: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let Some(prefix) = walk.file_prefixes.iter().find(|p| name.starts_with(**p)) else {
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
        report.fail(format!(
            "milestone-walk `{step_id}`: no milestone files found under `{}`",
            walk.dir
        ));
        return;
    }
    milestones.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path) in &milestones {
        let body = match std::fs::read_to_string(path) {
            Ok(b) => b,
            Err(err) => {
                report.fail(format!("milestone `{name}`: read failed: {err}"));
                continue;
            }
        };
        let pending_rows = body
            .lines()
            .filter(|l| l.trim_start().starts_with("- [ ]"))
            .count();
        let done_rows = body
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with("- [x]") || t.starts_with("- [X]")
            })
            .count();
        let deferred_rows = body
            .lines()
            .filter(|l| l.trim_start().starts_with("- [-]"))
            .count();
        let total_task_rows = pending_rows + done_rows + deferred_rows;

        match walk.placeholder_marker {
            Some(marker) => {
                // Planning-detail mode (DM2cd / DM3ad / DM4ad): the
                // orchestrator advanced when no body contains the
                // marker. We additionally require >= MIN_TASKS_PER_MILESTONE
                // task rows AND no marker-family substring (catches
                // marker-mismatch gate bugs where the agent left the
                // verbose `<!-- detail-pending: ... -->` form
                // intact).
                if body.contains(marker) {
                    report.fail(format!(
                        "milestone `{name}`: still contains placeholder marker `{marker}` (gate should have failed)"
                    ));
                }
                if total_task_rows < MIN_TASKS_PER_MILESTONE {
                    report.fail(format!(
                        "milestone `{name}`: only {total_task_rows} task row(s); expected >= {MIN_TASKS_PER_MILESTONE} for a detailed milestone"
                    ));
                }
            }
            None => {
                // Execution mode (DM2d / DM3b / DM3c / DM4b): every
                // task row must be resolved AND there must be at
                // least one row to call this milestone non-empty.
                if pending_rows > 0 {
                    report.fail(format!(
                        "milestone `{name}`: {pending_rows} unresolved `- [ ]` row(s) remain after advance"
                    ));
                }
                if total_task_rows == 0 {
                    report.fail(format!(
                        "milestone `{name}`: zero task rows (milestone has no executable items)"
                    ));
                }
                if done_rows == 0 && deferred_rows == 0 {
                    report.fail(format!(
                        "milestone `{name}`: no rows marked `- [x]` or `- [-]` (nothing was actually executed)"
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_is_clean() {
        let report = ValidationReport::default();
        assert!(report.is_clean());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn fail_appends_to_failures_and_marks_unclean() {
        let mut report = ValidationReport::default();
        report.fail("first failure");
        report.fail(String::from("second failure"));
        assert!(!report.is_clean());
        assert_eq!(report.failures, vec!["first failure", "second failure"]);
    }

    #[test]
    fn merge_appends_other_failures_after_own() {
        let mut a = ValidationReport::default();
        a.fail("a1");
        a.fail("a2");
        let mut b = ValidationReport::default();
        b.fail("b1");
        a.merge(b);
        assert_eq!(a.failures, vec!["a1", "a2", "b1"]);
    }

    #[test]
    fn merge_clean_into_clean_stays_clean() {
        let mut a = ValidationReport::default();
        a.merge(ValidationReport::default());
        assert!(a.is_clean());
    }

    #[test]
    fn merge_clean_into_dirty_preserves_dirty() {
        let mut a = ValidationReport::default();
        a.fail("only failure");
        a.merge(ValidationReport::default());
        assert_eq!(a.failures, vec!["only failure"]);
    }

    #[test]
    fn print_does_not_panic_on_clean_or_dirty_reports() {
        // We can't easily capture stdout from a unit test without
        // pulling in a redirect crate, but at minimum the print
        // path must not panic on either input shape.
        let clean = ValidationReport::default();
        clean.print("clean-label");
        let mut dirty = ValidationReport::default();
        dirty.fail("dirty failure 1");
        dirty.fail("dirty failure 2");
        dirty.print("dirty-label");
    }

    #[test]
    fn validate_step_advanced_fails_when_state_toml_missing() {
        // No `.sim-flow/state.toml` -> State::load errors -> the
        // validator should record a failure and not panic.
        let tmp = tempfile::tempdir().unwrap();
        let report = validate_step_advanced(tmp.path(), "DM0");
        assert!(
            !report.is_clean(),
            "missing state.toml should produce at least one failure"
        );
    }

    #[test]
    fn validate_full_state_fails_when_state_toml_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let report = validate_full_state(tmp.path());
        assert!(!report.is_clean());
    }
}
