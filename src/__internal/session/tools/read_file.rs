//! `read_file(path: string, offset?: integer, length?: integer)` —
//! read a project file, library file, or framework API asset (when
//! prefixed with `lib:` / `fw:`).
//!
//! Returns a single slice of the file at `[offset, offset+length)`,
//! clamped to `MAX_BYTES_PER_CALL` per response. The display string
//! reports `total_bytes`, `offset`, and `bytes_returned` so the
//! caller can paginate without a separate `stat` round-trip.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, image_mime_from_path, resolve_read_path};
use crate::Result;

/// Per-call cap on the bytes we return to the agent. Files larger
/// than this require explicit `offset` / `length` pagination — the
/// truncation marker tells the agent how much is left.
///
/// Was 16 KB historically; bumped to 64 KB so the typical
/// `docs/spec.md` (~40-50 KB) fits in a single call once the agent
/// is targeting it directly. Source-spec chunks under
/// `.sim-flow/spec-ingest/primary/chunks/` are ~8 KB each so the
/// new cap doesn't bloat retrieval-tool reads.
pub(crate) const MAX_BYTES_PER_CALL: usize = 64 * 1024;

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a file under the project directory and return its contents. \
         Supports `offset` / `length` pagination — the response reports \
         `total_bytes` and how many bytes were returned so the caller \
         can issue follow-up reads without a separate stat call. Files \
         larger than the per-call cap (64 KB) are clamped; check the \
         truncation note in the body."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative file path, `lib:<rel>` to read from the library root (sim-models docs / examples / library), or `fw:<rel>` to read framework assets. Prefer `fw:api/toc.md` plus specific `fw:api/pages/...md` files for curated API docs; use `fw:src/prelude.rs` when you need source-level signatures. Absolute paths and `..` traversal are rejected."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Byte offset to start reading from. Default: 0. The header line in the response always reports `total_bytes` so a follow-up call can be issued with `offset = previous_offset + bytes_returned`."
                },
                "length": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum bytes to return in this call. Capped at 64 KB regardless of the requested value. Default: 64 KB (the whole file when it fits)."
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
        // backend supports it) rather than as text. Pagination
        // doesn't apply — agents receive the full image or none.
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
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let requested_length = args
            .get("length")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        match std::fs::read_to_string(&abs) {
            Ok(body) => {
                let total_bytes = body.len();
                if offset > total_bytes {
                    return Ok(ToolResult::err(format!(
                        "read_file: offset {offset} is past end of file (total {total_bytes} bytes)"
                    )));
                }
                let remaining = total_bytes - offset;
                let length = requested_length
                    .unwrap_or(MAX_BYTES_PER_CALL)
                    .min(MAX_BYTES_PER_CALL)
                    .min(remaining);
                let slice = byte_slice_at_char_boundary(&body, offset, length);
                let bytes_returned = slice.len();
                let end = offset + bytes_returned;
                let header = if total_bytes <= MAX_BYTES_PER_CALL && offset == 0 {
                    // Common path: the whole file fits and the caller
                    // didn't paginate. Keep the header terse so simple
                    // reads don't pay a multi-line overhead.
                    format!("[read_file `{path}`] ({total_bytes} bytes)")
                } else {
                    let truncated = end < total_bytes;
                    let suffix = if truncated {
                        format!(
                            " — TRUNCATED; call again with offset={end} to continue \
                             (remaining {} bytes)",
                            total_bytes - end,
                        )
                    } else {
                        String::new()
                    };
                    format!("[read_file `{path}`] bytes {offset}..{end} of {total_bytes}{suffix}")
                };
                Ok(ToolResult::ok(format!("{header}\n\n{slice}")))
            }
            Err(err) => Ok(ToolResult::err(format!(
                "read_file: cannot read `{path}`: {err}"
            ))),
        }
    }
}

