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

use super::{Tool, ToolContext, ToolResult, preview_one_line, resolve_safe_path};
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
        // Refuse to edit through a symlink. read_to_string +
        // fs::write would otherwise follow it transparently and
        // overwrite the symlink target (potentially outside the
        // project directory). See orchestrator audit #6
        // (2026-05-16).
        if let Ok(meta) = std::fs::symlink_metadata(&abs)
            && meta.file_type().is_symlink()
        {
            return Ok(ToolResult::err(format!(
                "edit_file: refusing to edit through symlink `{path}` -- resolve the link manually if you intend to edit the target",
            )));
        }
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
        // Enforce-on-write for `docs/spec.md`: parse + validate the
        // POST-edit content before persisting. Mirrors the
        // `write_file` guard; see `validate_proposed_spec_md` for
        // the contract. An edit that breaks the structured schema
        // is rejected with a structured error and the file on disk
        // is unchanged.
        if path == "docs/spec.md"
            && let Err(spec_err) =
                crate::__internal::session::spec_md::validate_proposed_spec_md(&updated)
        {
            return Ok(ToolResult::err(spec_err.to_agent_message()));
        }
        if let Err(err) = std::fs::write(&abs, updated.as_bytes()) {
            return Ok(ToolResult::err(format!(
                "edit_file: cannot write `{path}`: {err}"
            )));
        }
        // Post-write read-back. Confirms the on-disk content matches
        // what we asked `replacen` to produce. Catches the rare case
        // where a concurrent writer (or a filesystem oddity) clobbers
        // our change between `write` and the next tool call. Also
        // forces the caller to face the actual diff: the success
        // message echoes `old -> new` so a model that emitted a
        // `new_string` accidentally still containing the offending
        // bit can see why its "successful" edit didn't fix the
        // critique finding.
        let on_disk = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "edit_file: post-write read-back failed for `{path}`: {err}"
                )));
            }
        };
        if on_disk != updated {
            return Ok(ToolResult::err(format!(
                "edit_file: post-write verification failed for `{path}`: on-disk content does not match the expected replacement (possible concurrent write). Re-read the file and try again."
            )));
        }
        if let Some(step_id) = ctx.step_id {
            crate::manifest::record_write(ctx.project_dir, step_id, &path);
        }
        let (start_line, end_line) = changed_line_range(&body, &old_string);
        let old_lines = old_string.matches('\n').count() + 1;
        let new_lines = new_string.matches('\n').count() + 1;
        Ok(ToolResult::ok(format!(
            "[edit_file `{path}`] replaced 1 occurrence at line {start_line}-{end_line} ({old_lines} -> {new_lines} line(s)).\n  old: {}\n  new: {}",
            preview_one_line(&old_string, 240),
            preview_one_line(&new_string, 240),
        ))
        .with_touched_path(&path))
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
    fn spec_md_edit_that_breaks_schema_is_rejected_and_reverts() {
        // Agent edits docs/spec.md to break the structured
        // schema (renames `## Purpose` to `## Purpose And Scope`).
        // The validator rejects; file unchanged on disk.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let valid_body = "## Metadata\n\n\n## Purpose\n\n## Scope\n\n## Non-goals\n\n## Assumptions and Constraints\n\n### Quantitative\n\n| Constraint | Value | Source-anchor |\n| --- | --- | --- |\n| Clock frequency | 1 GHz | primary:p1 |\n| Gate budget per cycle | 50 | primary:p1 |\n\n## Blocks\n\n## Functional Behavior\n\n## Timing, Latency, and Throughput\n\n## Pipeline and Hierarchy\n\n## Reset, Initialization, Flush, Drain\n\n## Worked Examples\n\n## Source-Spec Anchors\n\n## Open Questions\n\n## Auto-decisions\n\n";
        std::fs::write(tmp.path().join("docs/spec.md"), valid_body).unwrap();
        let writes = vec!["docs/".to_string()];
        // Concat `## Purpose\n\n## Scope` into the freeform combo
        // that breaks the parser's section-order dispatch.
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({
                    "path": "docs/spec.md",
                    "old_string": "## Purpose\n\n## Scope\n\n## Non-goals",
                    "new_string": "## Purpose And Scope"
                }),
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
        assert_eq!(on_disk, valid_body);
    }

    #[test]
    fn spec_md_edit_that_preserves_schema_succeeds() {
        // Agent edits an existing valid spec.md by changing a
        // quantitative value. The structured schema is preserved
        // so the guard lets the write through.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let valid_body = "## Metadata\n\n\n## Purpose\n\n## Scope\n\n## Non-goals\n\n## Assumptions and Constraints\n\n### Quantitative\n\n| Constraint | Value | Source-anchor |\n| --- | --- | --- |\n| Clock frequency | 1 GHz | primary:p1 |\n| Gate budget per cycle | 50 | primary:p1 |\n\n## Blocks\n\n## Functional Behavior\n\n## Timing, Latency, and Throughput\n\n## Pipeline and Hierarchy\n\n## Reset, Initialization, Flush, Drain\n\n## Worked Examples\n\n## Source-Spec Anchors\n\n## Open Questions\n\n## Auto-decisions\n\n";
        std::fs::write(tmp.path().join("docs/spec.md"), valid_body).unwrap();
        let writes = vec!["docs/".to_string()];
        let result = EditFileTool
            .invoke(
                &ctx(tmp.path(), &writes),
                &json!({
                    "path": "docs/spec.md",
                    "old_string": "1 GHz",
                    "new_string": "2 GHz"
                }),
            )
            .unwrap();
        assert!(result.ok, "expected success, got: {}", result.display);
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/spec.md")).unwrap();
        assert!(on_disk.contains("2 GHz"));
        assert!(!on_disk.contains("1 GHz"));
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
        assert!(
            result.display.contains("old: \"hello\""),
            "missing old preview in: {}",
            result.display
        );
        assert!(
            result.display.contains("new: \"HOWDY\""),
            "missing new preview in: {}",
            result.display
        );
        // edit_file must record the path it touched so the
        // orchestrator's no-progress classifier can recognize the
        // turn as a fix attempt (vs a read / new-file diagnostic).
        assert_eq!(
            result.touched_paths,
            vec!["note.md"],
            "expected touched_paths = [note.md], got {:?}",
            result.touched_paths,
        );
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
