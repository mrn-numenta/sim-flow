//! `api_expand_macro(path, line, character?)` -- show what a macro
//! call expands to at the given position. Backed by
//! rust-analyzer's `rust-analyzer/expandMacro` extension.
//!
//! This is the single biggest win for sim-foundation's derive-heavy
//! surface (`HasLogic`, `HasInstances`, `ConfigModel`,
//! `CheckpointModel`, `SignalTracePayload`, `SignalTraceState`):
//! the static `foundation-docs/api/pages/*.md` corpus cannot show
//! what a derive generates, because that information only exists
//! after the compiler runs the macro. rust-analyzer can.
//!
//! Position semantics match LSP everywhere else in this module:
//! `line` and `character` are 1-based to match what the other
//! `api_*` tools surface; we subtract one before sending to LSP.
//! The cursor needs to land inside a macro invocation -- for
//! derives that's on the derive attribute (e.g. on `HasLogic`
//! inside `#[derive(HasLogic)]`). If rust-analyzer finds no macro
//! at that position it returns null and we surface that as
//! "(no macro at this position)".

use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolResult, resolve_read_path};
use crate::__internal::session::lsp;
use crate::Result;

pub struct ApiExpandMacroTool;

const MAX_DISPLAY_BYTES: usize = 8 * 1024;

impl Tool for ApiExpandMacroTool {
    fn name(&self) -> &'static str {
        "api_expand_macro"
    }
    fn description(&self) -> &'static str {
        "Expand the macro invocation at a given source position and return the generated code. Backed by rust-analyzer's `rust-analyzer/expandMacro` extension. The biggest single win over the static `fw:api/pages/...` docs for sim-foundation, since the docs cannot show what `#[derive(HasLogic)]`, `#[derive(ConfigModel)]`, etc. actually generate. Use `api_search` first to find a struct that derives the macro of interest, then point this tool at its `#[derive(...)]` line. Only files INSIDE the foundation workspace (i.e. `fw:` paths under `crates/`) can be expanded -- rust-analyzer is rooted there and does not see `lib:` or project-relative sources."
    }
    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path", "line"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File containing the macro invocation. MUST live inside the foundation workspace -- use the `fw:<rel>` form returned by `api_search`. `lib:` and project-relative paths point outside the workspace rust-analyzer is rooted at and will be rejected up front."
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number that contains the macro invocation. For derive macros, the line with `#[derive(...)]`.",
                    "minimum": 1
                },
                "character": {
                    "type": "integer",
                    "description": "Optional 1-based column. Defaults to 1 (start of line). Override only if rust-analyzer reports no macro at the default position.",
                    "minimum": 1,
                    "default": 1
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => return Ok(ToolResult::err("api_expand_macro: missing `path` arg")),
        };
        let one_based_line = match args.get("line").and_then(|v| v.as_u64()) {
            Some(n) if n >= 1 => n,
            _ => {
                return Ok(ToolResult::err(
                    "api_expand_macro: `line` must be a 1-based integer >= 1",
                ));
            }
        };
        let one_based_character = args
            .get("character")
            .and_then(|v| v.as_u64())
            .filter(|n| *n >= 1)
            .unwrap_or(1);

        let abs = match resolve_read_path(ctx, &path) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Ok(ToolResult::err(format!(
                    "api_expand_macro: cannot resolve `{path}` (lib:/fw: root not configured?)"
                )));
            }
            Err(e) => return Ok(ToolResult::err(format!("api_expand_macro: {e}"))),
        };
        if !abs.is_file() {
            return Ok(ToolResult::err(format!(
                "api_expand_macro: `{path}` is not a file"
            )));
        }

        let Some(framework_root) = ctx.framework_root else {
            return Ok(ToolResult::err(
                "api_expand_macro: framework root not configured for this project",
            ));
        };
        let Some(workspace_root) = framework_root.parent().and_then(|p| p.parent()) else {
            return Ok(ToolResult::err(
                "api_expand_macro: cannot derive foundation workspace root from framework_root",
            ));
        };

        // rust-analyzer is rooted at `workspace_root` (sim-foundation).
        // It only sees workspace-member crates -- the framework crate
        // and its siblings under `crates/`. Files outside that tree
        // (lib: paths, project sources, bare paths into a sim-models
        // project) are invisible to it and the request would return
        // null with no actionable error. Reject up front so the agent
        // sees a clear message instead of a misleading
        // "(no macro at this position)".
        let canon_abs = abs.canonicalize().unwrap_or_else(|_| abs.clone());
        let canon_workspace = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        if !canon_abs.starts_with(&canon_workspace) {
            return Ok(ToolResult::err(format!(
                "api_expand_macro: `{path}` resolves to `{}`, which is outside the foundation workspace at `{}`. rust-analyzer is rooted at the foundation workspace and cannot see this file; only `fw:` paths inside the foundation tree are supported.",
                canon_abs.display(),
                canon_workspace.display(),
            )));
        }

        let uri = match lsp::path_to_uri(&abs) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("api_expand_macro: {e}"))),
        };

        let lsp_line = one_based_line - 1;
        let lsp_char = one_based_character - 1;
        let resp = match lsp::with_client(workspace_root, |c| {
            c.rust_analyzer_expand_macro(&uri, lsp_line, lsp_char)
        }) {
            Ok(v) => v,
            Err(lsp::LspError::Spawn(e)) => {
                return Ok(ToolResult::err(format!(
                    "api_expand_macro: cannot spawn rust-analyzer ({e}). Install it (`rustup component add rust-analyzer` or `brew install rust-analyzer`) or point `{}` at an existing binary.",
                    lsp::RUST_ANALYZER_BIN_ENV,
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_expand_macro: rust-analyzer call failed: {e}"
                )));
            }
        };

        Ok(ToolResult::ok(render(
            &path,
            one_based_line,
            one_based_character,
            &resp,
        )))
    }
}

