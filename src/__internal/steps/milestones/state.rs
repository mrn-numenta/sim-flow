//! Per-milestone state checks and the checkbox-mode reset.
//!
//! The walker decides which milestone to target by asking
//! [`milestone_is_pending`] / [`milestone_is_touched`] of each file.
//! Reset reverses the checkbox flips so `sim-flow reset <step>` leaves
//! the milestone bodies in the same shape they had pre-run.

use super::MilestoneWalkConfig;

/// True iff the named milestone is fully resolved -- the parallel
/// plan-detail walk dispatcher uses this to decide whether its
/// pinned milestone session is done, since
/// [`super::walk::find_current_milestone`] no longer gives a useful
/// answer when multiple workers are racing.
///
/// Placeholder-mode (DM2cd / DM3ad / DM4ad): resolved iff the
/// `<!-- detail-pending` marker is gone from the milestone body.
///
/// Execution-mode (DM2d / DM3b / DM3c / DM4b): resolved iff no `[ ]`
/// rows remain (and, when `walk.forbid_deferred`, no `[-]` rows
/// either). Defined as `!milestone_is_pending`; exposed so the
/// orchestrator's pinned-worker wind-down can ask the question
/// directly.
pub fn milestone_is_resolved(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    !milestone_is_pending(walk, path)
}

/// "Pending" means the agent still has work to do on this milestone.
/// In execution mode that's a `- [ ]` row -- and additionally `- [-]`
/// (deferred) when `walk.forbid_deferred` is true, so the walker
/// targets a milestone with deferrals until they're implemented as
/// `- [x]`. In planning-detail mode that's the placeholder marker
/// still present in the body.
pub(super) fn milestone_is_pending(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    match walk.placeholder_marker {
        Some(marker) => milestone_body_contains(path, marker),
        None => {
            milestone_has_pending_row(path)
                || (walk.forbid_deferred && milestone_has_deferred_row(path))
        }
    }
}

/// "Touched" means the agent has done at least some work on this
/// milestone -- used as the retry-target picker so a critique
/// re-runs the milestone the agent JUST attempted, not whichever
/// one is next pending. In execution mode that's a `- [x]` row
/// (the agent ticked at least one task); in planning-detail mode
/// that's the placeholder having been removed (the agent replaced
/// the stub with real content).
pub(super) fn milestone_is_touched(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    match walk.placeholder_marker {
        Some(marker) => !milestone_body_contains(path, marker),
        None => milestone_has_checked_row(path),
    }
}

fn milestone_body_contains(path: &std::path::Path, needle: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(body) => body.contains(needle),
        Err(_) => false,
    }
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [ ]`.
fn milestone_has_pending_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines()
        .any(|line| line.trim_start().starts_with("- [ ]"))
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [-]`. Used (with
/// `forbid_deferred`) to keep the walker targeting milestones that
/// still have deferred rows, so DM2d / DM3c / DM4b force the agent
/// to actually implement them rather than letting `- [-]` masquerade
/// as resolved.
fn milestone_has_deferred_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines()
        .any(|line| line.trim_start().starts_with("- [-]"))
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [x]` (or `- [X]`). Used to
/// detect "the agent has worked on this milestone".
fn milestone_has_checked_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]")
    })
}

/// Reset every milestone file under `walk.dir` matching one of
/// `walk.file_prefixes`: flip every `- [x]` / `- [X]` / `- [-]` row
/// back to `- [ ]`. The task TEXT after the checkbox is preserved
/// verbatim, as are non-task lines (headings, prose, code fences).
/// Files outside the prefix set (e.g. `plan.md`,
/// `plan-management.md`) are left untouched. Returns the list of
/// files actually rewritten (skipped when already in their target
/// shape so we don't churn mtimes).
pub(super) fn reset_checkbox_milestones(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut touched = Vec::new();
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
        // Same digit-suffix gate as find_current_milestone so non-
        // milestone files (plan.md, plan-management.md) aren't
        // touched even if their basename happens to start with the
        // prefix.
        let rest = &name[prefix.len()..];
        if !rest
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let mut changed = false;
        let new_lines: Vec<String> = body
            .lines()
            .map(|line| {
                let trimmed_start = line.trim_start();
                let indent_len = line.len() - trimmed_start.len();
                if trimmed_start.starts_with("- [x]")
                    || trimmed_start.starts_with("- [X]")
                    || trimmed_start.starts_with("- [-]")
                {
                    // Replace just the 5-char `- [x]` prefix; keep
                    // the indentation and trailing text byte-exact.
                    changed = true;
                    let after = &trimmed_start[5..];
                    format!("{}- [ ]{}", &line[..indent_len], after)
                } else {
                    line.to_string()
                }
            })
            .collect();
        if !changed {
            continue;
        }
        let mut new_body = new_lines.join("\n");
        if body.ends_with('\n') && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        std::fs::write(&path, new_body)?;
        touched.push(path);
    }
    Ok(touched)
}
