//! `list_dir(path: string)` - list a project, library, or framework
//! directory.

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
                    "description": "Project-relative directory path (use `.` for the project root), `lib:` / `lib:<rel>` to list the library root, or `fw:` / `fw:<rel>` to list framework assets. Use `fw:api/` for normalized API docs and `fw:src/` for the framework source tree."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("list_dir: missing `path` arg")),
        };
        if path == "fw:" || path == "fw" {
            return Ok(list_framework_roots(ctx));
        }
        // Allow "." as the project root and "lib:" as the library root.
        let abs = if path == "." || path == "./" {
            ctx.project_dir.to_path_buf()
        } else {
            match resolve_read_path(ctx, &path) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return Ok(ToolResult::err(
                        "list_dir: requested `lib:` / `fw:` root is not configured for this project",
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

fn list_framework_roots(ctx: &ToolContext) -> ToolResult {
    let mut rows: Vec<String> = Vec::new();
    if let Some(root) = ctx.framework_root {
        rows.push("- [dir] src".into());
        if root.join("Cargo.toml").is_file() {
            rows.push("- [file] Cargo.toml".into());
        }
    }
    if ctx.framework_docs_root.is_some() {
        rows.push("- [dir] api".into());
    }
    if rows.is_empty() {
        return ToolResult::err(
            "list_dir: requested `fw:` root is not configured for this project",
        );
    }
    rows.sort();
    ToolResult::ok(format!("[list_dir `fw:`]\n\n{}", rows.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_with_project(project: &std::path::Path) -> ToolContext<'_> {
        ToolContext::new(project, None, None, None)
    }

    #[test]
    fn list_dir_missing_path_arg_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project(tmp.path());
        let r = ListDirTool.invoke(&ctx, &json!({})).unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn list_dir_at_project_root_lists_top_level_entries_sorted_with_kind() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        std::fs::write(tmp.path().join("file_a.txt"), "a").unwrap();
        std::fs::write(tmp.path().join("file_b.md"), "b").unwrap();
        let ctx = ctx_with_project(tmp.path());
        let r = ListDirTool.invoke(&ctx, &json!({ "path": "." })).unwrap();
        assert!(r.ok);
        assert!(r.display.contains("[dir] subdir"));
        assert!(r.display.contains("[file] file_a.txt"));
        assert!(r.display.contains("[file] file_b.md"));
        // Sorted: file_a < file_b < subdir (asc).
        let a = r.display.find("file_a.txt").unwrap();
        let b = r.display.find("file_b.md").unwrap();
        assert!(a < b, "sorted alphabetically");
    }

    #[test]
    fn list_dir_unreadable_path_returns_err_with_path_in_message() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project(tmp.path());
        let r = ListDirTool
            .invoke(&ctx, &json!({ "path": "no/such/dir" }))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("no/such/dir"));
    }

    #[test]
    fn list_dir_fw_with_no_framework_root_or_docs_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project(tmp.path()); // framework_root=None, docs=None
        let r = ListDirTool.invoke(&ctx, &json!({ "path": "fw:" })).unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("fw:"));
    }

    #[test]
    fn list_dir_fw_with_framework_root_lists_src_and_optional_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let fw_root = tmp.path().join("fw-root");
        std::fs::create_dir(&fw_root).unwrap();
        std::fs::write(fw_root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let ctx = ToolContext::new(tmp.path(), None, Some(&fw_root), None);
        let r = ListDirTool.invoke(&ctx, &json!({ "path": "fw:" })).unwrap();
        assert!(r.ok);
        assert!(r.display.contains("[dir] src"));
        assert!(r.display.contains("[file] Cargo.toml"));
    }

    #[test]
    fn list_dir_fw_with_docs_root_includes_api_dir_in_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let docs_root = tmp.path().join("docs-root");
        std::fs::create_dir(&docs_root).unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, Some(&docs_root));
        let r = ListDirTool.invoke(&ctx, &json!({ "path": "fw:" })).unwrap();
        assert!(r.ok);
        assert!(r.display.contains("[dir] api"));
    }

    #[test]
    fn list_dir_lib_prefix_with_unconfigured_library_root_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project(tmp.path()); // library_root=None
        let r = ListDirTool
            .invoke(&ctx, &json!({ "path": "lib:" }))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("lib:") || r.display.contains("not configured"));
    }
}
