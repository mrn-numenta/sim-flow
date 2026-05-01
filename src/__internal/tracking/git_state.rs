//! Capture the current git state without creating branches or stashing.
//! The orchestrator records whatever the user has committed (or not);
//! users are responsible for their own branching discipline per
//! `docs/architecture/ai-flow/04-experiment-tracking.md`.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct GitState {
    pub commit: String,
    pub branch: Option<String>,
    pub dirty: bool,
}

impl GitState {
    /// Capture the commit, branch, and dirty flag for the working tree
    /// rooted at `project_dir`. If git is unavailable or the project is
    /// not a git repository, returns a sentinel state so tracking keeps
    /// working on non-git projects (useful for tests).
    pub fn capture(project_dir: &Path) -> Self {
        let commit = run_git(project_dir, &["rev-parse", "HEAD"])
            .unwrap_or_else(|| "unknown-not-a-git-repo".to_string());
        let branch = run_git(project_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
            .and_then(|b| if b == "HEAD" { None } else { Some(b) });
        let dirty = run_git(project_dir, &["status", "--porcelain"])
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        Self {
            commit,
            branch,
            dirty,
        }
    }

    /// Capture with an explicit "not a git project" marker. Useful when
    /// callers know up front that git should be bypassed.
    pub fn no_repo() -> Self {
        Self {
            commit: "unknown-not-a-git-repo".to_string(),
            branch: None,
            dirty: false,
        }
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_sim_foundation_state() {
        // The workspace this test runs in is a git repo, so the commit
        // lookup should succeed.
        let root = std::env::current_dir().unwrap();
        let state = GitState::capture(&root);
        assert!(!state.commit.is_empty());
        assert_ne!(state.commit, "unknown-not-a-git-repo");
    }

    #[test]
    fn non_git_dir_returns_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let state = GitState::capture(tmp.path());
        assert_eq!(state.commit, "unknown-not-a-git-repo");
    }
}
