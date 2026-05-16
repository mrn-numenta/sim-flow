//! `api_references(query, include_declaration?)` -- list every
//! reference to the named symbol across the foundation workspace.
//! Backed by `textDocument/references`.
//!
//! Useful for "show me how `LaneCtx` is actually consumed by
//! library models" or "where is `Scheduler::tick` called?" --
//! questions the static rustdoc corpus cannot answer because it
//! has no notion of call graph or cross-crate usage.

use serde_json::{Value, json};

use super::api_common::{format_location_value, pick_best_hit, symbol_kind_label};
use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::lsp;
use crate::Result;

pub struct ApiReferencesTool;

const MAX_RENDER: usize = 80;

impl Tool for ApiReferencesTool {
    fn name(&self) -> &'static str {
        "api_references"
    }
    fn description(&self) -> &'static str {
        "List every reference to a foundation framework symbol across the workspace. Backed by rust-analyzer's `textDocument/references`. Use this to understand actual usage patterns -- who calls `Scheduler::tick`? where is `LaneCtx` consumed? Resolves the name via `workspace/symbol` first, preferring exact-name matches. By default the declaration site is included so the agent can orient; pass `include_declaration: false` to exclude it."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name, e.g. `LaneCtx`, `Scheduler`. Exact-name matches are preferred."
                },
                "include_declaration": {
                    "type": "boolean",
                    "description": "Whether to include the symbol's own declaration in the results.",
                    "default": true
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "api_references: missing or empty `query` arg",
                ));
            }
        };
        let include_declaration = args
            .get("include_declaration")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let Some(framework_root) = ctx.framework_root else {
            return Ok(ToolResult::err(
                "api_references: framework root not configured for this project (no `fw:` paths)",
            ));
        };
        let Some(workspace_root) = framework_root.parent().and_then(|p| p.parent()) else {
            return Ok(ToolResult::err(
                "api_references: cannot derive foundation workspace root from framework_root",
            ));
        };
        let workspace_root_owned = workspace_root.to_path_buf();

        let result = lsp::with_client(workspace_root, |c| {
            let raw = c.workspace_symbol(&query)?;
            let Some(hit) = pick_best_hit(&raw, &query) else {
                return Ok(None);
            };
            let refs =
                c.text_document_references(&hit.uri, hit.line, hit.character, include_declaration)?;
            Ok(Some((hit, refs)))
        });

        let (hit, refs) = match result {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                return Ok(ToolResult::ok(format!(
                    "[api_references `{query}`]\n\n(no symbols match)"
                )));
            }
            Err(lsp::LspError::Spawn(e)) => {
                return Ok(ToolResult::err(format!(
                    "api_references: cannot spawn rust-analyzer ({e}). Install it (`rustup component add rust-analyzer` or `brew install rust-analyzer`) or point `{}` at an existing binary.",
                    lsp::RUST_ANALYZER_BIN_ENV,
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_references: rust-analyzer call failed: {e}"
                )));
            }
        };

        Ok(ToolResult::ok(render(
            &query,
            &hit,
            &refs,
            &workspace_root_owned,
        )))
    }
}

fn render(
    query: &str,
    hit: &super::api_common::SymbolHit,
    refs: &Value,
    workspace_root: &std::path::Path,
) -> String {
    let kind = symbol_kind_label(hit.kind);
    let banner = format!(
        "{kind} `{name}` at {loc}",
        name = hit.name,
        loc = super::api_common::format_uri(&hit.uri, Some(hit.line + 1), workspace_root),
    );
    let locations = match refs {
        Value::Array(a) => a.as_slice(),
        Value::Null => return format!("[api_references `{query}`]\n\n{banner}\n\n(no references)"),
        _ => {
            return format!(
                "[api_references `{query}`]\n\n{banner}\n\n(unexpected response shape: {refs})"
            );
        }
    };
    if locations.is_empty() {
        return format!("[api_references `{query}`]\n\n{banner}\n\n(no references)");
    }
    // Dedup by (path, line) so a hit that reports two character
    // ranges on the same line doesn't double-list.
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for loc in locations {
        let (s, line) = format_location_value(loc, workspace_root);
        // Use the formatted location string sans line+col as the
        // key plus the line number for dedup.
        let key = format!("{s}::{line}");
        if seen.insert(key) {
            rows.push(format!("- {s}"));
        }
    }
    let total = rows.len();
    let header = if total > MAX_RENDER {
        format!("{total} references (showing first {MAX_RENDER}):")
    } else {
        format!("{total} reference{}:", if total == 1 { "" } else { "s" })
    };
    let shown = rows.into_iter().take(MAX_RENDER).collect::<Vec<_>>();
    format!(
        "[api_references `{query}`]\n\n{banner}\n\n{header}\n{}",
        shown.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::super::api_common::SymbolHit;
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from("/abs/foundation")
    }

    fn hit() -> SymbolHit {
        SymbolHit {
            name: "LaneCtx".into(),
            kind: 23,
            container: None,
            uri: "file:///abs/foundation/crates/framework/src/lane.rs".into(),
            line: 10,
            character: 0,
        }
    }

    #[test]
    fn empty_refs_renders_no_references() {
        let out = render("LaneCtx", &hit(), &json!([]), &ws());
        assert!(out.contains("(no references)"), "{out}");
    }

    #[test]
    fn dedupes_same_file_same_line() {
        let refs = json!([
            {
                "uri": "file:///abs/foundation/library/foo/src/lib.rs",
                "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 5, "character": 7 } }
            },
            {
                "uri": "file:///abs/foundation/library/foo/src/lib.rs",
                "range": { "start": { "line": 5, "character": 20 }, "end": { "line": 5, "character": 27 } }
            },
            {
                "uri": "file:///abs/foundation/library/foo/src/lib.rs",
                "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 9, "character": 7 } }
            }
        ]);
        let out = render("LaneCtx", &hit(), &refs, &ws());
        // Two distinct lines after dedup.
        assert!(out.contains("2 references:"), "{out}");
        assert_eq!(out.lines().filter(|l| l.starts_with("- ")).count(), 2);
    }

    #[test]
    fn truncates_long_lists() {
        let mut arr = Vec::new();
        for i in 0..(MAX_RENDER + 10) {
            arr.push(json!({
                "uri": format!("file:///abs/foundation/library/m{i}/src/lib.rs"),
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
            }));
        }
        let out = render("LaneCtx", &hit(), &Value::Array(arr), &ws());
        let total = MAX_RENDER + 10;
        assert!(
            out.contains(&format!("{total} references (showing first {MAX_RENDER})")),
            "{out}"
        );
        assert_eq!(
            out.lines().filter(|l| l.starts_with("- ")).count(),
            MAX_RENDER
        );
    }

    #[test]
    fn singular_label_for_one_ref() {
        let refs = json!([{
            "uri": "file:///abs/foundation/library/foo/src/lib.rs",
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 7 } }
        }]);
        let out = render("LaneCtx", &hit(), &refs, &ws());
        assert!(out.contains("1 reference:"), "{out}");
    }
}
