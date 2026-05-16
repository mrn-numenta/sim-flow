//! `api_search(query, limit?)` -- look up symbols in the foundation
//! framework workspace by name. Backed by rust-analyzer's
//! `workspace/symbol` request via the shared
//! [`crate::__internal::session::lsp`] client.
//!
//! This is the first tool in the `api_*` family that replaces deep
//! reads against the static `foundation-docs/api/pages/*.md` corpus
//! with live queries. The TOC + curated starting points stay; what
//! changes is that "is there a symbol called `HasLogic`, and where?"
//! becomes one round-trip instead of a TOC search.
//!
//! Path handling: rust-analyzer is rooted at the foundation
//! workspace root (the parent of `crates/framework/`), so result
//! URIs come back as absolute `file://` paths. We render them
//! relative to that root with an `fw:` prefix so the agent can pipe
//! a hit directly into `read_file` if it wants the surrounding
//! source.

use serde_json::{Value, json};

use super::api_common::{format_uri, symbol_kind_label};
use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::lsp;
use crate::Result;

pub struct ApiSearchTool;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

impl Tool for ApiSearchTool {
    fn name(&self) -> &'static str {
        "api_search"
    }
    fn description(&self) -> &'static str {
        "Search the foundation framework workspace for symbols (types, traits, functions, modules) by name. Backed by rust-analyzer over LSP, so results reflect the live workspace, not generated docs. Use this BEFORE reading `fw:api/pages/...md` -- it tells you in one round-trip whether a symbol exists, its kind, and where it's defined. First call per session spawns rust-analyzer and waits for initial indexing (2-3 min on a cold workspace, capped at 5 min); subsequent calls are sub-second."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name or fragment, e.g. `HasLogic`, `LaneCtx`, `mesh`. rust-analyzer matches case-insensitively against the workspace symbol index."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default 20, max 100).",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return Ok(ToolResult::err("api_search: missing or empty `query` arg")),
        };
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);

        let Some(framework_root) = ctx.framework_root else {
            return Ok(ToolResult::err(
                "api_search: framework root not configured for this project (no `fw:` paths)",
            ));
        };
        // framework_root == <foundation>/crates/framework. Two levels
        // up lands on the foundation workspace root where rust-analyzer
        // should be rooted so it sees every foundation crate, not just
        // the framework one.
        let Some(workspace_root) = framework_root.parent().and_then(|p| p.parent()) else {
            return Ok(ToolResult::err(
                "api_search: cannot derive foundation workspace root from framework_root",
            ));
        };

        let raw = match lsp::with_client(workspace_root, |c| c.workspace_symbol(&query)) {
            Ok(v) => v,
            Err(lsp::LspError::Spawn(e)) => {
                return Ok(ToolResult::err(format!(
                    "api_search: cannot spawn rust-analyzer ({e}). Install it (`rustup component add rust-analyzer` or `brew install rust-analyzer`) or point `{}` at an existing binary.",
                    lsp::RUST_ANALYZER_BIN_ENV,
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_search: rust-analyzer call failed: {e}"
                )));
            }
        };

        let workspace_root_owned = workspace_root.to_path_buf();
        let lines = format_results(&raw, limit, &workspace_root_owned);
        Ok(ToolResult::ok(format!("[api_search `{query}`]\n\n{lines}")))
    }
}

