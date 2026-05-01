//! `list_dir(path: string)` - list a project or library directory.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_read_path};
use crate::Result;

pub struct ListDirTool;

impl Tool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }
    fn description(&self) -> &'static str {
        "List entries inside a project-relative directory."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative directory path (use `.` for the project root), `lib:` / `lib:<rel>` to list the library root, or `fw:` / `fw:<rel>` to list the foundation framework root (e.g. `fw:src/`)."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("list_dir: missing `path` arg")),
        };
        // Allow "." as the project root and "lib:" as the library root.
        let abs = if path == "." || path == "./" {
            ctx.project_dir.to_path_buf()
        } else {
            match resolve_read_path(ctx, &path) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return Ok(ToolResult::err(
                        "list_dir: `lib:` prefix used but no library root is configured for this project",
                    ));
                }
                Err(e) => {
                    return Ok(ToolResult::err(format!(
                        "list_dir: rejecting unsafe path `{path}`: {e}"
                    )));
                }
            }
        };
        let entries = match std::fs::read_dir(&abs) {
            Ok(it) => it,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "list_dir: cannot read `{path}`: {err}"
                )));
            }
        };
        let mut rows: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = match entry.file_type() {
                Ok(ft) if ft.is_dir() => "dir",
                Ok(ft) if ft.is_file() => "file",
                Ok(ft) if ft.is_symlink() => "symlink",
                _ => "other",
            };
            rows.push(format!("- [{kind}] {name}"));
        }
        rows.sort();
        Ok(ToolResult::ok(format!(
            "[list_dir `{path}`]\n\n{}",
            rows.join("\n")
        )))
    }
}
