//! Find / enumerate / look-up milestone files under a
//! [`MilestoneWalkConfig`]'s configured directory.
//!
//! Three public entry points:
//!
//! - [`find_current_milestone`] picks the single milestone the next
//!   work / critique session should target.
//! - [`enumerate_pending_milestones`] returns every still-pending
//!   milestone (used by the parallel plan-detail dispatcher).
//! - [`find_milestone_by_name`] resolves a specific filename or
//!   project-relative path back to a `CurrentMilestone::File`.

use super::state::{milestone_is_pending, milestone_is_touched};
use super::{CurrentMilestone, MilestoneWalkConfig, PendingMilestones};

/// Find the milestone file the next session should target.
///
/// Modes (cross-cutting with `MilestoneWalkConfig::placeholder_marker`):
///
/// **Execution mode** (placeholder_marker = None) -- DM2d / DM3b /
/// DM3c / DM4b walk over milestone files filling in code:
/// - **Fresh / advance** (`prior_critique_has_blockers = false`):
///   return the FIRST milestone file with at least one open `- [ ]`
///   row.
/// - **Retry** (`prior_critique_has_blockers = true`): return the
///   HIGHEST-NUMBERED milestone file with at least one `- [x]` row
///   (i.e., the agent's most recent target). Even if the agent
///   prematurely flipped its rows to `- [x]`, the retry stays on
///   THE SAME milestone the critique fired on. Falls back to
///   first-pending if no `- [x]` rows exist anywhere.
///
/// **Planning-detail mode** (placeholder_marker = Some(s)) --
/// DM2cd / DM3ad / DM4ad walk over stub milestone files written by
/// the outline step and replace the stub with a real task list:
/// - **Fresh / advance**: return the FIRST milestone file whose
///   body still contains the placeholder.
/// - **Retry**: return the HIGHEST-NUMBERED milestone whose
///   placeholder has been removed (= the agent has detailed it; the
///   critique fired on that milestone). Falls back to
///   first-pending if no milestone has been detailed yet.
///
/// Filenames are matched case-sensitively against any
/// `<prefix><digits>...md` for `<prefix>` in `walk.file_prefixes`.
/// The lexicographic sort over the matching subset gives the
/// natural numeric order as long as callers pad NN to two digits
/// (`01`, `02`, ...), which the prompts and `plan-management.md`
/// enforce.
pub fn find_current_milestone(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
    prior_critique_has_blockers: bool,
) -> CurrentMilestone {
    let files = match list_milestone_files(project_dir, walk) {
        Some(f) if !f.is_empty() => f,
        _ => return CurrentMilestone::NoMilestonesPresent,
    };

    if prior_critique_has_blockers {
        // Retry: highest-numbered milestone the agent has touched.
        for (name, path) in files.iter().rev() {
            if milestone_is_touched(walk, path) {
                return CurrentMilestone::File(join_milestone_rel(walk, name));
            }
        }
        // Nothing touched yet: fall through to first-pending.
    }

    for (name, path) in &files {
        if milestone_is_pending(walk, path) {
            return CurrentMilestone::File(join_milestone_rel(walk, name));
        }
    }
    CurrentMilestone::AllResolved
}