/// Render the raw `workspace/symbol` response into a compact
/// `kind  path:line  name [in container]` listing. Accepts both the
/// older `SymbolInformation[]` shape (with `location.uri/range`) and
/// the newer `WorkspaceSymbol[]` shape (with `location.uri` only and
/// no range when the server uses lazy resolution). rust-analyzer
/// emits the older shape today; we tolerate both so a future
/// rust-analyzer doesn't break us silently.
fn format_results(raw: &Value, limit: usize, workspace_root: &std::path::Path) -> String {
    let items = match raw {
        Value::Array(a) => a.as_slice(),
        Value::Null => return "(no results)".to_string(),
        _ => return format!("(unexpected result shape: {raw})"),
    };
    if items.is_empty() {
        return "(no results)".to_string();
    }
    let mut lines = Vec::with_capacity(items.len().min(limit));
    let total = items.len();
    for item in items.iter().take(limit) {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = item
            .get("kind")
            .and_then(|v| v.as_u64())
            .map(symbol_kind_label)
            .unwrap_or("Symbol");
        let container = item
            .get("containerName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let uri = item
            .get("location")
            .and_then(|loc| loc.get("uri"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let line = item
            .get("location")
            .and_then(|loc| loc.get("range"))
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|v| v.as_u64())
            .map(|l| l + 1); // LSP lines are 0-based; agents read 1-based.
        let location = format_uri(uri, line, workspace_root);
        let suffix = match container {
            Some(c) => format!(" in `{c}`"),
            None => String::new(),
        };
        lines.push(format!("- {kind:<10} {location}  `{name}`{suffix}"));
    }
    let header = if total > limit {
        format!("{total} matches (showing first {limit}):\n")
    } else {
        format!("{total} matches:\n")
    };
    format!("{header}{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from("/abs/foundation")
    }

    #[test]
    fn formats_empty_results_as_no_results() {
        assert_eq!(format_results(&json!([]), 20, &ws()), "(no results)");
        assert_eq!(format_results(&Value::Null, 20, &ws()), "(no results)");
    }

    #[test]
    fn formats_a_struct_hit_relative_to_workspace() {
        let raw = json!([{
            "name": "Scheduler",
            "kind": 23,
            "location": {
                "uri": "file:///abs/foundation/crates/framework/src/runtime.rs",
                "range": { "start": { "line": 41, "character": 0 }, "end": { "line": 41, "character": 9 } }
            }
        }]);
        let out = format_results(&raw, 20, &ws());
        assert!(out.contains("Struct"), "{out}");
        assert!(
            out.contains("fw:crates/framework/src/runtime.rs:42"),
            "{out}"
        );
        assert!(out.contains("Scheduler"), "{out}");
    }

    #[test]
    fn includes_container_name_when_present() {
        let raw = json!([{
            "name": "elaborate",
            "kind": 12,
            "containerName": "foundation_framework::model",
            "location": {
                "uri": "file:///abs/foundation/crates/framework/src/model.rs",
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }
            }
        }]);
        let out = format_results(&raw, 20, &ws());
        assert!(out.contains("in `foundation_framework::model`"), "{out}");
    }

    #[test]
    fn truncates_to_limit_and_reports_total() {
        let mut items = Vec::new();
        for i in 0..30u64 {
            items.push(json!({
                "name": format!("Sym{i}"),
                "kind": 12,
                "location": {
                    "uri": "file:///abs/foundation/crates/framework/src/lib.rs",
                    "range": { "start": { "line": i, "character": 0 }, "end": { "line": i, "character": 0 } }
                }
            }));
        }
        let raw = Value::Array(items);
        let out = format_results(&raw, 5, &ws());
        assert!(out.starts_with("30 matches (showing first 5):"), "{out}");
        // Five list items plus the header line.
        assert_eq!(out.lines().filter(|l| l.starts_with("- ")).count(), 5);
    }

    #[test]
    fn invoke_missing_query_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiSearchTool.invoke(&ctx, &json!({})).unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn invoke_whitespace_query_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiSearchTool
            .invoke(&ctx, &json!({"query": "   "}))
            .unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn invoke_without_framework_root_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiSearchTool
            .invoke(&ctx, &json!({"query": "HasLogic"}))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("framework root"));
    }

    #[test]
    fn falls_back_to_raw_path_outside_workspace() {
        let raw = json!([{
            "name": "External",
            "kind": 23,
            "location": {
                "uri": "file:///home/dev/.cargo/registry/foo/src/lib.rs",
                "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 9, "character": 0 } }
            }
        }]);
        let out = format_results(&raw, 20, &ws());
        assert!(
            out.contains("/home/dev/.cargo/registry/foo/src/lib.rs:10"),
            "{out}"
        );
        assert!(!out.contains("fw:"), "{out}");
    }
}
