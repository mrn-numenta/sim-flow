//! Per-step git commit on advance.
//!
//! Sim-flow projects accumulate artifacts step by step (`spec.md` at
//! DM0, `targets.md` + `testbench.md` at DM1, etc.). Once a step's
//! gate passes we commit those artifacts so the project history
//! mirrors the flow's progression. Subsequent steps can then be
//! reasoned about (and rolled back) at the granularity of "what
//! DM2d added on top of DM2c" rather than as one undifferentiated
//! diff.
//!
//! Behavior is best-effort: a project that isn't a git work tree, or
//! a turn that produced no diff, or a `git commit` that fails for
//! any reason (no identity configured, pre-commit hook, etc.)
//! returns a non-`Committed` outcome and the caller continues with
//! the advance. We never let a git problem block flow progress --
//! the gate already passed.

use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum CommitOutcome {
    /// `git commit` succeeded and produced a new HEAD.
    Committed,
    /// `project_dir` is not inside any git work tree; nothing to do.
    NotARepo,
    /// `git add -A .` staged nothing (the gate passed but no files
    /// changed since the previous commit -- e.g. an idempotent
    /// re-advance). The flow still progresses; just no commit.
    NothingToCommit,
    /// `git` was not found, or `add` / `commit` exited non-zero.
    /// The trailing string is a one-line summary of stderr.
    Failed(String),
}

/// Commit the staged changes inside `project_dir` with a message
/// naming `step_id` (and `next_step` when advancing to a successor).
/// Stages only paths under `project_dir` itself (not the entire
/// outer repo), since users typically run sim-flow inside the
/// `sim-models` repo and we shouldn't sweep up unrelated edits.
pub fn commit_step_advance(
    project_dir: &Path,
    step_id: &str,
    next_step: Option<&str>,
) -> CommitOutcome {
    if !is_inside_work_tree(project_dir) {
        return CommitOutcome::NotARepo;
    }
    if let Err(detail) = run_git(project_dir, &["add", "-A", "."]) {
        return CommitOutcome::Failed(format!("git add: {detail}"));
    }
    if !has_staged_changes(project_dir) {
        return CommitOutcome::NothingToCommit;
    }
    let message = match next_step {
        Some(next) => format!("sim-flow: advance past {step_id} -> {next}"),
        None => format!("sim-flow: advance past {step_id} (final step)"),
    };
    if let Err(detail) = run_git(project_dir, &["commit", "-m", &message]) {
        return CommitOutcome::Failed(format!("git commit: {detail}"));
    }
    CommitOutcome::Committed
}

/// Render a `CommitOutcome` for human-readable contexts (CLI stderr,
/// dashboard diagnostics). Returns `None` for `NothingToCommit` and
/// `NotARepo` so quiet success paths don't spam the user; both are
/// expected on rerun-after-rollback and on non-git projects.
pub fn outcome_message(outcome: &CommitOutcome) -> Option<String> {
    match outcome {
        CommitOutcome::Committed => Some("git: committed step artifacts".into()),
        CommitOutcome::NotARepo => None,
        CommitOutcome::NothingToCommit => None,
        CommitOutcome::Failed(detail) => Some(format!(
            "git: commit on advance failed (continuing): {detail}"
        )),
    }
}

fn is_inside_work_tree(project_dir: &Path) -> bool {
    matches!(
        Command::new("git")
            .arg("-C")
            .arg(project_dir)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output(),
        Ok(out) if out.status.success() && out.stdout.starts_with(b"true"),
    )
}

fn has_staged_changes(project_dir: &Path) -> bool {
    // `git diff --cached --quiet` exits 0 when there are NO staged
    // changes, 1 when there are, and >1 on error. Treat anything but
    // exit-0 as "yes, there's something to commit".
    match Command::new("git")
        .arg("-C")
        .arg(project_dir)
        .args(["diff", "--cached", "--quiet"])
        .status()
    {
        Ok(status) => !status.success(),
        Err(_) => false,
    }
}

fn run_git(project_dir: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_dir)
        .args(args)
        .output()
        .map_err(|err| format!("spawn `git {}` failed: {err}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed = stderr.lines().next().unwrap_or("").trim();
    Err(if trimmed.is_empty() {
        format!(
            "`git {}` exited {}",
            args.join(" "),
            output.status.code().unwrap_or(-1)
        )
    } else {
        trimmed.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(cmd: &mut Command) -> bool {
        cmd.output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn init_repo(dir: &Path) {
        assert!(run(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["init", "-q"])));
        assert!(run(Command::new("git").arg("-C").arg(dir).args([
            "config",
            "user.email",
            "test@example.com",
        ])));
        assert!(run(Command::new("git").arg("-C").arg(dir).args([
            "config",
            "user.name",
            "Test",
        ])));
        // Initial commit so HEAD exists.
        fs::write(dir.join(".gitkeep"), "").unwrap();
        assert!(run(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["add", "."])));
        assert!(run(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "-m", "init",])));
    }

    #[test]
    fn not_a_repo_returns_not_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = commit_step_advance(tmp.path(), "DM0", Some("DM1"));
        assert!(matches!(outcome, CommitOutcome::NotARepo), "{outcome:?}");
    }

    #[test]
    fn no_changes_returns_nothing_to_commit() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let outcome = commit_step_advance(tmp.path(), "DM0", Some("DM1"));
        assert!(
            matches!(outcome, CommitOutcome::NothingToCommit),
            "{outcome:?}",
        );
    }

    #[test]
    fn commits_step_artifacts_with_message() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("spec.md"), "# Spec\n").unwrap();
        let outcome = commit_step_advance(tmp.path(), "DM0", Some("DM1"));
        assert!(matches!(outcome, CommitOutcome::Committed), "{outcome:?}");
        // Inspect HEAD message.
        let log = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(
            subject.trim() == "sim-flow: advance past DM0 -> DM1",
            "unexpected subject: {subject:?}",
        );
    }

    #[test]
    fn commits_final_step_with_final_marker() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("DONE.md"), "done").unwrap();
        let outcome = commit_step_advance(tmp.path(), "DM4", None);
        assert!(matches!(outcome, CommitOutcome::Committed), "{outcome:?}");
        let log = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(
            subject.trim() == "sim-flow: advance past DM4 (final step)",
            "unexpected subject: {subject:?}",
        );
    }
}