/// Project-relative paths of every milestone file under `walk.dir`
/// that is currently pending, in walker order. Used by the parallel
/// plan-detail walk dispatcher to fan out Work sessions across all
/// pending stubs at once; the serial walker keeps using
/// [`find_current_milestone`] one-at-a-time.
pub fn enumerate_pending_milestones(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> PendingMilestones {
    let Some(files) = list_milestone_files(project_dir, walk) else {
        return PendingMilestones::DirectoryMissing;
    };
    let pending = files
        .iter()
        .filter(|(_, path)| milestone_is_pending(walk, path))
        .map(|(name, _)| join_milestone_rel(walk, name))
        .collect();
    PendingMilestones::Present { pending }
}

/// Look up a specific milestone by its bare filename (e.g.
/// `"milestone-03-decode.md"`) or by its project-relative path (e.g.
/// `"docs/impl-plan/milestone-03-decode.md"`). Returns
/// `CurrentMilestone::File` for an exact match, regardless of whether
/// the milestone is pending, touched, or resolved -- the parallel
/// dispatcher uses this to scope a worker's session to a specific
/// stub it has already decided to operate on.
///
/// Returns `NoMilestonesPresent` when the directory is missing or
/// empty; `AllResolved` is never returned (it would conflate "found
/// but resolved" with "nothing matched the name").
pub fn find_milestone_by_name(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
    needle: &str,
) -> CurrentMilestone {
    let bare = std::path::Path::new(needle)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(needle);
    let Some(files) = list_milestone_files(project_dir, walk) else {
        return CurrentMilestone::NoMilestonesPresent;
    };
    if files.is_empty() {
        return CurrentMilestone::NoMilestonesPresent;
    }
    for (name, _) in &files {
        if name == bare {
            return CurrentMilestone::File(join_milestone_rel(walk, name));
        }
    }
    CurrentMilestone::NoMilestonesPresent
}

/// Walker-order list of every `<prefix><digits>...md` file in
/// `walk.dir`, used by [`find_current_milestone`],
/// [`enumerate_pending_milestones`], and [`find_milestone_by_name`].
/// Returns `None` if the directory cannot be read at all (missing
/// directory or permission error); `Some(empty)` if the directory
/// exists but no milestone files match.
pub(super) fn list_milestone_files(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> Option<Vec<(String, std::path::PathBuf)>> {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
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
        files.push((name.to_string(), path));
    }
    // Sort by (prefix-group, numeric prefix, full filename) so a
    // 10th milestone doesn't lex-sort before the 9th within a
    // group. Pure string comparison gave
    // `milestone-10 < milestone-9` whenever a project crossed the
    // single-digit boundary. Falling back to the prefix string
    // first keeps the existing cross-prefix ordering (the test
    // suite codifies tb-* before test-*); within a prefix, the
    // numeric run is the primary key. Filename fallback for the
    // tie-breaking case. See orchestrator audit #12 (2026-05-16).
    files.sort_by(|a, b| {
        let pa = milestone_prefix_match(&a.0, walk);
        let pb = milestone_prefix_match(&b.0, walk);
        pa.cmp(pb)
            .then_with(|| milestone_numeric_key(&a.0, walk).cmp(&milestone_numeric_key(&b.0, walk)))
            .then_with(|| a.0.cmp(&b.0))
    });
    Some(files)
}

/// Return the `walk.file_prefixes` entry that matches `name`'s
/// start, or `""` when no prefix matches (sorts before all
/// matched entries). Used to group files by prefix so the
/// numeric-aware sort below doesn't reorder e.g. `tb-*` and
/// `test-*` against each other.
fn milestone_prefix_match<'a>(name: &str, walk: &'a MilestoneWalkConfig) -> &'a str {
    walk.file_prefixes
        .iter()
        .find(|p| name.starts_with(**p))
        .copied()
        .unwrap_or("")
}

/// Extract the numeric prefix run of `name` after stripping the
/// configured `walk.file_prefixes` match. Returns `u64::MAX` when
/// the file doesn't start with a recognized prefix + digits so
/// such entries sort last; callers should already have filtered
/// them out, but this keeps the sort total.
fn milestone_numeric_key(name: &str, walk: &MilestoneWalkConfig) -> u64 {
    let Some(prefix) = walk.file_prefixes.iter().find(|p| name.starts_with(**p)) else {
        return u64::MAX;
    };
    let rest = &name[prefix.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().unwrap_or(u64::MAX)
}

pub(super) fn join_milestone_rel(walk: &MilestoneWalkConfig, name: &str) -> String {
    format!(
        "{}{}",
        if walk.dir.ends_with('/') {
            walk.dir.to_string()
        } else {
            format!("{}/", walk.dir)
        },
        name
    )
}