/// Slice `body` starting at `byte_offset` for `length` bytes. If the
/// computed end lands inside a multi-byte UTF-8 codepoint, snap to
/// the nearest preceding codepoint boundary so we never split a
/// codepoint. Returns a borrowed slice when no snapping was needed.
fn byte_slice_at_char_boundary(body: &str, byte_offset: usize, length: usize) -> &str {
    let raw_end = byte_offset.saturating_add(length).min(body.len());
    // Walk backward from raw_end until we hit a char boundary.
    let mut end = raw_end;
    while end > byte_offset && !body.is_char_boundary(end) {
        end -= 1;
    }
    // The start should already be on a char boundary if the caller
    // is paginating off a prior response's `end`; if it isn't, snap
    // forward so we still return valid UTF-8.
    let mut start = byte_offset.min(body.len());
    while start < end && !body.is_char_boundary(start) {
        start += 1;
    }
    &body[start..end]
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
    fn read_file_returns_full_body_under_the_per_call_cap() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("note.md"), "hello\nworld\n").unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "note.md" }))
            .unwrap();
        assert!(r.ok);
        assert!(r.display.contains("hello"));
        assert!(r.display.contains("world"));
        // Header should advertise total size on a single-call read.
        assert!(r.display.contains("12 bytes"));
        // No truncation marker on a fits-in-one-call read.
        assert!(!r.display.contains("TRUNCATED"));
    }

    #[test]
    fn read_file_clamps_to_max_bytes_and_reports_offset_for_next_call() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 KB body, larger than the 64 KB per-call cap.
        let total = 100_000;
        let body = "x".repeat(total);
        std::fs::write(tmp.path().join("big.txt"), &body).unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "big.txt" }))
            .unwrap();
        assert!(r.ok);
        assert!(r.display.contains("TRUNCATED"));
        assert!(r.display.contains(&format!("of {total}")));
        // Cap is 64K so the next-offset hint should be 65536.
        assert!(
            r.display.contains(&format!("offset={MAX_BYTES_PER_CALL}")),
            "header should hint next offset; got `{}`",
            &r.display.lines().next().unwrap_or(""),
        );
    }

    #[test]
    fn read_file_with_offset_returns_the_tail() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 KB body. Read from offset 64K → returns the remaining
        // 36 KB (under the per-call cap).
        let total = 100_000;
        let body = "x".repeat(total);
        std::fs::write(tmp.path().join("big.txt"), &body).unwrap();
        let r = ReadFileTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "big.txt", "offset": MAX_BYTES_PER_CALL }),
            )
            .unwrap();
        assert!(r.ok);
        // Tail call doesn't TRUNCATE (the remainder fits in one cap).
        assert!(!r.display.contains("TRUNCATED"));
        // Header should report the range.
        assert!(
            r.display
                .contains(&format!("bytes {MAX_BYTES_PER_CALL}..{} of {total}", total)),
            "header should report range; got `{}`",
            &r.display.lines().next().unwrap_or(""),
        );
    }

    #[test]
    fn read_file_explicit_length_clamped_to_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let total = 100_000;
        let body = "x".repeat(total);
        std::fs::write(tmp.path().join("big.txt"), &body).unwrap();
        let r = ReadFileTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "big.txt", "length": 1_000_000 }),
            )
            .unwrap();
        assert!(r.ok);
        // Even though length was 1 MB, only 64 KB was returned.
        assert!(r.display.contains("TRUNCATED"));
        assert!(r.display.contains(&format!("offset={MAX_BYTES_PER_CALL}")));
    }

    #[test]
    fn read_file_offset_past_end_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("note.md"), "hello").unwrap();
        let r = ReadFileTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "note.md", "offset": 100 }),
            )
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("past end"));
    }

    #[test]
    fn read_file_offset_inside_multibyte_codepoint_does_not_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        // Each '✓' is 3 bytes in UTF-8. 5 copies = 15 bytes total.
        // Ask for offset=1 inside the first codepoint; the snapper
        // should advance to the next boundary at byte 3.
        let body = "✓✓✓✓✓";
        std::fs::write(tmp.path().join("utf.txt"), body).unwrap();
        let r = ReadFileTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "utf.txt", "offset": 1 }))
            .unwrap();
        // No panic on the slice → UTF-8 boundary snap worked.
        assert!(r.ok);
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
