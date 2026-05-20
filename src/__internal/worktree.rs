//! Per-worker git worktrees for the parallel plan-detail walk.
//!
//! Phase A of the parallel-execution rollout (see
//! `docs/brainstorming/parallel-llm-execution.md`) gives each
//! milestone worker its own filesystem so concurrent Work + Critique
//! sessions can't race on the shared per-step critique JSON path or
//! anything else under the project tree. The dispatcher creates one
//! worktree per pending stub off the current HEAD, runs the worker
//! session with its `project_dir` overridden to point at the
//! worktree, then merges the milestone file + aggregates the
//! critique JSONs back into the main project.
//!
//! Worktrees use detached HEAD (`git worktree add --detach`) because
//! workers don't commit -- they just write files the coordinator
//! later copies out. No temp branches to clean up; only the worktree
//! directory itself.
//!
//! Lifecycle:
//! 1. `WorktreeManager::create_for_step` per parallel walk: probes
//!    that `git` works and the project is a work tree.
//! 2. `manager.checkout(name)` per worker: creates a detached
//!    worktree at a unique path under the system temp dir.
//! 3. Worker session runs against the worktree's path.
//! 4. Coordinator reads worker artifacts; copies milestone file +
//!    aggregates critique JSON back to main.
//! 5. `worktree.cleanup()` (or `Drop`) removes the worktree.
//! 6. Best-effort `git worktree prune` on next startup catches
//!    orphans from crashed runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// Coordinator-side worktree manager. Holds the main project's
/// path and an output directory under which per-worker worktrees
/// live. Created once per parallel walk.
#[derive(Debug)]
pub struct WorktreeManager {
    main_project_dir: PathBuf,
    /// Where worker worktrees get created. Typically
    /// `<TMPDIR>/sim-flow-worktrees/<pid>-<step>/`. Kept distinct
    /// from `main_project_dir` so the worktrees don't accidentally
    /// land inside a gitignored corner of the parent repo and
    /// confuse future `git worktree list` output.
    output_root: PathBuf,
}

/// A live worktree. The `path` is the per-worker working tree the
/// orchestrator hands to `OrchestratorOptions::project_dir`. On
/// drop, `cleanup()` is invoked best-effort -- explicit cleanup is
/// preferred so errors surface.
#[derive(Debug)]
pub struct Worktree {
    path: PathBuf,
    main_project_dir: PathBuf,
    /// Whether `cleanup()` has already run. Drop is a no-op when
    /// true so we don't double-remove.
    cleaned: bool,
}

