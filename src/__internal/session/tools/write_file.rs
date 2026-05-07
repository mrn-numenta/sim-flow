//! `write_file(path: string, content: string)` - write a project
//! file. Mostly redundant with the artifact-write fenced-block
//! convention, but exposed as a tool too so LLMs that prefer the
//! native tool-use API don't have to switch styles.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_safe_path};
use crate::Result;
use crate::steps::is_path_allowed_for_writes;

pub struct WriteFileTool;

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
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err(MISSING_PATH_HELP)),
        };
        if path.starts_with("lib:") {
            return Ok(ToolResult::err(
                "write_file: the library root is read-only; `lib:` paths cannot be written",
            ));
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
                Ok(ToolResult::ok(format!(
                    "[write_file `{path}`] {} bytes",
                    content.len()
                )))
            }
            Err(err) => Ok(ToolResult::err(format!(
                "write_file: cannot write `{path}`: {err}"
            ))),
        }
    }
}
