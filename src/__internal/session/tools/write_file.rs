//! `write_file(path: string, content: string)` - write a project
//! file. Mostly redundant with the artifact-write fenced-block
//! convention, but exposed as a tool too so LLMs that prefer the
//! native tool-use API don't have to switch styles.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_safe_path};
use crate::Result;

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
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("write_file: missing `path` arg")),
        };
        if path.starts_with("lib:") {
            return Ok(ToolResult::err(
                "write_file: the library root is read-only; `lib:` paths cannot be written",
            ));
        }
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("write_file: missing `content` arg")),
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
            Ok(()) => Ok(ToolResult::ok(format!(
                "[write_file `{path}`] {} bytes",
                content.len()
            ))),
            Err(err) => Ok(ToolResult::err(format!(
                "write_file: cannot write `{path}`: {err}"
            ))),
        }
    }
}
