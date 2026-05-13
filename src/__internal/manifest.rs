//! Per-step write manifest.
//!
//! Each flow step writes a newline-delimited list of the
//! project-relative paths it mutated to
//! `<project>/.sim-flow/manifests/<step>.txt`. Tools that touch the
//! filesystem (`write_file`, `edit_file`, `delete_file`, and the
//! orchestrator-side artifact-extract / cargo-checks paths) record
//! into the manifest of the step that's currently running.
//!
//! The reset cascade reads these manifests to know exactly which
//! files each downstream step owns, so a shared work_artifact
//! directory (e.g. `tests/` is claimed by both DM2d and DM3b/c) can
//! be selectively cleaned: upstream-owned files survive, only the
//! downstream additions are swept. Without manifests, the protection
//! had to be dir-granular and a shared directory wholly blocked the
//! downstream cleanup -- the reset would short-circuit, leaving
//! stale DM3b/DM3c outputs on disk after a reset to DM3a.
//!
//! Best-effort everywhere: a missing manifest is interpreted as "no
//! recorded writes for that step" (callers fall back to
//! `work_artifacts` declarations). I/O errors during recording are
//! swallowed so a transient filesystem hiccup never aborts an
//! otherwise-successful tool call.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Resolve the on-disk path of a step's manifest file. Stable across
/// callers so writers and readers (record / `step_paths` / `clear`)
/// agree on layout. Public so callers that want to inspect /
/// hand-edit the manifest (rare; e.g. a one-off migration) can find
/// it without hard-coding the layout.
pub fn manifest_path(project_dir: &Path, step_id: &str) -> PathBuf {
    project_dir
        .join(".sim-flow")
        .join("manifests")
        .join(format!("{step_id}.txt"))
}

/// Append `rel_path` to `step_id`'s manifest, deduplicating against
/// the file's existing contents so repeated writes to the same path
/// (e.g. `edit_file` over many turns) don't bloat the list. Creates
/// the parent directory on first use. Silently no-ops on I/O errors
/// -- the manifest is best-effort metadata; a failure here must not
/// abort the calling tool.
pub fn record_write(project_dir: &Path, step_id: &str, rel_path: &str) {
    let trimmed = rel_path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return;
    }
    let path = manifest_path(project_dir, step_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == trimmed) {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = writeln!(file, "{trimmed}");
}

/// Absolute paths recorded for `step_id`. Empty set when the
/// manifest file is missing or unreadable. Paths are joined against
/// `project_dir` (manifest stores project-relative form).
pub fn step_paths(project_dir: &Path, step_id: &str) -> HashSet<PathBuf> {
    let path = manifest_path(project_dir, step_id);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    body.lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() {
                None
            } else {
                Some(project_dir.join(t))
            }
        })
        .collect()
}

/// Remove the manifest file for `step_id`. Called as part of a step
/// reset so the manifest doesn't outlive the artifacts it tracked
/// (a stale manifest would re-flag already-deleted files on the
/// next reset). Returns `true` only when a manifest actually existed
/// and was removed; `false` covers both "missing" and "delete
/// failed" because the caller treats both identically.
pub fn clear(project_dir: &Path, step_id: &str) -> bool {
    std::fs::remove_file(manifest_path(project_dir, step_id)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_creates_manifest_and_appends_unique_paths() {
        let tmp = tempfile::tempdir().unwrap();
        record_write(tmp.path(), "DM3a", "docs/test-plan/test-plan.md");
        record_write(tmp.path(), "DM3a", "docs/test-plan/coverage.md");
        // Duplicate -- should not double-record.
        record_write(tmp.path(), "DM3a", "docs/test-plan/test-plan.md");
        let body = std::fs::read_to_string(manifest_path(tmp.path(), "DM3a")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.contains(&"docs/test-plan/test-plan.md"));
        assert!(lines.contains(&"docs/test-plan/coverage.md"));
    }

    #[test]
    fn step_paths_returns_absolute_joined_paths() {
        let tmp = tempfile::tempdir().unwrap();
        record_write(tmp.path(), "DM2d", "tests/elaboration.rs");
        record_write(tmp.path(), "DM2d", "src/model/top.rs");
        let paths = step_paths(tmp.path(), "DM2d");
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&tmp.path().join("tests/elaboration.rs")));
        assert!(paths.contains(&tmp.path().join("src/model/top.rs")));
    }

    #[test]
    fn step_paths_missing_manifest_returns_empty_set() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(step_paths(tmp.path(), "NEVER").is_empty());
    }

    #[test]
    fn clear_removes_manifest_and_subsequent_reads_are_empty() {
        let tmp = tempfile::tempdir().unwrap();
        record_write(tmp.path(), "DM3b", "tests/testbench/mod.rs");
        assert!(!step_paths(tmp.path(), "DM3b").is_empty());
        assert!(clear(tmp.path(), "DM3b"));
        assert!(step_paths(tmp.path(), "DM3b").is_empty());
        // Second clear is idempotent (returns false, doesn't panic).
        assert!(!clear(tmp.path(), "DM3b"));
    }

    #[test]
    fn record_ignores_empty_and_trailing_slash_paths() {
        let tmp = tempfile::tempdir().unwrap();
        record_write(tmp.path(), "DM3a", "");
        record_write(tmp.path(), "DM3a", "   ");
        record_write(tmp.path(), "DM3a", "docs/test-plan/");
        let paths = step_paths(tmp.path(), "DM3a");
        // Trailing slash normalized off; one entry expected.
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&tmp.path().join("docs/test-plan")));
    }
}