fn render(path: &str, line: u64, character: u64, resp: &Value) -> String {
    let header = format!("[api_expand_macro `{path}:{line}:{character}`]");
    let Some(obj) = resp.as_object() else {
        return format!("{header}\n\n(no macro at this position)");
    };
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");
    let expansion = obj.get("expansion").and_then(|v| v.as_str()).unwrap_or("");
    let body = if expansion.len() > MAX_DISPLAY_BYTES {
        format!(
            "{}\n... (truncated; expansion is {} bytes total)",
            truncate_at_char_boundary(expansion, MAX_DISPLAY_BYTES),
            expansion.len()
        )
    } else {
        expansion.to_string()
    };
    format!("{header}\n\nMacro `{name}` expands to:\n\n```rust\n{body}\n```")
}

/// Truncate `s` to at most `max_bytes`, walking back to the nearest
/// `char` boundary so we never split a multi-byte codepoint. `&str`
/// slicing panics when the index isn't on a boundary, so a naive
/// `&s[..max_bytes]` against an expansion that contains a non-ASCII
/// char straddling the cut point would crash the tool. Macro
/// expansions for Rust code are usually ASCII, but anything that
/// inlines a `#[doc = "..."]` string or stringifies a value can
/// land non-ASCII inside the cut window.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_null_response_as_no_macro() {
        let out = render("fw:examples/x/src/a.rs", 12, 1, &Value::Null);
        assert!(out.contains("(no macro at this position)"), "{out}");
        assert!(out.contains("fw:examples/x/src/a.rs:12:1"), "{out}");
    }

    #[test]
    fn renders_expansion_in_fenced_rust_block() {
        let resp = json!({
            "name": "HasLogic",
            "expansion": "impl ::foundation_framework::model::HasLogic for Foo { ... }",
        });
        let out = render("fw:examples/x/src/a.rs", 7, 1, &resp);
        assert!(out.contains("Macro `HasLogic` expands to:"), "{out}");
        assert!(out.contains("```rust"), "{out}");
        assert!(
            out.contains("impl ::foundation_framework::model::HasLogic"),
            "{out}"
        );
    }

    #[test]
    fn truncates_oversized_expansions() {
        let big = "a".repeat(MAX_DISPLAY_BYTES + 100);
        let resp = json!({ "name": "Macro", "expansion": big });
        let out = render("fw:a.rs", 1, 1, &resp);
        assert!(out.contains("(truncated;"), "{out}");
        assert!(
            out.contains(&format!("{} bytes total", MAX_DISPLAY_BYTES + 100)),
            "{out}"
        );
    }

    #[test]
    fn truncate_at_char_boundary_no_panic_on_multibyte_cut() {
        // Build a string of ASCII 'a's, then plant a 3-byte char
        // ("…", U+2026) so it straddles the cut point. Naive
        // `&s[..MAX_DISPLAY_BYTES]` would panic; the helper must not.
        let mut s = "a".repeat(MAX_DISPLAY_BYTES - 1);
        s.push('\u{2026}'); // 3 bytes: 0xE2 0x80 0xA6
        s.push_str(&"b".repeat(100));
        // Sanity: the multibyte char crosses MAX_DISPLAY_BYTES.
        assert!(s.len() > MAX_DISPLAY_BYTES);
        let out = truncate_at_char_boundary(&s, MAX_DISPLAY_BYTES);
        // Walks back to before the multibyte char => length is
        // MAX_DISPLAY_BYTES - 1 (the ASCII prefix).
        assert_eq!(out.len(), MAX_DISPLAY_BYTES - 1);
        // And the result is valid UTF-8 (else the slice itself
        // would have panicked; this is belt-and-braces).
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn truncate_at_char_boundary_returns_full_string_when_under_limit() {
        let s = "hello";
        assert_eq!(truncate_at_char_boundary(s, 100), "hello");
    }

    #[test]
    fn truncate_at_char_boundary_handles_exact_boundary() {
        // 3-byte char placed so its END lands exactly on the cut.
        let mut s = String::new();
        s.push('\u{2026}');
        assert_eq!(s.len(), 3);
        // Cut at 3 is on a boundary (end of the char); should keep
        // the whole string.
        assert_eq!(truncate_at_char_boundary(&s, 3), "\u{2026}");
        // Cut at 1 is mid-char; walks back to 0.
        assert_eq!(truncate_at_char_boundary(&s, 1), "");
    }

    #[test]
    fn render_oversized_expansion_with_multibyte_does_not_panic() {
        // End-to-end: an expansion long enough to trigger
        // truncation, with a multibyte char straddling the cut.
        let mut expansion = "x".repeat(MAX_DISPLAY_BYTES - 1);
        expansion.push('\u{2026}');
        expansion.push_str(&"y".repeat(50));
        let resp = json!({ "name": "Macro", "expansion": expansion });
        // Must not panic.
        let out = render("fw:a.rs", 1, 1, &resp);
        assert!(out.contains("(truncated;"), "{out}");
    }
}
