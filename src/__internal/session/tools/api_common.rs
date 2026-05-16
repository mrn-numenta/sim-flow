//! Shared helpers for the `api_*` LSP-backed discovery tools.
//!
//! Each `api_*` tool starts the same way: take a symbol name, call
//! `workspace/symbol`, pick the best match, then issue a second
//! LSP request at that location (hover / implementation /
//! references / macro expansion). The plumbing is identical, so it
//! lives here instead of being copied into each tool. Tool-specific
//! response formatting stays in the tool's own module.

use std::path::Path;

use serde_json::Value;

/// One match from a `workspace/symbol` response, normalized into
/// the fields we need to (a) form a `textDocument/*` follow-up
/// request and (b) render the result for the agent.
#[derive(Debug, Clone)]
pub struct SymbolHit {
    pub name: String,
    pub kind: u64,
    pub container: Option<String>,
    pub uri: String,
    pub line: u64,
    pub character: u64,
}

/// Pick the most useful entry from a `workspace/symbol` response.
/// Exact name match wins; otherwise the first hit is returned.
/// We never look past the first 50 entries -- if the agent's
/// query was so broad that the right answer is past #50, the
/// right move is to refine via `api_search` first.
///
/// Items whose shape `extract_hit` can't parse are skipped rather
/// than collapsing the whole response to `None` -- a single
/// malformed entry mid-list must not hide the 49 valid hits next
/// to it.
pub fn pick_best_hit(raw: &Value, query: &str) -> Option<SymbolHit> {
    let items = raw.as_array()?;
    let mut first: Option<SymbolHit> = None;
    for item in items.iter().take(50) {
        let Some(hit) = extract_hit(item) else {
            continue;
        };
        if hit.name == query {
            return Some(hit);
        }
        if first.is_none() {
            first = Some(hit);
        }
    }
    first
}

pub fn extract_hit(item: &Value) -> Option<SymbolHit> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item.get("kind")?.as_u64()?;
    let container = item
        .get("containerName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let location = item.get("location")?;
    let uri = location.get("uri")?.as_str()?.to_string();
    // LSP 3.17 `WorkspaceSymbol.location` can be either `Location`
    // (uri + range) or `{ uri }` alone when the server supports
    // lazy resolution. rust-analyzer always sends the full form
    // today because we don't advertise
    // `workspaceSymbol.resolveSupport`, but if it ever switches we
    // don't want every hit to fall out of `pick_best_hit`.
    // Default to (0, 0) when the range is absent so follow-up
    // textDocument/* calls land at the top of the file -- imperfect
    // but better than silently dropping the symbol. See
    // LSP-discovery post-impl critique #10 (2026-05-16).
    let (line, character) = match location.get("range").and_then(|r| r.get("start")) {
        Some(start) => (
            start.get("line").and_then(|v| v.as_u64()).unwrap_or(0),
            start.get("character").and_then(|v| v.as_u64()).unwrap_or(0),
        ),
        None => (0, 0),
    };
    Some(SymbolHit {
        name,
        kind,
        container,
        uri,
        line,
        character,
    })
}

/// Render an LSP `Location` (uri + range.start) as an `fw:<rel>:<line>`
/// reference when it falls inside `workspace_root`, or fall back to
/// the absolute path when it doesn't. Accepts the `Location` object
/// directly so callers don't have to unwrap. Returns
/// `(formatted, one_based_line)` -- callers occasionally want the
/// line number separately for sorting / dedup.
pub fn format_location_value(loc: &Value, workspace_root: &Path) -> (String, u64) {
    let uri = loc.get("uri").and_then(|v| v.as_str()).unwrap_or("");
    let line = loc
        .get("range")
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|v| v.as_u64())
        .map(|l| l + 1)
        .unwrap_or(0);
    (format_uri(uri, Some(line), workspace_root), line)
}

/// Render a `file://` URI + optional 1-based line as a token the
/// agent can paste into `read_file`: `fw:<rel>:<line>` when the path
/// lives under `workspace_root`, raw `<path>:<line>` otherwise.
/// `None` for `line` omits the suffix.
pub fn format_uri(uri: &str, one_based_line: Option<u64>, workspace_root: &Path) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let abs = std::path::PathBuf::from(path);
    let rel = abs.strip_prefix(workspace_root).ok();
    let display: String = match rel {
        Some(r) => format!("fw:{}", r.to_string_lossy()),
        None => path.to_string(),
    };
    match one_based_line {
        Some(n) if n > 0 => format!("`{display}:{n}`"),
        _ => format!("`{display}`"),
    }
}

