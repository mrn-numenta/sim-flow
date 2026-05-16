//! `api_hover(query)` -- return the signature and rustdoc for a
//! named symbol, live from rust-analyzer. Direct replacement for
//! reading `foundation-docs/api/pages/<crate>/<item>.md`.
//!
//! Composes two LSP calls:
//!
//! 1. `workspace/symbol(query)` to resolve the name to a location.
//! 2. `textDocument/hover(uri, line, character)` against the first
//!    exact-name match (or, lacking one, the first match overall).
//!
//! rust-analyzer's hover content is markdown -- typically a
//! fenced Rust signature followed by the item's rustdoc comment --
//! which we surface verbatim. That's the same content the static
//! pages were generated from, just always live.

use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::lsp;
use crate::Result;

pub struct ApiHoverTool;

impl Tool for ApiHoverTool {
    fn name(&self) -> &'static str {
        "api_hover"
    }
    fn description(&self) -> &'static str {
        "Show the signature and rustdoc for a foundation framework symbol by name. Backed by rust-analyzer's `textDocument/hover`, so the content is always live -- this is the direct replacement for reading `fw:api/pages/<crate>/<item>.md`. If the name is ambiguous, hover is returned for the first exact match (or first match overall); use `api_search` to disambiguate."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name, e.g. `HasLogic`, `Scheduler`, `elaborate`. Exact-name matches are preferred over substring matches."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return Ok(ToolResult::err("api_hover: missing or empty `query` arg")),
        };

        let Some(framework_root) = ctx.framework_root else {
            return Ok(ToolResult::err(
                "api_hover: framework root not configured for this project (no `fw:` paths)",
            ));
        };
        let Some(workspace_root) = framework_root.parent().and_then(|p| p.parent()) else {
            return Ok(ToolResult::err(
                "api_hover: cannot derive foundation workspace root from framework_root",
            ));
        };

        let workspace_root_owned = workspace_root.to_path_buf();
        let outcome = lsp::with_client(workspace_root, |c| {
            let raw = c.workspace_symbol(&query)?;
            let Some(hit) = pick_best_hit(&raw, &query) else {
                return Ok(HoverOutcome::NoMatch);
            };
            let hover = c.text_document_hover(&hit.uri, hit.line, hit.character)?;
            Ok(HoverOutcome::Resolved {
                hit,
                total: raw.as_array().map(|a| a.len()).unwrap_or(0),
                hover,
            })
        });

        let outcome = match outcome {
            Ok(o) => o,
            Err(lsp::LspError::Spawn(e)) => {
                return Ok(ToolResult::err(format!(
                    "api_hover: cannot spawn rust-analyzer ({e}). Install it (`rustup component add rust-analyzer` or `brew install rust-analyzer`) or point `{}` at an existing binary.",
                    lsp::RUST_ANALYZER_BIN_ENV,
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_hover: rust-analyzer call failed: {e}"
                )));
            }
        };

        let body = render(&query, &outcome, &workspace_root_owned);
        Ok(ToolResult::ok(body))
    }
}

enum HoverOutcome {
    NoMatch,
    Resolved {
        hit: SymbolHit,
        total: usize,
        hover: Value,
    },
}

#[derive(Debug, Clone)]
struct SymbolHit {
    name: String,
    kind: u64,
    container: Option<String>,
    uri: String,
    line: u64,
    character: u64,
}

/// Pick the most useful entry from a `workspace/symbol` response
/// when the agent passed a name and we need to choose. Preference:
/// exact name match wins; otherwise the first hit. We never look
/// past the first 50 entries -- if the agent's query was so broad
/// that the right answer is past #50, the right move is to refine
/// via `api_search` first.
fn pick_best_hit(raw: &Value, query: &str) -> Option<SymbolHit> {
    let items = raw.as_array()?;
    let scan = items.iter().take(50);
    let mut first: Option<SymbolHit> = None;
    for item in scan {
        let hit = extract_hit(item)?;
        if hit.name == query {
            return Some(hit);
        }
        if first.is_none() {
            first = Some(hit);
        }
    }
    first
}

