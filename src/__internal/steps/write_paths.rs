//! Write-path policy for sessions.
//!
//! Each step exposes the project-relative path prefixes its work sessions
//! are allowed to write to. Critique sessions are always scoped to the
//! canonical critique filenames (JSON + the rendered Markdown sibling).
//! Path enforcement is prefix-match for entries ending in `/` and
//! exact-match otherwise.

use super::StepDescriptor;

/// Convention documents that agents MUST treat as read-only. When an
/// agent's `write_file` or `edit_file` targets one of these paths,
/// the tool rejects the call with a structured error. These files
/// encode the cross-flow contracts every step relies on (plan-file
/// shape, architecture-doc shape) and must NEVER be mutated by an
/// agent mid-run -- if the file looks wrong, the answer is to fix
/// project bootstrap, not to rewrite the convention.
///
/// Paths are project-relative. Entries are exact-match (no prefix
/// wildcards) so we don't accidentally lock down whole directories.
pub const READ_ONLY_CONVENTION_PATHS: &[&str] = &[
    "docs/plan-management.md",
    "docs/architecture/architecture-format.md",
];

/// File-extension-based read-only set. Paths whose lowercased
/// extension matches one of these suffixes are rejected regardless
/// of their location. Covers:
///
/// - `.pdf` -- input source-spec corpus (preserved verbatim for
///   citation / ingestion; agents must never mutate the upstream
///   document).
/// - `.tmpl` -- project-bootstrap template files. The agent reads
///   these to learn the required artifact shape but the canonical
///   templates live in the foundation tree; rewriting a project's
///   `.tmpl` would silently fork the bootstrap contract.
pub const READ_ONLY_EXTENSIONS: &[&str] = &["pdf", "tmpl"];

/// Reason for rejecting a write/edit on a read-only path.
pub enum ReadOnlyReason {
    /// Path matches one of [`READ_ONLY_CONVENTION_PATHS`] verbatim.
    ConventionDoc,
    /// Path's lowercased extension matches one of
    /// [`READ_ONLY_EXTENSIONS`].
    ProtectedExtension(&'static str),
}

/// Classify `path` (project-relative) against the read-only policy.
/// Returns `None` when writes are allowed; `Some(reason)` when the
/// caller MUST reject. Mirrors the normalization in
/// [`is_path_allowed_for_writes`] so Windows-style backslash inputs
/// resolve the same way.
pub fn classify_read_only(path: &str) -> Option<ReadOnlyReason> {
    let normalized = path.replace('\\', "/");
    if READ_ONLY_CONVENTION_PATHS.iter().any(|p| normalized == *p) {
        return Some(ReadOnlyReason::ConventionDoc);
    }
    let ext = std::path::Path::new(&normalized)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    if let Some(ext) = ext.as_deref()
        && let Some(matched) = READ_ONLY_EXTENSIONS.iter().find(|x| **x == ext)
    {
        return Some(ReadOnlyReason::ProtectedExtension(matched));
    }
    None
}

/// True iff `path` (project-relative) is a read-only convention
/// document. Mirrors the normalization in
/// [`is_path_allowed_for_writes`] so Windows-style backslash inputs
/// resolve the same way.
pub fn is_read_only_convention_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    READ_ONLY_CONVENTION_PATHS.iter().any(|p| normalized == *p)
}

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

    #[test]
    fn classify_read_only_convention_doc_hits() {
        assert!(matches!(
            classify_read_only("docs/plan-management.md"),
            Some(ReadOnlyReason::ConventionDoc)
        ));
        assert!(matches!(
            classify_read_only("docs/architecture/architecture-format.md"),
            Some(ReadOnlyReason::ConventionDoc)
        ));
    }

    #[test]
    fn classify_read_only_pdf_extension_hits() {
        assert!(matches!(
            classify_read_only("docs/RV12.pdf"),
            Some(ReadOnlyReason::ProtectedExtension("pdf"))
        ));
        // Case-insensitive.
        assert!(matches!(
            classify_read_only("docs/RV12.PDF"),
            Some(ReadOnlyReason::ProtectedExtension("pdf"))
        ));
        // Anywhere in the tree.
        assert!(matches!(
            classify_read_only(".sim-flow/source-spec.pdf"),
            Some(ReadOnlyReason::ProtectedExtension("pdf"))
        ));
    }

    #[test]
    fn classify_read_only_tmpl_extension_hits() {
        assert!(matches!(
            classify_read_only("docs/spec.md.tmpl"),
            Some(ReadOnlyReason::ProtectedExtension("tmpl"))
        ));
        assert!(matches!(
            classify_read_only("docs/analysis/decomposition.md.tmpl"),
            Some(ReadOnlyReason::ProtectedExtension("tmpl"))
        ));
        // Case-insensitive.
        assert!(matches!(
            classify_read_only("docs/spec.md.TMPL"),
            Some(ReadOnlyReason::ProtectedExtension("tmpl"))
        ));
    }

    #[test]
    fn classify_read_only_normal_files_pass() {
        assert!(classify_read_only("docs/spec.md").is_none());
        assert!(classify_read_only("src/model/foo.rs").is_none());
        assert!(classify_read_only("docs/critiques/DM0-critique.json").is_none());
        // Filenames that happen to CONTAIN ".pdf" or ".tmpl" but
        // aren't the actual extension are not blocked.
        assert!(classify_read_only("docs/notes/about-pdfs.md").is_none());
    }

    #[test]
    fn classify_read_only_windows_separators_normalize() {
        assert!(matches!(
            classify_read_only("docs\\plan-management.md"),
            Some(ReadOnlyReason::ConventionDoc)
        ));
        assert!(matches!(
            classify_read_only("docs\\spec.md.tmpl"),
            Some(ReadOnlyReason::ProtectedExtension("tmpl"))
        ));
    }

    #[test]
    fn is_read_only_convention_path_matches_only_exact() {
        assert!(is_read_only_convention_path("docs/plan-management.md"));
        assert!(!is_read_only_convention_path("docs/plan-management.md.bak"));
        assert!(!is_read_only_convention_path("plan-management.md"));
        assert!(!is_read_only_convention_path(
            "subdir/docs/plan-management.md"
        ));
    }
}