/// LSP `SymbolKind` enum -> short human label. The full LSP set
/// (1..=26) is in the spec; rust-analyzer emits a subset. Anything
/// unrecognized renders as `Symbol`.
pub fn symbol_kind_label(kind: u64) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Trait",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParam",
        _ => "Symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from("/abs/foundation")
    }

    fn sym(name: &str, kind: u64, path: &str, line: u64) -> Value {
        json!({
            "name": name,
            "kind": kind,
            "location": {
                "uri": format!("file://{path}"),
                "range": {
                    "start": { "line": line, "character": 0 },
                    "end": { "line": line, "character": name.len() as u64 }
                }
            }
        })
    }

    #[test]
    fn exact_name_match_wins() {
        let raw = json!([
            sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1),
            sym("HasLogic", 11, "/abs/foundation/b.rs", 5),
        ]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        assert_eq!(hit.name, "HasLogic");
        assert_eq!(hit.kind, 11);
    }

    #[test]
    fn falls_back_to_first_when_no_exact() {
        let raw = json!([sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1),]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        assert_eq!(hit.name, "HasLogicFor");
    }

    #[test]
    fn skips_malformed_entries_without_dropping_valid_ones() {
        // A malformed entry in the middle (missing `kind`) must
        // not erase the valid entries around it. Before the fix,
        // `extract_hit`'s `?` propagated None out of pick_best_hit
        // on the first bad item, swallowing the 49 valid hits next
        // to it.
        let raw = json!([
            { "name": "MalformedNoKind", "location": { "uri": "file:///abs/foundation/x.rs", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } } } },
            sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1),
            { "name": "MalformedNoLocation", "kind": 12 },
            sym("HasLogic", 11, "/abs/foundation/b.rs", 5),
        ]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        // Exact match should still win even though malformed entries
        // appear earlier in the list.
        assert_eq!(hit.name, "HasLogic");
        assert_eq!(hit.kind, 11);
    }

    #[test]
    fn extracts_hit_with_missing_range_at_origin() {
        // LSP 3.17 WorkspaceSymbol may omit `range` under lazy
        // resolve. extract_hit should still return Some with
        // (0, 0) so the symbol isn't dropped silently.
        let raw = serde_json::json!({
            "name": "LazyResolved",
            "kind": 23,
            "location": { "uri": "file:///abs/foundation/a.rs" }
        });
        let hit = extract_hit(&raw).expect("hit");
        assert_eq!(hit.name, "LazyResolved");
        assert_eq!(hit.line, 0);
        assert_eq!(hit.character, 0);
    }

    #[test]
    fn skips_leading_malformed_when_no_exact_match() {
        let raw = json!([
            { "name": "Malformed" },
            sym("HasLogicFor", 12, "/abs/foundation/a.rs", 1),
        ]);
        let hit = pick_best_hit(&raw, "HasLogic").expect("hit");
        assert_eq!(hit.name, "HasLogicFor");
    }

    #[test]
    fn format_uri_uses_fw_prefix_when_under_workspace() {
        let out = format_uri(
            "file:///abs/foundation/crates/framework/src/lib.rs",
            Some(10),
            &ws(),
        );
        assert_eq!(out, "`fw:crates/framework/src/lib.rs:10`");
    }

    #[test]
    fn format_uri_falls_back_outside_workspace() {
        let out = format_uri("file:///opt/other/lib.rs", Some(3), &ws());
        assert_eq!(out, "`/opt/other/lib.rs:3`");
    }

    #[test]
    fn format_uri_omits_line_when_none_or_zero() {
        assert_eq!(
            format_uri("file:///abs/foundation/a.rs", None, &ws()),
            "`fw:a.rs`"
        );
        assert_eq!(
            format_uri("file:///abs/foundation/a.rs", Some(0), &ws()),
            "`fw:a.rs`"
        );
    }

    #[test]
    fn symbol_kind_label_covers_lsp_kinds_and_falls_back_for_unknown() {
        // Spot-check every named kind in the dense LSP enum -- a
        // sloppy `match` would silently fall through and the
        // dashboard would render "Symbol" for everything.
        for (k, expected) in [
            (1u64, "File"),
            (2, "Module"),
            (5, "Class"),
            (6, "Method"),
            (10, "Enum"),
            (11, "Trait"),
            (12, "Function"),
            (13, "Variable"),
            (14, "Constant"),
            (22, "EnumMember"),
            (23, "Struct"),
            (26, "TypeParam"),
        ] {
            assert_eq!(symbol_kind_label(k), expected, "kind {k}");
        }
        // Unknown / out-of-range -> generic fallback label.
        assert_eq!(symbol_kind_label(999), "Symbol");
        assert_eq!(symbol_kind_label(0), "Symbol");
    }

    #[test]
    fn format_location_value_returns_line_for_dedup() {
        let loc = json!({
            "uri": "file:///abs/foundation/a.rs",
            "range": { "start": { "line": 41, "character": 0 }, "end": { "line": 41, "character": 1 } }
        });
        let (s, n) = format_location_value(&loc, &ws());
        assert_eq!(n, 42);
        assert!(s.ends_with(":42`"), "{s}");
    }
}
