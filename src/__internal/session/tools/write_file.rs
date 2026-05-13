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
        if let Some(parent) = abs.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return Ok(ToolResult::err(format!(
                "write_file: mkdir `{}` failed: {err}",
                parent.display()
            )));
        }
        match std::fs::write(&abs, content.as_bytes()) {
            Ok(()) => {
                // Render the critique markdown sibling when the
                // agent writes a critique JSON. Mirrors the
                // fenced-block path in `orchestrator::write_artifact`
                // so both write surfaces produce both files.
                if crate::critique::is_critique_json_path(&path)
                    && let Err(err) =
                        crate::critique::render_critique_markdown_to_disk(ctx.project_dir, &path)
                {
                    return Ok(ToolResult::err(format!(
                        "write_file: critique JSON written to `{path}` but markdown render failed: {err}"
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
