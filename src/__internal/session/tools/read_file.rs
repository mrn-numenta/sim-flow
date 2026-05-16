//! `read_file(path: string)` - read a project file, library file, or
//! framework API asset (when prefixed with `lib:` / `fw:`).

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
                    "description": "Project-relative file path, `lib:<rel>` to read from the library root (sim-models docs / examples / library), or `fw:<rel>` to read framework assets. Prefer `fw:api/toc.md` plus specific `fw:api/pages/...md` files for curated API docs; use `fw:src/prelude.rs` when you need source-level signatures. Absolute paths and `..` traversal are rejected."
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
                    "read_file: requested `lib:` / `fw:` root is not configured for this project",
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(project: &std::path::Path) -> ToolContext<'_> {
        ToolContext::new(project, None, None, None)
    }

    #[test]
    fn read_file_missing_path_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadFileTool.invoke(&ctx(tmp.path()), &json!({})).unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn read_file_path_with_dot_dot_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "../escape.txt" }))
            .unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn read_file_returns_full_body_under_the_truncation_cap() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("note.md"), "hello\nworld\n").unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "note.md" }))
            .unwrap();
        assert!(r.ok);
        assert!(r.display.contains("hello"));
        assert!(r.display.contains("world"));
        assert!(!r.display.contains("truncated"));
    }

    #[test]
    fn read_file_truncates_bodies_past_the_max_bytes_cap() {
        let tmp = tempfile::tempdir().unwrap();
        // Write a body longer than MAX_BYTES (16K).
        let body = "x".repeat(20_000);
        std::fs::write(tmp.path().join("big.txt"), &body).unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "big.txt" }))
            .unwrap();
        assert!(r.ok);
        assert!(r.display.contains("truncated"));
        assert!(r.display.contains("20000 bytes"));
    }

    #[test]
    fn read_file_missing_file_returns_an_error_referencing_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "no/such/file.md" }))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("no/such/file.md"));
    }

    #[test]
    fn read_file_lib_prefix_with_unconfigured_library_root_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "lib:something.md" }))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("not configured") || r.display.contains("lib:"));
    }

    #[test]
    fn read_file_image_returns_attachment_with_byte_count_and_mime() {
        let tmp = tempfile::tempdir().unwrap();
        // Minimal PNG header is enough to convince image_mime_from_path.
        // Path extension is the actual probe -- just give it `.png`.
        let png = tmp.path().join("pic.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "pic.png" }))
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.attachments.len(), 1);
        assert_eq!(r.attachments[0].mime, "image/png");
        assert!(r.display.contains("inline image attachment"));
    }
}