impl WorktreeManager {
    /// Create a manager for the parallel walk. Returns `Err` if the
    /// project isn't inside a git work tree or `git` isn't on PATH
    /// -- callers translate that into a fall-back to the in-place
    /// (V1) parallel path. The `step_id` is just a label baked into
    /// the output_root path so concurrent walks of different steps
    /// can't collide.
    pub fn create_for_step(main_project_dir: &Path, step_id: &str) -> Result<Self> {
        if !is_inside_work_tree(main_project_dir) {
            return Err(Error::State(format!(
                "{} is not inside a git work tree; cannot create per-milestone worktrees",
                main_project_dir.display()
            )));
        }
        // Canonicalize both paths so the symlinks macOS puts in
        // front of /var (and equivalents on other platforms) don't
        // confuse `git worktree add`: the worktree's `.git` pointer
        // file gets written with the resolved gitdir, and any
        // later `git -C <main>` call has to match against the same
        // resolved form. Without canonicalization, `git worktree
        // remove` fails with "not a git repository" on macOS temp
        // dirs.
        let main = std::fs::canonicalize(main_project_dir).map_err(|source| Error::Io {
            path: main_project_dir.to_path_buf(),
            source,
        })?;
        let temp_root = std::env::temp_dir();
        let temp_root = std::fs::canonicalize(&temp_root).unwrap_or(temp_root);
        // Pid + monotonic-ish counter makes the output_root unique
        // both across processes and across concurrent in-process
        // callers (e.g. tests running in parallel). Without the
        // counter, two parallel walks on the same step would share
        // an output_root and step on each other's worker dirs.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let output_root = temp_root.join(format!(
            "sim-flow-worktrees-{}-{}-{seq}",
            std::process::id(),
            step_id
        ));
        std::fs::create_dir_all(&output_root).map_err(|source| Error::Io {
            path: output_root.clone(),
            source,
        })?;
        Ok(Self {
            main_project_dir: main,
            output_root,
        })
    }

    /// Check out a fresh worktree off the main project's current
    /// HEAD at `<output_root>/<name>`. The `name` should be a
    /// path-safe identifier unique to this worker (e.g. the
    /// milestone filename without its extension).
    pub fn checkout(&self, name: &str) -> Result<Worktree> {
        let path = self.output_root.join(name);
        // `git worktree add --detach <path> HEAD`: creates a new
        // working tree at <path> with detached HEAD pointing at
        // the same commit as main. `--detach` avoids creating a
        // temp branch we'd have to clean up.
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.main_project_dir)
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg("HEAD")
            .output()
            .map_err(|source| Error::Io {
                path: self.main_project_dir.clone(),
                source,
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::State(format!(
                "git worktree add failed: {}",
                stderr.trim()
            )));
        }
        // Propagate `.sim-flow/state.toml` and `.sim-flow/config.toml`
        // into the worktree so the orchestrator's per-session reads
        // see the same project state as main. These files are
        // gitignored in many sim-flow projects so `worktree add`
        // doesn't carry them over. Other `.sim-flow/` files (logs,
        // critiques) are per-worker by design.
        propagate_sim_flow_state(&self.main_project_dir, &path)?;
        Ok(Worktree {
            path,
            main_project_dir: self.main_project_dir.clone(),
            cleaned: false,
        })
    }

    /// Remove the output_root directory after all worktrees have
    /// been cleaned up. Best-effort: a leftover directory is
    /// harmless (next run picks a different `<pid>` suffix).
    pub fn cleanup_root(&self) {
        let _ = std::fs::remove_dir_all(&self.output_root);
    }
}

impl Drop for WorktreeManager {
    fn drop(&mut self) {
        self.cleanup_root();
    }
}

