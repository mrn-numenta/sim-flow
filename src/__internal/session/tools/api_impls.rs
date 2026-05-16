//! `api_impls(query)` -- list every `impl` of the named trait, or
//! every concrete instantiation when the symbol is a generic type.
//! Backed by `textDocument/implementation`.
//!
//! This is one of the discovery questions the static rustdoc
//! corpus genuinely cannot answer: the markdown pages list each
//! trait once at its declaration site, but they don't enumerate
//! cross-crate impls. rust-analyzer's symbol index does, so we
//! ask it.

use serde_json::{Value, json};

use super::api_common::{format_location_value, pick_best_hit, symbol_kind_label};
use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::lsp;
use crate::Result;

pub struct ApiImplsTool;

const MAX_RENDER: usize = 80;

impl Tool for ApiImplsTool {
    fn name(&self) -> &'static str {
        "api_impls"
    }
    fn description(&self) -> &'static str {
        "List every `impl` of a foundation framework trait (or concrete instantiation of a generic type) by name. Backed by rust-analyzer's `textDocument/implementation`. Answers questions the static `fw:api/pages/...` docs cannot: who implements `HasLogic`? which types implement `ConfigModel`? Resolves the name via `workspace/symbol` first; prefers exact-name matches and falls back to the first match otherwise."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Trait or type name, e.g. `HasLogic`, `ConfigModel`. Exact-name matches are preferred."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return Ok(ToolResult::err("api_impls: missing or empty `query` arg")),
        };

        let Some(framework_root) = ctx.framework_root else {
            return Ok(ToolResult::err(
                "api_impls: framework root not configured for this project (no `fw:` paths)",
            ));
        };
        let Some(workspace_root) = framework_root.parent().and_then(|p| p.parent()) else {
            return Ok(ToolResult::err(
                "api_impls: cannot derive foundation workspace root from framework_root",
            ));
        };
        let workspace_root_owned = workspace_root.to_path_buf();

        let result = lsp::with_client(workspace_root, |c| {
            let raw = c.workspace_symbol(&query)?;
            let Some(hit) = pick_best_hit(&raw, &query) else {
                return Ok(None);
            };
            let impls = c.text_document_implementation(&hit.uri, hit.line, hit.character)?;
            Ok(Some((hit, impls)))
        });

        let (hit, impls) = match result {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                return Ok(ToolResult::ok(format!(
                    "[api_impls `{query}`]\n\n(no symbols match)"
                )));
            }
            Err(lsp::LspError::Spawn(e)) => {
                return Ok(ToolResult::err(format!(
                    "api_impls: cannot spawn rust-analyzer ({e}). Install it (`rustup component add rust-analyzer` or `brew install rust-analyzer`) or point `{}` at an existing binary.",
                    lsp::RUST_ANALYZER_BIN_ENV,
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_impls: rust-analyzer call failed: {e}"
                )));
            }
        };

        Ok(ToolResult::ok(render(
            &query,
            &hit,
            &impls,
            &workspace_root_owned,
        )))
    }
}

fn render(
    query: &str,
    hit: &super::api_common::SymbolHit,
    impls: &Value,
    workspace_root: &std::path::Path,
) -> String {
    let kind = symbol_kind_label(hit.kind);
    let banner = format!(
        "{kind} `{name}` at {loc}",
        name = hit.name,
        loc = super::api_common::format_uri(&hit.uri, Some(hit.line + 1), workspace_root),
    );
    let locations = match impls {
        Value::Array(a) => a.as_slice(),
        Value::Null => return format!("[api_impls `{query}`]\n\n{banner}\n\n(no impls)"),
        _ => {
            return format!(
                "[api_impls `{query}`]\n\n{banner}\n\n(unexpected response shape: {impls})"
            );
        }
    };
    if locations.is_empty() {
        return format!("[api_impls `{query}`]\n\n{banner}\n\n(no impls)");
    }
    let total = locations.len();
    let lines: Vec<String> = locations
        .iter()
        .take(MAX_RENDER)
        .map(|loc| {
            let (s, _line) = format_location_value(loc, workspace_root);
            format!("- {s}")
        })
        .collect();
    let header = if total > MAX_RENDER {
        format!("{total} impls (showing first {MAX_RENDER}):")
    } else {
        format!("{total} impl{}:", if total == 1 { "" } else { "s" })
    };
    format!(
        "[api_impls `{query}`]\n\n{banner}\n\n{header}\n{}",
        lines.join("\n")
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
            name: "HasLogic".into(),
            kind: 11,
            container: None,
            uri: "file:///abs/foundation/crates/framework/src/model/dataflow.rs".into(),
            line: 640,
            character: 10,
        }
    }

    #[test]
    fn renders_empty_impl_list() {
        let out = render("HasLogic", &hit(), &json!([]), &ws());
        assert!(out.contains("(no impls)"), "{out}");
    }

    #[test]
    fn renders_null_impl_response_as_empty() {
        let out = render("HasLogic", &hit(), &Value::Null, &ws());
        assert!(out.contains("(no impls)"), "{out}");
    }

    #[test]
    fn renders_impl_locations_with_fw_prefix() {
        let impls = json!([
            {
                "uri": "file:///abs/foundation/crates/framework/src/model_a.rs",
                "range": { "start": { "line": 10, "character": 0 }, "end": { "line": 10, "character": 1 } }
            },
            {
                "uri": "file:///abs/foundation/library/foo/src/lib.rs",
                "range": { "start": { "line": 41, "character": 0 }, "end": { "line": 41, "character": 1 } }
            }
        ]);
        let out = render("HasLogic", &hit(), &impls, &ws());
        assert!(out.contains("2 impls:"), "{out}");
        assert!(
            out.contains("fw:crates/framework/src/model_a.rs:11"),
            "{out}"
        );
        assert!(out.contains("fw:library/foo/src/lib.rs:42"), "{out}");
    }

    #[test]
    fn truncates_long_lists() {
        let mut arr = Vec::new();
        for i in 0..(MAX_RENDER + 5) {
            arr.push(json!({
                "uri": format!("file:///abs/foundation/crates/framework/src/m{i}.rs"),
                "range": { "start": { "line": i, "character": 0 }, "end": { "line": i, "character": 1 } }
            }));
        }
        let out = render("HasLogic", &hit(), &Value::Array(arr), &ws());
        let expected_total = MAX_RENDER + 5;
        assert!(
            out.contains(&format!(
                "{expected_total} impls (showing first {MAX_RENDER})"
            )),
            "{out}"
        );
        assert_eq!(
            out.lines().filter(|l| l.starts_with("- ")).count(),
            MAX_RENDER
        );
    }

    #[test]
    fn invoke_missing_query_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiImplsTool.invoke(&ctx, &json!({})).unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn invoke_whitespace_query_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiImplsTool.invoke(&ctx, &json!({"query": "   "})).unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn invoke_without_framework_root_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = ApiImplsTool
            .invoke(&ctx, &json!({"query": "HasLogic"}))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("framework root"));
    }

    #[test]
    fn singular_label_for_one_impl() {
        let impls = json!([{
            "uri": "file:///abs/foundation/crates/framework/src/a.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
        }]);
        let out = render("HasLogic", &hit(), &impls, &ws());
        assert!(out.contains("1 impl:"), "{out}");
    }
}