fn extract_hit(item: &Value) -> Option<SymbolHit> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item.get("kind")?.as_u64()?;
    let container = item
        .get("containerName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let location = item.get("location")?;
    let uri = location.get("uri")?.as_str()?.to_string();
    let start = location.get("range")?.get("start")?;
    let line = start.get("line")?.as_u64()?;
    let character = start.get("character")?.as_u64()?;
    Some(SymbolHit {
        name,
        kind,
        container,
        uri,
        line,
        character,
    })
}

fn render(query: &str, outcome: &HoverOutcome, workspace_root: &std::path::Path) -> String {
    match outcome {
        HoverOutcome::NoMatch => format!("[api_hover `{query}`]\n\n(no results)"),
        HoverOutcome::Resolved { hit, total, hover } => {
            let location = format_location(&hit.uri, hit.line + 1, workspace_root);
            let kind = symbol_kind_label(hit.kind);
            let container = match &hit.container {
                Some(c) => format!(" in `{c}`"),
                None => String::new(),
            };
            let banner = if *total > 1 {
                format!(
                    "{total} matches; hover for {kind} {location} `{name}`{container}",
                    name = hit.name,
                )
            } else {
                format!("{kind} {location} `{name}`{container}", name = hit.name,)
            };
            let body = format_hover_contents(hover);
            format!("[api_hover `{query}`]\n\n{banner}\n\n{body}")
        }
    }
}

/// Stringify the LSP `Hover.contents` field. rust-analyzer always
/// sends `MarkupContent { kind: "markdown", value: "..." }`; we
/// also tolerate the legacy `MarkedString` (raw string or `{
/// language, value }`) shape and `MarkedString[]` for forward
/// compatibility.
fn format_hover_contents(hover: &Value) -> String {
    let contents = match hover.get("contents") {
        Some(c) => c,
        None => return "(no hover content)".to_string(),
    };
    if let Some(text) = contents.as_str() {
        return text.to_string();
    }
    if let Some(obj) = contents.as_object() {
        // MarkupContent: { kind, value }
        if let Some(v) = obj.get("value").and_then(|v| v.as_str()) {
            return v.to_string();
        }
        // MarkedString: { language, value }
        if let (Some(lang), Some(v)) = (
            obj.get("language").and_then(|v| v.as_str()),
            obj.get("value").and_then(|v| v.as_str()),
        ) {
            return format!("```{lang}\n{v}\n```");
        }
    }
    if let Some(arr) = contents.as_array() {
        return arr
            .iter()
            .map(format_marked_string_entry)
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    format!("(unexpected hover shape: {contents})")
}

fn format_marked_string_entry(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(obj) = v.as_object() {
        if let (Some(lang), Some(val)) = (
            obj.get("language").and_then(|v| v.as_str()),
            obj.get("value").and_then(|v| v.as_str()),
        ) {
            return format!("```{lang}\n{val}\n```");
        }
        if let Some(val) = obj.get("value").and_then(|v| v.as_str()) {
            return val.to_string();
        }
    }
    v.to_string()
}

fn format_location(uri: &str, one_based_line: u64, workspace_root: &std::path::Path) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let abs = std::path::PathBuf::from(path);
    if let Ok(rel) = abs.strip_prefix(workspace_root) {
        format!("`fw:{}:{one_based_line}`", rel.to_string_lossy())
    } else {
        format!("`{path}:{one_based_line}`")
    }
}

