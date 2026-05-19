//! `write_file(path: string, content: string)` - write a project
//! file. Mostly redundant with the artifact-write fenced-block
//! convention, but exposed as a tool too so LLMs that prefer the
//! native tool-use API don't have to switch styles.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_safe_path};
use crate::Result;
use crate::steps::is_path_allowed_for_writes;

pub struct WriteFileTool;

/// If `requested` doesn't appear in the milestone body but a SIBLING
/// path with the same filename does (under a different parent dir
/// rooted at the same top-level component), return the milestone's
/// canonical path. Returns `None` when no body, no match, or the
/// path is already canonical.
///
/// Example: agent writes `src/stage_add_one.rs`; milestone body
/// contains `` `src/model/stage_add_one.rs::AddOne` `` (in a task
/// row) -> redirect to `src/model/stage_add_one.rs`.
///
/// Conservative on purpose: only redirects when the requested path's
/// FILENAME matches a milestone-mentioned path AND the requested
/// path is NOT itself mentioned. Same-filename matches across
/// unrelated top-level dirs (e.g. `tests/foo.rs` vs `src/foo.rs`)
/// are left alone -- the requested path stays where the agent put
/// it.
fn autocorrect_path(requested: &str, milestone_body: Option<&str>) -> Option<String> {
    let body = milestone_body?;
    if body.contains(requested) {
        // Already canonical (or at least milestone-mentioned).
        return None;
    }
    let filename = std::path::Path::new(requested).file_name()?.to_str()?;
    let req_top = requested.split('/').next()?;
    // Scan body for tokens that look like project-relative paths
    // ending in our filename. The milestone format encloses paths
    // in backticks (`src/model/stage_add_one.rs::AddOne`); we strip
    // the optional `::Symbol[::Sub]` tail and pick the first
    // match whose top-level component matches the requested path.
    for token in body
        .split(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'' | ',' | ';' | '(' | ')'))
    {
        let cleaned = token.split("::").next().unwrap_or(token);
        if cleaned.is_empty() || cleaned == requested {
            continue;
        }
        let cleaned_filename = match std::path::Path::new(cleaned)
            .file_name()
            .and_then(|n| n.to_str())
        {
            Some(n) => n,
            None => continue,
        };
        if cleaned_filename != filename {
            continue;
        }
        let cleaned_top = match cleaned.split('/').next() {
            Some(t) => t,
            None => continue,
        };
        if cleaned_top != req_top {
            continue;
        }
        // Sanity: the canonical token must be a valid relative path
        // (no `..`, no leading `/`).
        if cleaned.starts_with('/') || cleaned.split('/').any(|c| c == ".." || c.is_empty()) {
            continue;
        }
        return Some(cleaned.to_string());
    }
    None
}

impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "Write a file under the project directory. Replaces any existing content."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": { "type": "string", "description": "Project-relative file path." },
                "content": { "type": "string", "description": "Full file contents." }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        // Pedagogical errors. qwen3.6 (and similar verbose-CoT
        // models) repeatedly emit `{"path": "..."}` without a
        // `content` field, get the bare "missing arg" error back,
        // and emit the SAME shape again -- a runaway loop that
        // burns 5-15 dispatches before the model recovers. Spelling
        // out the full required shape inline lets the model
        // self-correct on the first retry instead.
        const MISSING_PATH_HELP: &str = "write_file: missing `path` arg. \
             Required shape: \
             `{\"path\": \"<relative-path>\", \"content\": \"<full file body>\"}`. \
             Both fields are required; `content` cannot be omitted.";
        const MISSING_CONTENT_HELP: &str = "write_file: missing `content` arg. \
             Required shape: \
             `{\"path\": \"<relative-path>\", \"content\": \"<full file body>\"}`. \
             You provided `path` but no `content`; include the full file body \
             as a string literal.";
        let mut path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err(MISSING_PATH_HELP)),
        };
        if path.starts_with("lib:") {
            return Ok(ToolResult::err(
                "write_file: the library root is read-only; `lib:` paths cannot be written",
            ));
        }
        // Path autocorrect: if the agent's path is not referenced by
        // the active milestone task list but a SIBLING path with the
        // same filename IS, redirect to the milestone-canonical path.
        // Catches the common DM2d miss where the agent puts stage
        // code at `src/<file>.rs` even though the plan placed it at
        // `src/model/<file>.rs`.
        let mut redirect_note: Option<String> = None;
        if let Some(canonical) = autocorrect_path(&path, ctx.current_milestone_body) {
            redirect_note = Some(format!(
                "[write_file] auto-redirected `{path}` -> `{canonical}` to match the active milestone's task list. Update your subsequent reads / writes to use the canonical path."
            ));
            path = canonical;
        }
        if !is_path_allowed_for_writes(ctx.write_paths, &path) {
            return Ok(ToolResult::err(format!(
                "write_file: `{path}` is outside the write allowlist for this step+kind. Allowed: {}.",
                if ctx.write_paths.is_empty() {
                    "(none)".to_string()
                } else {
                    ctx.write_paths.join(", ")
                },
            )));
        }
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err(MISSING_CONTENT_HELP)),
        };
        let abs = match resolve_safe_path(ctx.project_dir, &path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("{e}"))),
        };
        // Refuse to follow symlinks. is_safe_relative_path already
        // rejects `..` traversal in the requested string, but
        // project_dir.join(rel) follows any pre-existing symlink
        // on disk -- a malicious file the LLM was asked to read
        // first and then re-creates (or a symlink in node_modules
        // pointing at /etc/hosts) would let write_file overwrite
        // files the user has access to. Check the IMMEDIATE
        // destination (symlink_metadata, not metadata): if it's a
        // symlink, refuse. If it doesn't exist yet, the parent
        // walk still has to be cleared, but rejecting the
        // destination is the main blast-radius reduction. See
        // orchestrator audit #6 (2026-05-16).
        if let Ok(meta) = std::fs::symlink_metadata(&abs)
            && meta.file_type().is_symlink()
        {
            return Ok(ToolResult::err(format!(
                "write_file: refusing to write through symlink `{path}` -- delete the symlink first if you intended to replace it with a regular file",
            )));
        }
        if let Some(parent) = abs.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return Ok(ToolResult::err(format!(
                "write_file: mkdir `{}` failed: {err}",
                parent.display()
            )));
        }
        // Enforce-on-write: when the agent writes `docs/spec.md`,
        // parse + validate the proposed content BEFORE persisting.
        // If validation fails the write is refused and the agent
        // gets a structured listing of every violation. This makes
        // the structured schema actually mandatory rather than
        // advisory — the agent cannot land a spec.md that would
        // trip the DM0 gate. See the user-led discussion in
        // 2026-05-19 (Phase 9.x) for the motivation.
        if path == "docs/spec.md"
            && let Err(spec_err) =
                crate::__internal::session::spec_md::validate_proposed_spec_md(&content)
        {
            return Ok(ToolResult::err(spec_err.to_agent_message()));
        }
        // When the agent writes a critique JSON, render the markdown
        // sibling AFTER the JSON write -- but roll the JSON write
        // back if render fails. Previously the JSON was committed
        // and render failure left a half-state (gate would act on
        // the JSON findings on the next evaluation, but the
        // dashboard had no .md to display). Now we either commit
        // BOTH files atomically or neither. See orchestrator audit
        // #13 (2026-05-16).
        match std::fs::write(&abs, content.as_bytes()) {
            Ok(()) => {
                if crate::critique::is_critique_json_path(&path)
                    && let Err(err) =
                        crate::critique::render_critique_markdown_to_disk(ctx.project_dir, &path)
                {
                    // Roll back the JSON write so we don't leave a
                    // half-state on disk for the gate to read.
                    // Best-effort -- if the unlink fails the user
                    // still has a clear diagnostic naming the file.
                    let _ = std::fs::remove_file(&abs);
                    return Ok(ToolResult::err(format!(
                        "write_file: critique markdown render failed for `{path}`: {err}. Rolled back the JSON write so the gate doesn't act on an unrendered critique."
                    )));
                }
                if let Some(step_id) = ctx.step_id {
                    crate::manifest::record_write(ctx.project_dir, step_id, &path);
                }
                let mut msg = format!("[write_file `{path}`] {} bytes", content.len());
                if let Some(note) = redirect_note {
                    msg.push_str("\n\n");
                    msg.push_str(&note);
                }
                Ok(ToolResult::ok(msg).with_touched_path(&path))
            }
            Err(err) => Ok(ToolResult::err(format!(
                "write_file: cannot write `{path}`: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path, write_paths: &'a [String]) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None).with_write_paths(write_paths)
    }

    fn open_writes() -> Vec<String> {
        // Prefix-style entries (trailing slash) permit anything
        // under the named dir; non-slash entries match the exact
        // filename. is_path_allowed_for_writes treats a
        // trailing-slash entry as a prefix and a non-slash entry
        // as exact-match.
        vec![
            "out/".to_string(),
            "src/".to_string(),
            "a.txt".to_string(),
            "b.txt".to_string(),
            "link.txt".to_string(),
        ]
    }

    #[test]
    fn writes_a_file_under_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "out/a.txt", "content": "hello\n"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        let body = std::fs::read_to_string(tmp.path().join("out/a.txt")).unwrap();
        assert_eq!(body, "hello\n");
        assert_eq!(result.touched_paths, vec!["out/a.txt".to_string()]);
    }

    #[test]
    fn spec_md_invalid_content_is_rejected_without_writing() {
        // Agent tries to write a freeform spec.md (the
        // pre-Phase-1 layout the rv12 project hit). The
        // validator rejects it; file on disk is unchanged.
        let tmp = tempfile::tempdir().unwrap();
        let writes = vec!["docs/".to_string()];
        // Pre-existing content the write should NOT overwrite.
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/spec.md"), "preexisting\n").unwrap();
        let bad_content = "# Bad spec\n\n## Purpose And Scope\n\nfreeform body\n";
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "docs/spec.md", "content": bad_content}),
            )
            .unwrap();
        assert!(!result.ok, "expected rejection, got: {}", result.display);
        assert!(
            result.display.contains("docs/spec.md rejected"),
            "missing rejection header: {}",
            result.display
        );
        // File on disk is unchanged.
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/spec.md")).unwrap();
        assert_eq!(on_disk, "preexisting\n");
    }

    #[test]
    fn spec_md_valid_content_passes_the_guard() {
        // The minimal-valid body from `gate.rs::tests::minimal_valid_body`
        // satisfies `spec.validate()`. The write guard MUST let
        // this through unchanged.
        let tmp = tempfile::tempdir().unwrap();
        let writes = vec!["docs/".to_string()];
        let minimal_valid = "## Metadata\n\n\n## Purpose\n\n## Scope\n\n## Non-goals\n\n## Assumptions and Constraints\n\n### Quantitative\n\n| Constraint | Value | Source-anchor |\n| --- | --- | --- |\n| Clock frequency | 1 GHz | primary:p1 |\n| Gate budget per cycle | 50 | primary:p1 |\n\n## Blocks\n\n## Functional Behavior\n\n## Timing, Latency, and Throughput\n\n## Pipeline and Hierarchy\n\n## Reset, Initialization, Flush, Drain\n\n## Worked Examples\n\n## Source-Spec Anchors\n\n## Open Questions\n\n## Auto-decisions\n\n";
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "docs/spec.md", "content": minimal_valid}),
            )
            .unwrap();
        assert!(result.ok, "expected success, got: {}", result.display);
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/spec.md")).unwrap();
        assert_eq!(on_disk, minimal_valid);
    }

    #[test]
    fn writes_overwrite_existing_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "old\n").unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "a.txt", "content": "new\n"}),
            )
            .unwrap();
        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn missing_path_arg_returns_pedagogical_error() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(&ctx(tmp.path(), &writes), &json!({"content": "x"}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing `path`"));
        // The error should include the required-shape hint so the
        // model can self-correct on the first retry.
        assert!(result.display.contains("Required shape"));
    }

    #[test]
    fn missing_content_arg_returns_pedagogical_error() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(&ctx(tmp.path(), &writes), &json!({"path": "a.txt"}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing `content`"));
        assert!(result.display.contains("Required shape"));
    }

    #[test]
    fn lib_prefix_is_rejected_as_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "lib:foo.md", "content": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("library root is read-only"));
    }

    #[test]
    fn write_outside_allowlist_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = vec!["docs/".to_string()];
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "src/a.rs", "content": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("outside the write allowlist"));
        assert!(result.display.contains("docs/"));
    }

    #[test]
    fn write_with_empty_allowlist_says_none() {
        let tmp = tempfile::tempdir().unwrap();
        let writes: Vec<String> = vec![];
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "a.txt", "content": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("(none)"));
    }

    #[test]
    fn dotdot_traversal_is_rejected_by_resolve_safe_path() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "../escape.txt", "content": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn absolute_path_is_rejected_by_resolve_safe_path() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "/etc/passwd", "content": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn symlink_destination_is_refused() {
        // Pre-existing symlink at the target path must be refused
        // (orchestrator audit #6 fix). Create a regular file
        // outside the project dir and a symlink inside pointing
        // at it; write_file must refuse rather than overwrite the
        // target.
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let writes = open_writes();
        let result = WriteFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({"path": "link.txt", "content": "would-be evil\n"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.display.contains("symlink"),
            "got: {}",
            result.display
        );
        // The outside-tempfile should still hold its original
        // contents (empty).
        let outside_body = std::fs::read_to_string(outside.path()).unwrap();
        assert!(outside_body.is_empty(), "symlink target was clobbered");
    }

    // --- autocorrect_path ---

    #[test]
    fn autocorrect_returns_none_with_no_milestone_body() {
        assert!(autocorrect_path("src/a.rs", None).is_none());
    }

    #[test]
    fn autocorrect_returns_none_when_path_is_already_milestone_canonical() {
        let body = "- [ ] `src/model/foo.rs::Foo` builds the foo";
        assert!(autocorrect_path("src/model/foo.rs", Some(body)).is_none());
    }

    #[test]
    fn autocorrect_redirects_to_milestone_canonical_path() {
        let body = "- [ ] `src/model/foo.rs::Foo` builds the foo";
        let out = autocorrect_path("src/foo.rs", Some(body));
        assert_eq!(out.as_deref(), Some("src/model/foo.rs"));
    }

    #[test]
    fn autocorrect_keeps_top_level_component_constraint() {
        // Same filename across DIFFERENT top-level dirs is not
        // redirected; the agent's choice stands.
        let body = "- [ ] `tests/foo.rs::Foo` tests";
        let out = autocorrect_path("src/foo.rs", Some(body));
        assert!(out.is_none());
    }

    #[test]
    fn autocorrect_rejects_traversal_in_milestone_token() {
        // Milestone text containing a token with `..` must not
        // produce a redirect target -- defense in depth in case
        // the milestone is LLM-authored.
        let body = "- [ ] `src/../../etc/foo.rs::Foo` mischief";
        let out = autocorrect_path("src/foo.rs", Some(body));
        assert!(out.is_none());
    }
}
