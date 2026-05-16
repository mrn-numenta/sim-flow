//! `search(pattern: string, path?: string)` - regex search across
//! the project. Uses the existing `regex` crate; no shell-out, so
//! results are reproducible and the orchestrator stays in control of
//! file traversal limits.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult, resolve_read_path};
use crate::Result;

const MAX_HITS: usize = 100;
const MAX_FILES_SCANNED: usize = 4_000;

pub struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &'static str {
        "search"
    }
    fn description(&self) -> &'static str {
        "Regex-search project files for a pattern. Returns up to 100 matches."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern (Rust regex flavor)." },
                "path": {
                    "type": "string",
                    "description": "Optional project-relative directory or file to limit the search. Use `lib:` (or `lib:<rel>`) to search the library root, or `fw:` / `fw:api` to search framework source or normalized API docs. Defaults to the project root."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return Ok(ToolResult::err("search: missing `pattern` arg")),
        };
        let regex = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(err) => return Ok(ToolResult::err(format!("search: invalid regex: {err}"))),
        };
        let scope = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        // Resolve scope: "." → project root; lib:[<rel>] → library
        // root (or library_root/<rel>); anything else → project_dir
        // join under the safety check.
        let (scope_abs, strip_root) = if scope == "." || scope == "./" {
            (ctx.project_dir.to_path_buf(), ctx.project_dir)
        } else {
            match resolve_read_path(ctx, &scope) {
                Ok(Some(p)) => {
                    let strip = if scope.starts_with("lib:") {
                        ctx.library_root.unwrap_or(ctx.project_dir)
                    } else if scope == "fw:api"
                        || scope == "fw:api/"
                        || scope.starts_with("fw:api/")
                    {
                        ctx.framework_docs_root.unwrap_or(ctx.project_dir)
                    } else if scope.starts_with("fw:") {
                        ctx.framework_root
                            .or(ctx.framework_docs_root)
                            .unwrap_or(ctx.project_dir)
                    } else {
                        ctx.project_dir
                    };
                    (p, strip)
                }
                Ok(None) => {
                    return Ok(ToolResult::err(
                        "search: requested `lib:` / `fw:` root is not configured for this project",
                    ));
                }
                Err(err) => {
                    return Ok(ToolResult::err(format!(
                        "search: rejecting unsafe path `{scope}`: {err}"
                    )));
                }
            }
        };

        let mut hits: Vec<String> = Vec::new();
        let mut files_scanned = 0usize;
        for path in walk_files(&scope_abs)? {
            if files_scanned >= MAX_FILES_SCANNED {
                hits.push(format!(
                    "(scan stopped after {MAX_FILES_SCANNED} files; refine `path` to narrow)"
                ));
                break;
            }
            files_scanned += 1;
            // Skip non-UTF8 / huge files.
            let body = match std::fs::read_to_string(&path) {
                Ok(b) if b.len() < 1_048_576 => b,
                _ => continue,
            };
            for (idx, line) in body.lines().enumerate() {
                if regex.is_match(line) {
                    let rel = path
                        .strip_prefix(strip_root)
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    hits.push(format!("{rel}:{}: {line}", idx + 1));
                    if hits.len() >= MAX_HITS {
                        hits.push(format!("(hit cap {MAX_HITS}; refine `pattern` to narrow)"));
                        return Ok(ToolResult::ok(format!(
                            "[search `{pattern}` under `{scope}`]\n\n{}",
                            hits.join("\n")
                        )));
                    }
                }
            }
        }
        if hits.is_empty() {
            Ok(ToolResult::ok(format!(
                "[search `{pattern}` under `{scope}`]\n\n(no matches in {files_scanned} files)"
            )))
        } else {
            Ok(ToolResult::ok(format!(
                "[search `{pattern}` under `{scope}`]\n\n{}",
                hits.join("\n")
            )))
        }
    }
}

/// Recursive walk that skips a small set of conventionally-noisy
/// directories. We don't pull in the `walkdir` crate to keep the
/// dependency surface small.
fn walk_files(start: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    if start.is_file() {
        out.push(start.to_path_buf());
        return Ok(out);
    }
    let mut stack: Vec<PathBuf> = vec![start.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(
                name.as_str(),
                "target" | "node_modules" | ".git" | ".sim-flow"
            ) {
                continue;
            }
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(path),
                Ok(ft) if ft.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None)
    }

    fn write(root: &std::path::Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn missing_pattern_arg_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = SearchTool.invoke(&ctx(tmp.path()), &json!({})).unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing `pattern`"));
    }

    #[test]
    fn invalid_regex_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = SearchTool
            .invoke(&ctx(tmp.path()), &json!({"pattern": "[unclosed"}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("invalid regex"));
    }

    #[test]
    fn search_finds_a_match_in_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.rs", "fn foo() {}\nfn bar() {}\n");
        write(tmp.path(), "b.rs", "fn baz() {}\n");
        let result = SearchTool
            .invoke(&ctx(tmp.path()), &json!({"pattern": "fn foo"}))
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(result.display.contains("a.rs"));
        assert!(result.display.contains("fn foo()"));
    }

    #[test]
    fn search_excludes_target_dir() {
        // The project's `target/` is conventionally enormous and
        // shouldn't appear in search hits. Walk filters it.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/a.rs", "needle here\n");
        write(tmp.path(), "target/debug/deps/x.rs", "needle here\n");
        let result = SearchTool
            .invoke(&ctx(tmp.path()), &json!({"pattern": "needle"}))
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(result.display.contains("src/a.rs"));
        // target/ should not be walked.
        assert!(
            !result.display.contains("target/"),
            "target/ should be excluded; got {}",
            result.display
        );
    }

    #[test]
    fn search_path_arg_scopes_to_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/a.rs", "needle in src\n");
        write(tmp.path(), "docs/b.md", "needle in docs\n");
        let result = SearchTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"pattern": "needle", "path": "docs"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(result.display.contains("docs/b.md"));
        assert!(!result.display.contains("src/a.rs"));
    }

    #[test]
    fn search_returns_no_matches_message_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", "hello\n");
        let result = SearchTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"pattern": "won't-match-anything"}),
            )
            .unwrap();
        assert!(result.ok);
        assert!(
            result.display.contains("0 hit")
                || result.display.contains("(no matches)")
                || result.display.contains("no matches"),
            "expected zero-result message; got {}",
            result.display
        );
    }

    #[test]
    fn search_lib_prefix_without_library_root_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = SearchTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"pattern": "x", "path": "lib:foo"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("not configured"));
    }
}