impl Worktree {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Explicitly remove the worktree. Prefer this over relying on
    /// `Drop` so errors propagate. `git worktree remove --force`
    /// handles the case where workers crashed mid-write.
    pub fn cleanup(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        self.cleaned = true;
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.main_project_dir)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
        // Fall back to manual rm if `git worktree remove` reports
        // failure (the worktree might never have been registered
        // if `add` partial-failed). Either way the working tree
        // shouldn't survive this function.
        match output {
            Ok(out) if out.status.success() => Ok(()),
            _ => {
                let _ = std::fs::remove_dir_all(&self.path);
                Ok(())
            }
        }
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Probe whether `dir` is inside a git work tree. Returns false if
/// `git` isn't installed or the directory isn't a repo -- callers
/// fall back to the in-place (V1) parallel path.
pub fn is_inside_work_tree(dir: &Path) -> bool {
    matches!(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output(),
        Ok(out) if out.status.success() && out.stdout.starts_with(b"true"),
    )
}

/// Best-effort `git worktree prune` to clear records of worktrees
/// whose directories were already removed (typically by a crashed
/// prior run that lost the `Drop` cleanup). Errors are swallowed --
/// a stale record doesn't block new worktree creation; it just
/// shows in `git worktree list`.
pub fn prune_orphans(main_project_dir: &Path) {
    let _ = Command::new("git")
        .arg("-C")
        .arg(main_project_dir)
        .args(["worktree", "prune"])
        .output();
}

/// A worktree's contribution to the parallel walk's outcome. The
/// coordinator builds one of these per worker after the worker's
/// session has ended; aggregation then folds the contributions into
/// a single main-tree critique JSON and copies the milestone file
/// back into the main project.
///
/// `Clone` so the dispatcher can hand a copy to
/// [`merge_contributions`] while still holding the original (paired
/// with the live `Worktree`) for cleanup at scope exit.
#[derive(Debug, Clone)]
pub struct WorktreeContribution {
    /// The worker's worktree path (whence we read).
    pub worktree_path: PathBuf,
    /// The milestone name this worker was scoped to (used as a
    /// section prefix in the aggregated critique so humans can see
    /// which finding belongs to which milestone).
    pub milestone_name: String,
    /// Project-relative path of the milestone file the worker wrote
    /// (e.g. `"docs/impl-plan/milestone-03-decode.md"`). Coordinator
    /// copies the worker's version of this file back to the main
    /// project tree.
    pub milestone_rel_path: String,
}

/// Merge per-worker contributions back into the main project tree.
///
/// For each contribution:
///   - Copy the worker's milestone file to the main project tree at
///     the same project-relative path.
///   - Copy the worker's `docs/critiques/<step>-critique.json` (if
///     present) to a per-milestone shard at
///     `docs/critiques/<step>/<milestone_name>.json` in main,
///     and re-render the markdown sibling alongside.
///
/// No aggregation: each milestone's critique stays separate so the
/// gate's directory-aware `CritiqueClean` evaluator can surface
/// every blocker from every shard in one pass. The legacy
/// `docs/critiques/<step>-critique.json` is left alone (the parallel
/// walk's entry point already removes it on re-entry to avoid stale
/// findings).
///
/// Returns the number of shards written -- the caller logs this
/// alongside the parallel-walk Diagnostic.
pub fn merge_contributions(
    main_project_dir: &Path,
    step_id: &str,
    contributions: &[WorktreeContribution],
) -> Result<usize> {
    let shard_dir = main_project_dir.join("docs/critiques").join(step_id);
    let mut shards_written: usize = 0;

    for contrib in contributions {
        // Copy milestone file back to main.
        let src = contrib.worktree_path.join(&contrib.milestone_rel_path);
        let dst = main_project_dir.join(&contrib.milestone_rel_path);
        if src.exists() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            std::fs::copy(&src, &dst).map_err(|source| Error::Io {
                path: dst.clone(),
                source,
            })?;
        }

        // Copy worker's critique JSON to a per-milestone shard. A
        // worker that ended without writing one (e.g. crashed
        // mid-Critique) leaves no shard -- the directory-mode gate
        // sees the empty slot and the run flips to manual the same
        // way it would for any other missing critique.
        let worker_json = contrib
            .worktree_path
            .join("docs/critiques")
            .join(format!("{step_id}-critique.json"));
        if !worker_json.exists() {
            continue;
        }
        let body = std::fs::read_to_string(&worker_json).map_err(|source| Error::Io {
            path: worker_json.clone(),
            source,
        })?;
        // Parse just to validate -- a malformed worker JSON should
        // surface as a hard error rather than silently writing
        // garbage into the shard.
        let _ = serde_json::from_str::<crate::critique::CritiqueJson>(&body).map_err(|err| {
            Error::State(format!(
                "worker critique JSON malformed at {}: {err}",
                worker_json.display()
            ))
        })?;
        std::fs::create_dir_all(&shard_dir).map_err(|source| Error::Io {
            path: shard_dir.clone(),
            source,
        })?;
        let shard_filename = sanitize_shard_filename(&contrib.milestone_name);
        let shard_json = shard_dir.join(format!("{shard_filename}.json"));
        std::fs::write(&shard_json, body).map_err(|source| Error::Io {
            path: shard_json.clone(),
            source,
        })?;
        // Re-render the markdown sibling. Path is computed
        // relative to the main project dir for the renderer's
        // `read_to_string` step.
        let json_rel = format!("docs/critiques/{step_id}/{shard_filename}.json");
        crate::critique::render_critique_markdown_to_disk(main_project_dir, &json_rel)?;
        shards_written += 1;
    }

    Ok(shards_written)
}

