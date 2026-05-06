//! `edit_file(path, old_string, new_string)` - exact-string replace
//! against an existing project file. The agent uses this for small
//! changes (rename a header, fix a typo, tweak a single value) so the
//! critique-iteration loop doesn't have to re-emit the whole file
//! every turn.
//!
//! Constraints (mirrors the Anthropic Edit-tool contract because that
//! is the shape the major LLMs already know how to drive):
//!
//! - `path` must resolve under the project directory. `lib:` / `fw:`
//!   are read-only and rejected here.
//! - `old_string` must be NON-EMPTY and appear EXACTLY ONCE in the
//!   current file. Zero matches -> error (typically a stale string
//!   from an earlier turn). Multiple matches -> error (caller should
//!   add surrounding context until the string is unique).
//! - `old_string` and `new_string` must differ; otherwise the call
//!   is a no-op and we reject so the agent doesn't spin.
//!
//! Returns the line range that changed so the chat UI / next-turn
//! context have a concrete pointer to inspect.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_safe_path};
use crate::Result;
use crate::steps::is_path_allowed_for_writes;

pub struct EditFileTool;

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> &'static str {
        "Replace one occurrence of `old_string` with `new_string` in a project file. Use for small targeted edits instead of rewriting the entire file. `old_string` must appear exactly once; include surrounding context to make it unique."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path", "old_string", "new_string"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative file path. `lib:` / `fw:` (read-only roots) are rejected."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact substring to replace. Must be non-empty and appear exactly once in the current file."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text. Must differ from `old_string`."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("edit_file: missing `path` arg")),
        };
        if path.starts_with("lib:") || path.starts_with("fw:") {
            return Ok(ToolResult::err(
                "edit_file: `lib:` and `fw:` are read-only roots; cannot edit",
            ));
        }
        if !is_path_allowed_for_writes(ctx.write_paths, &path) {
            return Ok(ToolResult::err(format!(
                "edit_file: `{path}` is outside the write allowlist for this step+kind. Allowed: {}.",
                if ctx.write_paths.is_empty() {
                    "(none)".to_string()
                } else {
                    ctx.write_paths.join(", ")
                },
            )));
        }
        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("edit_file: missing `old_string` arg")),
        };
        if old_string.is_empty() {
            return Ok(ToolResult::err(
                "edit_file: `old_string` must be non-empty (use write_file or the artifact-write convention to create a new file)",
            ));
        }
        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("edit_file: missing `new_string` arg")),
        };
        if old_string == new_string {
            return Ok(ToolResult::err(
                "edit_file: `old_string` and `new_string` are identical; nothing to change",
            ));
        }
        let abs = match resolve_safe_path(ctx.project_dir, &path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("{e}"))),
        };
        let body = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "edit_file: cannot read `{path}`: {err}"
                )));
            }
        };
        let match_count = body.matches(&old_string).count();
        if match_count == 0 {
            return Ok(ToolResult::err(format!(
                "edit_file: `old_string` not found in `{path}`. Read the file and copy the exact text (including whitespace) you want to replace."
            )));
        }
        if match_count > 1 {
            return Ok(ToolResult::err(format!(
                "edit_file: `old_string` matches {match_count} times in `{path}`. Add surrounding context until the substring is unique."
            )));
        }
        let updated = body.replacen(&old_string, &new_string, 1);
        if let Err(err) = std::fs::write(&abs, updated.as_bytes()) {
            return Ok(ToolResult::err(format!(
                "edit_file: cannot write `{path}`: {err}"
            )));
        }
        let (start_line, end_line) = changed_line_range(&body, &old_string);
        let new_lines = new_string.matches('\n').count() + 1;
        Ok(ToolResult::ok(format!(
            "[edit_file `{path}`] replaced 1 occurrence at line {start_line}-{end_line} ({} -> {} line(s)).",
            old_string.matches('\n').count() + 1,
            new_lines,
        )))
    }
}

/// Return the 1-based line range the matched `old_string` covers in
/// `body`. Used purely for the human-readable result blurb -- the
/// agent doesn't depend on it for correctness.
fn changed_line_range(body: &str, old_string: &str) -> (usize, usize) {
    let Some(byte_offset) = body.find(old_string) else {
        return (0, 0);
    };
    let prefix = &body[..byte_offset];
    let start_line = prefix.matches('\n').count() + 1;
    let end_line = start_line + old_string.matches('\n').count();
    (start_line, end_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path, write_paths: &'a [String]) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None).with_write_paths(write_paths)
    }

    fn root_writes() -> Vec<String> {
        vec![
            "note.md".to_string(),
            "n.md".to_string(),
            "a.txt".to_string(),
        ]
    }

    #[test]
    fn replaces_a_unique_substring() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("note.md"), "hello world\nbye world\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({"path": "note.md", "old_string": "hello", "new_string": "HOWDY"}),
            )
            .unwrap();
        assert!(result.ok, "expected ok, got: {}", result.display);
        let body = std::fs::read_to_string(tmp.path().join("note.md")).unwrap();
        assert_eq!(body, "HOWDY world\nbye world\n");
        assert!(result.display.contains("line 1-1"));
    }

    #[test]
    fn replaces_a_multiline_substring() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({
                    "path": "a.txt",
                    "old_string": "alpha\nbeta",
                    "new_string": "A\nB\nB2",
                }),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "A\nB\nB2\ngamma\n"
        );
        assert!(result.display.contains("line 1-2"));
    }

    #[test]
    fn rejects_zero_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("n.md"), "hello\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({"path": "n.md", "old_string": "missing", "new_string": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("not found"));
    }

    #[test]
    fn rejects_multiple_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("n.md"), "hi\nhi\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({"path": "n.md", "old_string": "hi", "new_string": "yo"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("matches 2 times"));
    }

    #[test]
    fn rejects_empty_old_string() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("n.md"), "hi\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({"path": "n.md", "old_string": "", "new_string": "x"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("non-empty"));
    }

    #[test]
    fn rejects_identical_strings() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("n.md"), "hello\n").unwrap();
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &root_writes()),
                &json!({"path": "n.md", "old_string": "hello", "new_string": "hello"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("identical"));
    }

    #[test]
    fn rejects_lib_and_fw_paths() {
        let tmp = tempfile::tempdir().unwrap();
        for path in ["lib:foo.md", "fw:src/lib.rs"] {
            let result = EditFileTool
                .invoke(
                    &ctx(tmp.path(), &root_writes()),
                    &json!({"path": path, "old_string": "x", "new_string": "y"}),
                )
                .unwrap();
            assert!(!result.ok, "{path} should be rejected");
            assert!(result.display.contains("read-only"));
        }
    }
}
