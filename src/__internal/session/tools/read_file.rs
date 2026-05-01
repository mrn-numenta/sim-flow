//! `read_file(path: string)` - read a project file or a library file
//! (when prefixed with `lib:`).

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, image_mime_from_path, resolve_read_path};
use crate::Result;

const MAX_BYTES: usize = 16 * 1024;

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a file under the project directory and return its contents."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative file path, `lib:<rel>` to read from the library root (sim-models docs / examples / library), or `fw:<rel>` to read from the foundation framework root (e.g. `fw:src/prelude.rs` for the model-author API surface). Absolute paths and `..` traversal are rejected."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let path = match parse_path(args) {
            Some(p) => p,
            None => return Ok(ToolResult::err("read_file: missing or invalid `path` arg")),
        };
        let abs = match resolve_read_path(ctx, &path) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Ok(ToolResult::err(
                    "read_file: `lib:` prefix used but no library root is configured for this project",
                ));
            }
            Err(e) => return Ok(ToolResult::err(format!("{e}"))),
        };
        // Image files come back as multimodal attachments (when the
        // backend supports it) rather than as text. The display
        // string still goes into the conversation so the agent has
        // a textual reference, while the bytes ride alongside as an
        // attachment.
        if let Some(mime) = image_mime_from_path(&abs) {
            return match std::fs::read(&abs) {
                Ok(bytes) => Ok(ToolResult::ok_with_attachment(
                    format!(
                        "[read_file `{path}`] returned {} bytes of {mime} as an inline image \
                         attachment. The image is now visible to you in this turn.",
                        bytes.len()
                    ),
                    mime,
                    bytes,
                    path.clone(),
                )),
                Err(err) => Ok(ToolResult::err(format!(
                    "read_file: cannot read image `{path}`: {err}"
                ))),
            };
        }
        match std::fs::read_to_string(&abs) {
            Ok(body) => {
                let truncated = if body.len() > MAX_BYTES {
                    format!(
                        "{}\n... (truncated; original {} bytes)",
                        &body[..MAX_BYTES],
                        body.len()
                    )
                } else {
                    body
                };
                Ok(ToolResult::ok(format!(
                    "[read_file `{path}`]\n\n{truncated}"
                )))
            }
            Err(err) => Ok(ToolResult::err(format!(
                "read_file: cannot read `{path}`: {err}"
            ))),
        }
    }
}

/// Parse the `path` argument from either:
/// - A JSON object `{"path": "..."}` (native tool-use), or
/// - A bare string body where the first non-blank line is the path
///   (fenced-block fallback). The fenced-block path is wrapped into
///   `{"path": "..."}` by the orchestrator before invoking.
fn parse_path(args: &serde_json::Value) -> Option<String> {
    args.get("path").and_then(|v| v.as_str()).map(String::from)
}