/// Subset of LSP `SymbolKind` we expect from rust-analyzer; kept
/// inline rather than reusing `api_search`'s copy so each tool's
/// formatting stays self-contained.
fn symbol_kind_label(kind: u64) -> &'static str {
    match kind {
        2 => "Module",
        5 => "Class",
        6 => "Method",
        10 => "Enum",
        11 => "Trait",
        12 => "Function",
        14 => "Constant",
        22 => "EnumMember",
        23 => "Struct",
        _ => "Symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from("/abs/foundation")
    }

    fn sym(
        name: &str,
        kind: u64,
        path: &str,
        line: u64,
        character: u64,
        container: Option<&str>,
    ) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), Value::String(name.into()));
        obj.insert("kind".into(), Value::from(kind));
        if let Some(c) = container {
            obj.insert("containerName".into(), Value::String(c.into()));
        }
        obj.insert(
            "location".into(),
            json!({
                "uri": format!("file://{path}"),
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character + 1 }
                }
            }),
        );
        Value::Object(obj)
    }

    #[test]
    fn exact_name_match_wins_over_substring() {
        let raw = json!([
            sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1, 0, None),
            sym("HasLogic", 11, "/abs/foundation/b.rs", 5, 0, None),
            sym("HasLogicHelper", 12, "/abs/foundation/c.rs", 9, 0, None),
        ]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        assert_eq!(hit.name, "HasLogic");
        assert_eq!(hit.line, 5);
    }

    #[test]
    fn falls_back_to_first_when_no_exact_match() {
        let raw = json!([
            sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1, 0, None),
            sym("HasLogicHelper", 12, "/abs/foundation/c.rs", 9, 0, None),
        ]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        assert_eq!(hit.name, "HasLogicFor");
    }

    #[test]
    fn returns_none_for_empty_results() {
        assert!(pick_best_hit(&json!([]), "HasLogic").is_none());
    }

    #[test]
    fn render_no_match_lists_query() {
        let out = render("Missing", &HoverOutcome::NoMatch, &ws());
        assert!(out.contains("[api_hover `Missing`]"));
        assert!(out.contains("(no results)"));
    }

    #[test]
    fn render_resolved_includes_location_and_signature() {
        let hover = json!({
            "contents": {
                "kind": "markdown",
                "value": "```rust\npub trait HasLogic { ... }\n```\nDoc body."
            }
        });
        let outcome = HoverOutcome::Resolved {
            hit: SymbolHit {
                name: "HasLogic".into(),
                kind: 11,
                container: None,
                uri: "file:///abs/foundation/crates/framework/src/model/dataflow.rs".into(),
                line: 640,
                character: 10,
            },
            total: 3,
            hover,
        };
        let out = render("HasLogic", &outcome, &ws());
        assert!(out.contains("3 matches"), "{out}");
        assert!(
            out.contains("fw:crates/framework/src/model/dataflow.rs:641"),
            "{out}"
        );
        assert!(out.contains("Trait"), "{out}");
        assert!(out.contains("pub trait HasLogic"), "{out}");
        assert!(out.contains("Doc body."), "{out}");
    }

    #[test]
    fn formats_legacy_marked_string_array() {
        let hover = json!({
            "contents": [
                { "language": "rust", "value": "fn foo() -> i32" },
                "Doc body."
            ]
        });
        let s = format_hover_contents(&hover);
        assert!(s.contains("```rust\nfn foo() -> i32\n```"), "{s}");
        assert!(s.contains("Doc body."), "{s}");
    }

    #[test]
    fn renders_singular_banner_when_only_one_match() {
        let outcome = HoverOutcome::Resolved {
            hit: SymbolHit {
                name: "Scheduler".into(),
                kind: 23,
                container: Some("foundation_framework".into()),
                uri: "file:///abs/foundation/crates/framework/src/runtime.rs".into(),
                line: 41,
                character: 0,
            },
            total: 1,
            hover: json!({ "contents": { "kind": "markdown", "value": "pub struct Scheduler" } }),
        };
        let out = render("Scheduler", &outcome, &ws());
        // No "N matches" banner when total == 1.
        assert!(!out.contains("matches"), "{out}");
        assert!(out.contains("Struct"), "{out}");
        assert!(out.contains("in `foundation_framework`"), "{out}");
    }
}
