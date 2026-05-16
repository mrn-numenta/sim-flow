//! Write-path policy for sessions.
//!
//! Each step exposes the project-relative path prefixes its work sessions
//! are allowed to write to. Critique sessions are always scoped to the
//! canonical critique filenames (JSON + the rendered Markdown sibling).
//! Path enforcement is prefix-match for entries ending in `/` and
//! exact-match otherwise.

use super::StepDescriptor;

/// Allowed write-path prefixes for the given (step, kind). Work
/// sessions return the step's `work_write_paths`; critique sessions
/// allow the canonical critique filename in BOTH JSON form (the
/// shape the agent emits) and markdown form (the shape the
/// orchestrator renders post-write for human review). Path
/// enforcement uses prefix-match for entries ending in `/` and
/// exact-match otherwise.
pub fn allowed_write_paths(step: &StepDescriptor, kind: crate::client::SessionKind) -> Vec<String> {
    match kind {
        crate::client::SessionKind::Work => step
            .work_write_paths
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        crate::client::SessionKind::Critique => {
            vec![
                format!("docs/critiques/{}-critique.json", step.id),
                format!("docs/critiques/{}-critique.md", step.id),
            ]
        }
    }
}

/// True iff `path` (project-relative) is covered by one of the
/// `allowed` prefixes. `/` accepts both `\\` and `/` separators on
/// the input side so artifact paths that came in via the fenced
/// extractor on Windows still resolve correctly.
pub fn is_path_allowed_for_writes(allowed: &[String], path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    allowed.iter().any(|prefix| {
        if let Some(stripped) = prefix.strip_suffix('/') {
            normalized == stripped || normalized.starts_with(prefix)
        } else {
            normalized == *prefix
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn directory_prefix_matches_files_under_it() {
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs/spec.md"));
        assert!(is_path_allowed_for_writes(&a, "docs/critiques/DM0.md"));
    }

    #[test]
    fn directory_prefix_matches_bare_dir_form() {
        // The agent occasionally targets the directory itself
        // (e.g. `list_dir`-style references). For write paths this
        // is a no-op write target, but accepting `docs` against
        // allowlist `docs/` keeps the matcher symmetric and avoids
        // surprising rejections of round-tripped paths.
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs"));
    }

    #[test]
    fn directory_prefix_does_not_match_sibling_with_same_name_prefix() {
        // Regression: prefix `src/` must NOT match `src-backup/foo.rs`
        // -- the trailing slash in the prefix is load-bearing for
        // exactly this reason.
        let a = allowed(&["src/"]);
        assert!(!is_path_allowed_for_writes(&a, "src-backup/foo.rs"));
        assert!(!is_path_allowed_for_writes(&a, "srcsomething"));
    }

    #[test]
    fn exact_file_entry_matches_only_that_file() {
        let a = allowed(&["Cargo.toml"]);
        assert!(is_path_allowed_for_writes(&a, "Cargo.toml"));
        assert!(!is_path_allowed_for_writes(&a, "Cargo.toml.bak"));
        assert!(!is_path_allowed_for_writes(&a, "subdir/Cargo.toml"));
    }

    #[test]
    fn empty_allowlist_rejects_everything() {
        let a: Vec<String> = Vec::new();
        assert!(!is_path_allowed_for_writes(&a, "docs/spec.md"));
        assert!(!is_path_allowed_for_writes(&a, "anything"));
    }

    #[test]
    fn windows_style_separators_normalize_to_forward_slash() {
        // Artifact paths that come back through fenced blocks on
        // Windows shells use backslash separators. The allowlist is
        // declared in forward-slash form, so the matcher normalizes
        // the input before comparing.
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs\\spec.md"));
    }

    #[test]
    fn multiple_entries_match_independently() {
        let a = allowed(&["src/", "tests/", "Cargo.toml"]);
        assert!(is_path_allowed_for_writes(&a, "src/lib.rs"));
        assert!(is_path_allowed_for_writes(&a, "tests/it.rs"));
        assert!(is_path_allowed_for_writes(&a, "Cargo.toml"));
        assert!(!is_path_allowed_for_writes(&a, "docs/spec.md"));
    }
}