/// Strip path separators / leading dots / control chars from a
/// milestone name so it can be safely used as a filename. The names
/// we receive (e.g. `"milestone-03-decode.md"`) are already
/// well-formed; this is defensive against accidental path-traversal
/// content. Trailing `.md` is stripped since the shard adds `.json`
/// itself.
fn sanitize_shard_filename(name: &str) -> String {
    let trimmed = name.trim_end_matches(".md");
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '/' | '\\' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    if out.starts_with('.') {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

fn propagate_sim_flow_state(main: &Path, worktree: &Path) -> Result<()> {
    let main_dot = main.join(".sim-flow");
    let worktree_dot = worktree.join(".sim-flow");
    if !main_dot.exists() {
        // No project state to propagate (unusual for sim-flow but
        // not fatal -- the worker session would error on its own
        // state.toml load if it really needed one).
        return Ok(());
    }
    std::fs::create_dir_all(&worktree_dot).map_err(|source| Error::Io {
        path: worktree_dot.clone(),
        source,
    })?;
    for name in &["state.toml", "config.toml"] {
        let src = main_dot.join(name);
        if !src.exists() {
            continue;
        }
        let dst = worktree_dot.join(name);
        std::fs::copy(&src, &dst).map_err(|source| Error::Io { path: dst, source })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_git_repo(dir: &Path) {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "Test"])
            .status()
            .unwrap();
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();
    }

    #[test]
    fn is_inside_work_tree_detects_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());
        assert!(is_inside_work_tree(tmp.path()));
    }

    #[test]
    fn is_inside_work_tree_rejects_a_plain_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_inside_work_tree(tmp.path()));
    }

    #[test]
    fn create_for_step_fails_for_non_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let err = WorktreeManager::create_for_step(tmp.path(), "DM2cd").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not inside a git work tree"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn checkout_creates_a_detached_worktree_with_main_files() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        init_git_repo(&tmp_path);
        std::fs::create_dir_all(tmp_path.join(".sim-flow")).unwrap();
        std::fs::write(
            tmp_path.join(".sim-flow/state.toml"),
            "current_step = \"DM2cd\"\n",
        )
        .unwrap();
        std::fs::write(tmp_path.join(".sim-flow/config.toml"), "").unwrap();
        let mgr = WorktreeManager::create_for_step(&tmp_path, "DM2cd").unwrap();
        let wt = mgr.checkout("worker-01").unwrap();
        // Main repo's tracked files are present.
        assert!(wt.path().join("README.md").exists());
        // .sim-flow state was propagated.
        assert!(wt.path().join(".sim-flow/state.toml").exists());
        assert!(wt.path().join(".sim-flow/config.toml").exists());
        // Detached HEAD: HEAD file points to a SHA, not a ref.
        let head = std::fs::read_to_string(wt.path().join(".git")).unwrap();
        // `.git` in a worktree is a file pointing at the parent's
        // gitdir; checking the file exists is enough -- the
        // detachedness lives in HEAD inside that gitdir.
        assert!(head.starts_with("gitdir:"));
    }

    #[test]
    fn cleanup_removes_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        init_git_repo(&tmp_path);
        let mgr = WorktreeManager::create_for_step(&tmp_path, "DM2cd-cleanup").unwrap();
        let mut wt = mgr.checkout("worker-01").unwrap();
        let path = wt.path().to_path_buf();
        assert!(path.exists());
        wt.cleanup().unwrap();
        assert!(!path.exists(), "worktree directory should be gone");
    }

    #[test]
    fn drop_cleans_up_best_effort() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        init_git_repo(&tmp_path);
        let mgr = WorktreeManager::create_for_step(&tmp_path, "DM2cd-drop").unwrap();
        let path = {
            let wt = mgr.checkout("worker-drop").unwrap();
            let p = wt.path().to_path_buf();
            assert!(p.exists());
            p
            // wt drops here
        };
        assert!(
            !path.exists(),
            "Drop impl should have removed the worktree directory"
        );
    }

    fn write_critique_json(worktree: &Path, step: &str, body: &str) {
        let dir = worktree.join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{step}-critique.json")), body).unwrap();
    }

    fn write_milestone(worktree: &Path, rel: &str, body: &str) {
        let path = worktree.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn merge_contributions_aggregates_findings_and_copies_milestones() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt_a = tmp.path().join("wt-a");
        let wt_b = tmp.path().join("wt-b");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&wt_a).unwrap();
        std::fs::create_dir_all(&wt_b).unwrap();

        // Worker A: clean critique, milestone-01 detailed.
        write_milestone(
            &wt_a,
            "docs/impl-plan/milestone-01-foo.md",
            "# 01 detailed\n",
        );
        write_critique_json(
            &wt_a,
            "DM2cd",
            r#"{
              "step": "DM2cd",
              "summary": "milestone-01 clean",
              "findings": [],
              "notes": ""
            }"#,
        );

        // Worker B: BLOCKER, milestone-02 detailed.
        write_milestone(
            &wt_b,
            "docs/impl-plan/milestone-02-bar.md",
            "# 02 detailed\n",
        );
        write_critique_json(
            &wt_b,
            "DM2cd",
            r#"{
              "step": "DM2cd",
              "summary": "milestone-02 had issues",
              "findings": [
                {"kind": "blocker", "section": "Task list", "title": "missing trace", "body": "task 3 has no trace target"}
              ],
              "notes": "follow up before DM2d"
            }"#,
        );

        let contribs = vec![
            WorktreeContribution {
                worktree_path: wt_a.clone(),
                milestone_name: "milestone-01-foo.md".into(),
                milestone_rel_path: "docs/impl-plan/milestone-01-foo.md".into(),
            },
            WorktreeContribution {
                worktree_path: wt_b.clone(),
                milestone_name: "milestone-02-bar.md".into(),
                milestone_rel_path: "docs/impl-plan/milestone-02-bar.md".into(),
            },
        ];

        let shards_written = merge_contributions(&main, "DM2cd", &contribs).unwrap();
        assert_eq!(shards_written, 2);

        // Milestone files copied back to main.
        assert_eq!(
            std::fs::read_to_string(main.join("docs/impl-plan/milestone-01-foo.md")).unwrap(),
            "# 01 detailed\n"
        );
        assert_eq!(
            std::fs::read_to_string(main.join("docs/impl-plan/milestone-02-bar.md")).unwrap(),
            "# 02 detailed\n"
        );

        // Each worker's critique landed at its own shard path.
        let shard_a = main.join("docs/critiques/DM2cd/milestone-01-foo.json");
        let shard_b = main.join("docs/critiques/DM2cd/milestone-02-bar.json");
        let body_a = std::fs::read_to_string(&shard_a).unwrap();
        let body_b = std::fs::read_to_string(&shard_b).unwrap();
        assert!(body_a.contains("milestone-01 clean"));
        assert!(body_b.contains("milestone-02 had issues"));
        assert!(body_b.contains("missing trace"));
        // Markdown siblings rendered.
        assert!(
            main.join("docs/critiques/DM2cd/milestone-01-foo.md")
                .exists()
        );
        assert!(
            main.join("docs/critiques/DM2cd/milestone-02-bar.md")
                .exists()
        );
        // No aggregated per-step JSON is written -- the gate's
        // directory-mode scan walks the shard dir directly.
        assert!(
            !main.join("docs/critiques/DM2cd-critique.json").exists(),
            "merge_contributions must not write the legacy aggregated JSON"
        );
    }

    #[test]
    fn merge_contributions_handles_missing_critique_json() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        write_milestone(&wt, "docs/impl-plan/milestone-01-foo.md", "# 01\n");
        // No critique JSON written by the worker.

        let contribs = vec![WorktreeContribution {
            worktree_path: wt,
            milestone_name: "milestone-01-foo.md".into(),
            milestone_rel_path: "docs/impl-plan/milestone-01-foo.md".into(),
        }];

        let shards_written = merge_contributions(&main, "DM2cd", &contribs).unwrap();
        assert_eq!(
            shards_written, 0,
            "missing critique JSON means no shard was written"
        );
        assert!(
            main.join("docs/impl-plan/milestone-01-foo.md").exists(),
            "milestone file still copied even when critique is missing"
        );
    }

    #[test]
    fn merge_contributions_fails_on_malformed_critique_json() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        write_milestone(&wt, "docs/impl-plan/milestone-01-foo.md", "# 01\n");
        write_critique_json(&wt, "DM2cd", "this is not json");

        let contribs = vec![WorktreeContribution {
            worktree_path: wt,
            milestone_name: "milestone-01-foo.md".into(),
            milestone_rel_path: "docs/impl-plan/milestone-01-foo.md".into(),
        }];

        let err = merge_contributions(&main, "DM2cd", &contribs).unwrap_err();
        assert!(err.to_string().contains("malformed"));
    }

    #[test]
    fn cleanup_root_removes_output_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
        init_git_repo(&tmp_path);
        let mgr = WorktreeManager::create_for_step(&tmp_path, "DM2cd-root").unwrap();
        let root = mgr.output_root.clone();
        assert!(root.exists());
        mgr.cleanup_root();
        assert!(!root.exists(), "output_root should be removed");
    }
}
