//! `delete_file(path: string)` -- remove a file under the project
//! directory. Scoped to the active step+kind's `write_paths`
//! allowlist: an agent can only delete what the same step is allowed
//! to write. Refuses absolute paths, traversal, paths outside the
//! allowlist, and directories.
//!
//! Motivation: critique passes can flag orphan files (e.g. a
//! milestone wrote `src/stage_gray.rs` then renamed it to
//! `src/model/stage_gray.rs` leaving a 0-byte stub at the old
//! location). Before this tool existed, the auto loop had no way to
//! resolve such findings -- the model would document an
//! `Auto-decisions` note saying "I can't delete files" and the
//! `Unresolved` finding would plateau through the no-progress cap
//! until the run flipped to manual.
//!
//! Scope rationale -- "files created by this step": the
//! `ToolContext::write_paths` allowlist already encodes "what this
//! step+kind is permitted to touch on disk." Symmetric write/delete
//! permissions keep the mental model simple (you can delete what
//! you can write) and naturally cover the orphan-from-prior-iteration
//! case the user asked about, without per-step state tracking.

use serde_json::json;

use super::{DELETE_SCOPE_VIOLATION_MARKER, Tool, ToolContext, ToolResult, resolve_safe_path};
use crate::Result;
use crate::steps::is_path_allowed_for_writes;

pub struct DeleteFileTool;

impl Tool for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }
    fn description(&self) -> &'static str {
        "Remove a file under the project directory. Scoped to paths this step is allowed to write."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative file path. Must be inside this step's write allowlist."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        const MISSING_PATH_HELP: &str = "delete_file: missing `path` arg. \
             Required shape: `{\"path\": \"<relative-path>\"}`.";
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err(MISSING_PATH_HELP)),
        };
        if path.starts_with("lib:") || path.starts_with("fw:") {
            return Ok(ToolResult::err(
                "delete_file: the library and framework roots are read-only; `lib:` / `fw:` paths cannot be deleted.",
            ));
        }
        // Two paths grant permission to delete:
        //   1. The path falls inside the step+kind write allowlist
        //      (symmetric with write_file / edit_file).
        //   2. The user already approved this exact path via a
        //      scope-override prompt during this session. The
        //      orchestrator populates `approved_deletes` from the
        //      RequestUserInput reply (interactive mode only --
        //      auto mode keeps the silent-refuse behavior the user
        //      explicitly chose).
        let user_approved = ctx.approved_deletes.iter().any(|p| p == &path);
        if !is_path_allowed_for_writes(ctx.write_paths, &path) && !user_approved {
            // Stable prefix so the orchestrator can detect this
            // exact failure mode and emit a scope-override
            // RequestUserInput in interactive mode. The display
            // includes both the marker (for the orchestrator) and
            // the user-readable "Allowed: ..." list (for the agent
            // / chat panel).
            return Ok(ToolResult::err(format!(
                "{DELETE_SCOPE_VIOLATION_MARKER} {path}\n\
                 delete_file: `{path}` is outside the write allowlist for this step+kind. Allowed: {}.",
                if ctx.write_paths.is_empty() {
                    "(none)".to_string()
                } else {
                    ctx.write_paths.join(", ")
                },
            )));
        }
        let abs = match resolve_safe_path(ctx.project_dir, &path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("{e}"))),
        };
        // Refuse to recursively delete directories. The tool exists
        // to clean up stray files (orphans, renamed-source stubs);
        // a directory remove is almost never what the agent means
        // and the blast radius is too easy to mis-estimate.
        match std::fs::symlink_metadata(&abs) {
            Ok(md) if md.is_dir() => {
                return Ok(ToolResult::err(format!(
                    "delete_file: `{path}` is a directory; this tool only removes regular files."
                )));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Pedagogical: the agent thought a file existed but
                // it doesn't. Tell it explicitly so it doesn't loop
                // calling delete_file expecting eventual success.
                return Ok(ToolResult::err(format!(
                    "delete_file: `{path}` does not exist (already gone or never created)."
                )));
            }
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "delete_file: cannot stat `{path}`: {err}"
                )));
            }
        }
        match std::fs::remove_file(&abs) {
            Ok(()) => Ok(ToolResult::ok(format!("[delete_file `{path}`] removed"))),
            Err(err) => Ok(ToolResult::err(format!(
                "delete_file: cannot remove `{path}`: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tools::ToolContext;
    use std::path::Path;

    fn make_ctx<'a>(project_dir: &'a Path, write_paths: &'a [String]) -> ToolContext<'a> {
        ToolContext::new(project_dir, None, None, None).with_write_paths(write_paths)
    }

    #[test]
    fn deletes_file_in_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("src/orphan.rs");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"").unwrap();
        let writes = vec!["src/".to_string()];
        let ctx = make_ctx(tmp.path(), &writes);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "src/orphan.rs" }))
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(!target.exists());
    }

    #[test]
    fn rejects_path_outside_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), b"keep me").unwrap();
        let writes = vec!["src/".to_string()];
        let ctx = make_ctx(tmp.path(), &writes);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "README.md" }))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("write allowlist"));
        assert!(result.display.contains(DELETE_SCOPE_VIOLATION_MARKER));
        assert!(tmp.path().join("README.md").exists());
    }

    #[test]
    fn user_approved_path_bypasses_allowlist() {
        // Simulates the post-RequestUserInput state: the user said
        // "yes" to deleting an out-of-scope file, the orchestrator
        // populated `approved_deletes`, and the next delete attempt
        // succeeds.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), b"prune me").unwrap();
        let writes = vec!["src/".to_string()];
        let approved = vec!["README.md".to_string()];
        let ctx = ToolContext::new(tmp.path(), None, None, None)
            .with_write_paths(&writes)
            .with_approved_deletes(&approved);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "README.md" }))
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(!tmp.path().join("README.md").exists());
    }

    #[test]
    fn refuses_to_remove_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/sub")).unwrap();
        let writes = vec!["src/".to_string()];
        let ctx = make_ctx(tmp.path(), &writes);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "src/sub" }))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("directory"));
        assert!(tmp.path().join("src/sub").exists());
    }

    #[test]
    fn reports_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = vec!["src/".to_string()];
        let ctx = make_ctx(tmp.path(), &writes);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "src/ghost.rs" }))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("does not exist"));
    }

    #[test]
    fn rejects_lib_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let writes = vec!["src/".to_string()];
        let ctx = make_ctx(tmp.path(), &writes);
        let result = DeleteFileTool
            .invoke(&ctx, &serde_json::json!({ "path": "lib:foo.md" }))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("read-only"));
    }
}
