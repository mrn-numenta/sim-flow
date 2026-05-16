//! Auto-tick checkbox rows whose backtick-quoted artifact has
//! landed on disk. Removes a "flip the box" turn from the agent's
//! milestone loop; the Critique still runs the full review on
//! anything the auto-tick touched.

use crate::steps::StepDescriptor;

use super::CurrentMilestone;
use super::walk::find_current_milestone;

/// Per-milestone task-row auto-tick. Walks the current milestone
/// file, finds every `- [ ]` row whose first backtick-quoted token
/// matches the `path[::Symbol[::Sub]]` pattern, verifies the file
/// exists (and the symbol grep-matches if a symbol was named), and
/// flips the row in place to `- [x]`. Returns the number of rows
/// flipped. Idempotent and a no-op when the step has no
/// `milestone_walk` config or the current milestone is `AllResolved`.
///
/// Conservative on purpose: a row whose backtick-quoted token does
/// NOT parse as `path::sym` (e.g. a prose row, or a row that names
/// only a directory) is left alone. The Critique still does the full
/// review; this just removes the agent's tick-the-checkbox turn from
/// the milestone loop.
pub fn tick_resolved_milestone_tasks(
    project_dir: &std::path::Path,
    step: &StepDescriptor,
) -> usize {
    let Some(walk) = step.milestone_walk else {
        return 0;
    };
    // Planning-detail walks (DM2cd / DM3ad / DM4ad,
    // `placeholder_marker = Some`) walk milestone STUBS and write
    // task lists describing what DM2d / DM3b / DM3c / DM4b will
    // later build. At the planning stage, "the named artifact
    // exists on disk" does NOT mean the task is done -- the task
    // is naming what WILL be produced, not what already is. Flip
    // here would silently mark planning tasks as completed, and
    // the critique would then re-flag the mismatch and loop until
    // the no-progress streak guard fires. Execution walks
    // (`placeholder_marker = None`) keep the auto-tick behavior --
    // there the rule "artifact exists -> task done" is correct.
    if walk.placeholder_marker.is_some() {
        return 0;
    }
    let CurrentMilestone::File(rel) = find_current_milestone(project_dir, &walk, true) else {
        return 0;
    };
    let path = project_dir.join(&rel);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let mut flipped = 0usize;
    // Iterate via split_inclusive so each segment retains its
    // original line ending (\n, \r\n, or the trailing slice with
    // no terminator). The prior `body.lines()` strip + `join("\n")`
    // re-emit silently converted CRLF files to LF -- breaks
    // milestone files edited on Windows or by editors that
    // preserve CRLF. See orchestrator audit #8 (2026-05-16).
    let new_body: String = body
        .split_inclusive('\n')
        .map(|line_with_terminator| {
            // Split off the trailing \r?\n so we mutate the
            // content, not the terminator.
            let (content, terminator) = split_line_terminator(line_with_terminator);
            let trimmed = content.trim_start();
            if !trimmed.starts_with("- [ ]") {
                return line_with_terminator.to_string();
            }
            let after = trimmed.trim_start_matches("- [ ]").trim_start();
            let Some(token) = first_backtick_token(after) else {
                return line_with_terminator.to_string();
            };
            if !task_artifact_resolved(project_dir, token) {
                return line_with_terminator.to_string();
            }
            flipped += 1;
            let replaced = content.replacen("- [ ]", "- [x]", 1);
            format!("{replaced}{terminator}")
        })
        .collect();
    if flipped > 0 && write_milestone_atomic(&path, &new_body).is_err() {
        return 0;
    }
    flipped
}

/// Split `s` into (content, terminator) where terminator is one of
/// "\r\n", "\n", or "" (no terminator on the final no-newline
/// segment returned by `split_inclusive`).
fn split_line_terminator(s: &str) -> (&str, &str) {
    if let Some(stripped) = s.strip_suffix("\r\n") {
        (stripped, "\r\n")
    } else if let Some(stripped) = s.strip_suffix('\n') {
        (stripped, "\n")
    } else {
        (s, "")
    }
}

/// Atomic write: tempfile -> sync -> rename -> parent fsync. Same
/// shape as state::write_atomic; inlined here to avoid coupling
/// the steps module to state. Without atomicity a crash mid-write
/// leaves a milestone file truncated and the auto loop can't
/// resume cleanly. See orchestrator audit #8 (2026-05-16).
fn write_milestone_atomic(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Pull the FIRST backtick-quoted token from `s`. Returns the inner
/// string (without the surrounding backticks) or `None` if no
/// well-formed token is found.
fn first_backtick_token(s: &str) -> Option<&str> {
    let start = s.find('`')?;
    let rest = &s[start + 1..];
    let end = rest.find('`')?;
    Some(&rest[..end])
}

/// True if `token` parses as `path[::Symbol[::Sub]]` AND the path
/// exists under `project_dir`, AND -- if a symbol was named -- the
/// LAST `::`-separated segment grep-matches as a word boundary in the
/// file. The grep is conservative: it accepts the symbol name in any
/// position (definition, comment, doc string) because tightening to
/// `\bfn name\b` / `\bstruct name\b` would miss legitimate variants
/// (associated methods, trait impls, type aliases) and produce
/// false-negatives the agent would then have to correct anyway. False
/// positives are recoverable: the Critique does the full review.
fn task_artifact_resolved(project_dir: &std::path::Path, token: &str) -> bool {
    let mut parts = token.splitn(2, "::");
    let path_str = match parts.next() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let abs = project_dir.join(path_str);
    if !abs.exists() {
        return false;
    }
    let Some(symbol_chain) = parts.next() else {
        return true;
    };
    let last_symbol = symbol_chain.rsplit("::").next().unwrap_or(symbol_chain);
    if last_symbol.is_empty() {
        return false;
    }
    let body = match std::fs::read_to_string(&abs) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == last_symbol)
}
