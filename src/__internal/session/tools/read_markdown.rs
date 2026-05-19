//! `read_markdown(path, section?, max_depth?)` — navigate a markdown
//! file by heading rather than by byte range.
//!
//! Two modes:
//!
//! - **Outline mode** (no `section` argument): walks the file's
//!   heading tree and returns a nested bullet list down to
//!   `max_depth` (default 3). Lets the agent learn the file's
//!   structure without pulling the whole body into context.
//!
//! - **Section mode** (`section` argument set): finds the heading
//!   whose text matches `section` (case-insensitive, backticks
//!   stripped) and returns the heading line plus its body up to
//!   the next heading of the same or shallower level. Disambiguates
//!   duplicate headings by accepting a breadcrumb path
//!   (`"Blocks > Block: Instruction Fetch (IF)"`); on ambiguity
//!   returns a structured error listing every match.
//!
//! Per-call body cap matches `read_file`'s `MAX_BYTES_PER_CALL`
//! (64 KB). A section larger than that gets truncated with a
//! "TRUNCATED; switch to read_file with offset=… for the rest"
//! hint, because `read_markdown` doesn't itself page within a
//! section.

use serde_json::json;

use super::read_file::MAX_BYTES_PER_CALL;
use super::{Tool, ToolContext, ToolResult, resolve_read_path};
use crate::Result;

const DEFAULT_OUTLINE_DEPTH: usize = 3;
const MAX_OUTLINE_DEPTH: usize = 6;

pub struct ReadMarkdownTool;

impl Tool for ReadMarkdownTool {
    fn name(&self) -> &'static str {
        "read_markdown"
    }

    fn description(&self) -> &'static str {
        "Navigate a markdown file by heading. With no `section` arg, returns a \
         hierarchical outline (heading tree only) down to `max_depth` (default 3). \
         With `section` set, returns that section's heading + body up to the next \
         heading at the same-or-shallower level. Match is case-insensitive on \
         heading text after stripping `#` markers and backticks; pass a `>`-separated \
         breadcrumb path (e.g. `Blocks > Block: Instruction Fetch (IF)`) to \
         disambiguate when the same heading text appears multiple times. Saves the \
         turns the agent would otherwise spend paginating `read_file` to find the \
         right offset."
    }

    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative markdown path, or `lib:<rel>` / `fw:<rel>` to read from the library / framework roots. Same resolution rules as `read_file`."
                },
                "section": {
                    "type": "string",
                    "description": "Optional heading to fetch. Match is case-insensitive on the heading text (with `#` markers and backticks stripped). To disambiguate duplicate headings, pass a `>`-separated breadcrumb path like `Blocks > Block: Instruction Fetch (IF)`."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_OUTLINE_DEPTH,
                    "description": "Outline-mode depth cap (default 3, max 6). Ignored when `section` is set."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(ToolResult::err("read_markdown: missing `path` arg")),
        };
        let abs = match resolve_read_path(ctx, &path) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Ok(ToolResult::err(
                    "read_markdown: requested `lib:` / `fw:` root is not configured for this project",
                ));
            }
            Err(e) => return Ok(ToolResult::err(format!("{e}"))),
        };
        let body = match std::fs::read_to_string(&abs) {
            Ok(b) => b,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "read_markdown: cannot read `{path}`: {err}"
                )));
            }
        };

        let headings = scan_headings(&body);
        let section_arg = args
            .get("section")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        match section_arg {
            None => {
                let max_depth = args
                    .get("max_depth")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(DEFAULT_OUTLINE_DEPTH)
                    .clamp(1, MAX_OUTLINE_DEPTH);
                Ok(ToolResult::ok(render_outline(&path, &headings, max_depth)))
            }
            Some(needle) => render_section(&path, &body, &headings, &needle),
        }
    }
}

/// A single ATX heading discovered in the file. `start_byte` points
/// at the first byte of the heading line (the leading `#`);
/// `body_end_byte` is the first byte AFTER this heading's body
/// (i.e. the start of the next same-or-shallower heading, or
/// `body.len()` if this section runs to EOF). Computed in a single
/// scan up-front.
#[derive(Debug, Clone)]
struct Heading {
    level: u8,
    /// Cleaned heading text — leading `#`s and backticks stripped,
    /// inner whitespace collapsed. Used for matching and outline
    /// rendering.
    text: String,
    /// Byte offset of the heading line's first character.
    start_byte: usize,
    /// Byte offset just past the end of this heading's body. Equals
    /// the start of the next heading at the same-or-shallower level,
    /// or the file length when no such heading exists.
    body_end_byte: usize,
    /// 1-indexed line number for the heading line. Surfaced in error
    /// messages so the agent can correlate with `read_file` output.
    line_no: usize,
}

/// Walk `body` line by line, identifying ATX headings (`#`..`######`).
/// Lines inside fenced code blocks (` ``` ` or `~~~`) are skipped so
/// a literal `# ...` inside a code sample doesn't get mistaken for a
/// heading. Computes the body-end offset for each heading in a
/// second pass so callers can slice out a section's body in O(1).
fn scan_headings(body: &str) -> Vec<Heading> {
    let mut out: Vec<Heading> = Vec::new();
    let mut in_fence = false;
    let mut byte_offset: usize = 0;
    for (line_idx, line) in body.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        // Code-fence toggle. Fences must start at the beginning of
        // the line (no leading indent in our usage); the trim guards
        // against mixed CRLF/LF endings.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            byte_offset += line.len();
            continue;
        }
        if in_fence {
            byte_offset += line.len();
            continue;
        }
        // ATX heading: 1..6 leading `#` then a space then the text.
        let bytes = trimmed.as_bytes();
        let mut level = 0u8;
        while (level as usize) < bytes.len() && bytes[level as usize] == b'#' && level < 6 {
            level += 1;
        }
        if level == 0 || bytes.get(level as usize) != Some(&b' ') {
            byte_offset += line.len();
            continue;
        }
        let raw = &trimmed[(level as usize + 1)..];
        let text = clean_heading_text(raw);
        out.push(Heading {
            level,
            text,
            start_byte: byte_offset,
            body_end_byte: 0, // filled in below
            line_no: line_idx + 1,
        });
        byte_offset += line.len();
    }
    let total = body.len();
    // Second pass: each heading's body ends at the next heading of
    // the same or shallower level.
    for i in 0..out.len() {
        let mine = out[i].level;
        let mut end = total;
        for h in &out[i + 1..] {
            if h.level <= mine {
                end = h.start_byte;
                break;
            }
        }
        out[i].body_end_byte = end;
    }
    out
}

/// Strip backticks and collapse internal whitespace so a heading
/// like `### Block: \`Instruction Fetch (IF)\`` matches a query
/// passed as plain `Block: Instruction Fetch (IF)`.
fn clean_heading_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for c in s.chars() {
        if c == '`' {
            continue;
        }
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Render an outline-mode response: a hierarchical bullet list of
/// every heading at level <= `max_depth`. Tree structure is encoded
/// by indentation (two spaces per level beyond the file's minimum).
fn render_outline(path: &str, headings: &[Heading], max_depth: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "[read_markdown `{path}` outline, max_depth={max_depth}]\n\n",
    ));
    if headings.is_empty() {
        out.push_str("(no headings found)\n");
        return out;
    }
    // Anchor depth on the file's shallowest heading so a doc that
    // starts at H2 (like spec.md sections) still renders cleanly.
    let min_level = headings.iter().map(|h| h.level).min().unwrap_or(1);
    let mut emitted = 0usize;
    for h in headings {
        let depth = h.level.saturating_sub(min_level) as usize;
        if depth >= max_depth {
            continue;
        }
        let indent = "  ".repeat(depth);
        // Reproduce the original heading-markup so the agent sees
        // `### Block: …` style strings it can echo straight back as
        // a section query.
        let prefix = "#".repeat(h.level as usize);
        out.push_str(&format!("{indent}- `{prefix} {}`\n", h.text));
        emitted += 1;
    }
    let truncated_at_depth = headings
        .iter()
        .any(|h| (h.level.saturating_sub(min_level) as usize) >= max_depth);
    if truncated_at_depth {
        out.push_str(&format!(
            "\n(outline truncated at depth {max_depth}; re-call with max_depth={MAX_OUTLINE_DEPTH} for the full tree)\n",
        ));
    }
    out.push_str(&format!(
        "\n{emitted} heading(s) shown of {} total.\n",
        headings.len()
    ));
    out
}

/// Render a section-mode response. Finds the heading matching
/// `needle` and returns its body. On ambiguity returns a structured
/// error listing every match's breadcrumb path so the agent can
/// re-call with a more specific query.
fn render_section(
    path: &str,
    body: &str,
    headings: &[Heading],
    needle: &str,
) -> Result<ToolResult> {
    let needle_norm = clean_heading_text(needle).to_ascii_lowercase();
    // Treat a `>`-separated needle as a breadcrumb path; the last
    // segment is the heading text the caller is targeting and the
    // earlier segments are ancestor headings the match must match.
    let breadcrumb_segments: Vec<String> = needle_norm
        .split('>')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // Track ancestor indices into `headings` (NOT cloned strings) so
    // we can reach back to the original-case `text` when rendering
    // the disambiguation error. Match lookups use the lowercase
    // counterpart cached in `ancestors_lower`.
    let mut matches: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut ancestors_idx: Vec<usize> = Vec::new();
    let mut ancestors_lower: Vec<String> = Vec::new();
    for (i, h) in headings.iter().enumerate() {
        // Pop ancestors until the stack only contains shallower
        // headings than this one.
        while ancestors_idx.len() >= h.level as usize {
            ancestors_idx.pop();
            ancestors_lower.pop();
        }
        let h_text = h.text.to_ascii_lowercase();
        let mut full: Vec<String> = ancestors_lower.clone();
        full.push(h_text.clone());
        let is_match = if breadcrumb_segments.len() == 1 {
            // Single-segment needle: match the heading text directly.
            h_text == breadcrumb_segments[0]
        } else {
            // Multi-segment needle: the trailing segments must match
            // the tail of `full` exactly. Allows callers to omit the
            // very top heading when it's unique enough.
            full.len() >= breadcrumb_segments.len()
                && full[full.len() - breadcrumb_segments.len()..] == breadcrumb_segments[..]
        };
        if is_match {
            matches.push((i, ancestors_idx.clone()));
        }
        ancestors_idx.push(i);
        ancestors_lower.push(h_text);
    }
    if matches.is_empty() {
        // Surface close-by suggestions: every heading whose cleaned
        // text contains the needle as a substring. Helps the agent
        // spell-correct without going back to outline mode.
        let needle_simple = breadcrumb_segments.last().cloned().unwrap_or_default();
        let suggestions: Vec<String> = headings
            .iter()
            .filter(|h| h.text.to_ascii_lowercase().contains(&needle_simple))
            .take(5)
            .map(|h| format!("`{}`", h.text))
            .collect();
        let hint = if suggestions.is_empty() {
            "no near matches; call without `section` for the full outline.".to_string()
        } else {
            format!("did you mean: {}?", suggestions.join(", "))
        };
        return Ok(ToolResult::err(format!(
            "read_markdown `{path}`: section `{needle}` not found; {hint}"
        )));
    }
    if matches.len() > 1 {
        let breadcrumbs: Vec<String> = matches
            .iter()
            .map(|(idx, anc_idx)| {
                let h = &headings[*idx];
                if anc_idx.is_empty() {
                    format!("`{}` (line {})", h.text, h.line_no)
                } else {
                    let anc_path = anc_idx
                        .iter()
                        .map(|i| headings[*i].text.as_str())
                        .collect::<Vec<_>>()
                        .join(" > ");
                    format!("`{anc_path} > {}` (line {})", h.text, h.line_no)
                }
            })
            .collect();
        return Ok(ToolResult::err(format!(
            "read_markdown `{path}`: section `{needle}` is ambiguous ({} matches). \
             Re-call with a `>`-separated breadcrumb. Candidates: {}",
            matches.len(),
            breadcrumbs.join("; "),
        )));
    }
    let (idx, anc_idx) = &matches[0];
    let h = &headings[*idx];
    let raw_slice = &body[h.start_byte..h.body_end_byte];
    let breadcrumb_display = if anc_idx.is_empty() {
        h.text.clone()
    } else {
        let anc_path = anc_idx
            .iter()
            .map(|i| headings[*i].text.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        format!("{anc_path} > {}", h.text)
    };
    let section_bytes = raw_slice.len();
    let (slice, truncated) = if section_bytes > MAX_BYTES_PER_CALL {
        // Snap end to a char boundary so multi-byte codepoints stay
        // intact (read_file's slicer applies the same rule).
        let mut end = MAX_BYTES_PER_CALL;
        while end > 0 && !raw_slice.is_char_boundary(end) {
            end -= 1;
        }
        (&raw_slice[..end], true)
    } else {
        (raw_slice, false)
    };
    let header = if truncated {
        format!(
            "[read_markdown `{path}` section `{breadcrumb_display}` line={}, {} of {section_bytes} bytes — TRUNCATED; call `read_file` with offset={} for the rest]",
            h.line_no,
            slice.len(),
            h.start_byte + slice.len(),
        )
    } else {
        format!(
            "[read_markdown `{path}` section `{breadcrumb_display}` line={}, {section_bytes} bytes]",
            h.line_no,
        )
    };
    Ok(ToolResult::ok(format!("{header}\n\n{slice}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(project: &std::path::Path) -> ToolContext<'_> {
        ToolContext::new(project, None, None, None)
    }

    fn fixture() -> &'static str {
        "# Title\n\
         \n\
         Intro prose.\n\
         \n\
         ## Metadata\n\
         \n\
         body of metadata\n\
         \n\
         ## Blocks\n\
         \n\
         overview\n\
         \n\
         ### Block: Instruction Fetch (IF)\n\
         \n\
         IF body line A\n\
         IF body line B\n\
         \n\
         ### Block: Pre-Decode (PD)\n\
         \n\
         PD body\n\
         \n\
         ## Parameters\n\
         \n\
         params body\n"
    }

    #[test]
    fn outline_mode_returns_heading_tree() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.md"), fixture()).unwrap();
        let r = ReadMarkdownTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "spec.md" }))
            .unwrap();
        assert!(r.ok, "got `{}`", r.display);
        for hd in [
            "# Title",
            "## Metadata",
            "## Blocks",
            "### Block: Instruction Fetch (IF)",
            "### Block: Pre-Decode (PD)",
            "## Parameters",
        ] {
            assert!(
                r.display.contains(hd),
                "missing `{hd}` in outline:\n{}",
                r.display
            );
        }
    }

    #[test]
    fn outline_max_depth_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.md"), fixture()).unwrap();
        // Depth 2 from H1 → H1 + H2 only; the ### Block headings
        // are at depth 3 and must be hidden.
        let r = ReadMarkdownTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "spec.md", "max_depth": 2 }),
            )
            .unwrap();
        assert!(r.ok);
        assert!(r.display.contains("## Blocks"));
        assert!(!r.display.contains("### Block:"));
        assert!(r.display.contains("outline truncated"));
    }

    #[test]
    fn section_mode_returns_heading_body_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.md"), fixture()).unwrap();
        let r = ReadMarkdownTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "spec.md", "section": "Metadata" }),
            )
            .unwrap();
        assert!(r.ok, "got `{}`", r.display);
        assert!(r.display.contains("## Metadata"));
        assert!(r.display.contains("body of metadata"));
        // The next H2 (`## Blocks`) MUST NOT leak into this slice.
        assert!(!r.display.contains("## Blocks"));
        assert!(!r.display.contains("## Parameters"));
    }

    #[test]
    fn section_mode_breadcrumb_disambiguates_duplicate_headings() {
        // Both `### Block: A` and `### Block: B` are children of
        // `## Blocks`. The needle `Block: Instruction Fetch (IF)` is
        // unique here so a plain match works; we additionally verify
        // that a breadcrumb form (`Blocks > Block: Instruction Fetch (IF)`)
        // resolves correctly.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.md"), fixture()).unwrap();
        let r = ReadMarkdownTool
            .invoke(
                &ctx(tmp.path()),
                &json!({
                    "path": "spec.md",
                    "section": "Blocks > Block: Instruction Fetch (IF)"
                }),
            )
            .unwrap();
        assert!(r.ok, "got `{}`", r.display);
        assert!(r.display.contains("IF body line A"));
        assert!(!r.display.contains("PD body"));
    }

    #[test]
    fn section_mode_ambiguous_match_reports_candidates() {
        // Both blocks have heading text `Common`; the breadcrumb
        // resolver should refuse and list both.
        let body = "# Top\n\n## A\n\n### Common\n\nA body\n\n## B\n\n### Common\n\nB body\n";
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.md"), body).unwrap();
        let r = ReadMarkdownTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "x.md", "section": "Common" }),
            )
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("ambiguous"));
        assert!(r.display.contains("A > Common"));
        assert!(r.display.contains("B > Common"));
    }

    #[test]
    fn section_not_found_offers_suggestions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.md"), fixture()).unwrap();
        let r = ReadMarkdownTool
            .invoke(
                &ctx(tmp.path()),
                &json!({ "path": "spec.md", "section": "Block" }),
            )
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("did you mean"));
        // The substring `Block` is in three headings; at least one
        // of those should appear as a suggestion.
        assert!(
            r.display.contains("Block: Instruction Fetch (IF)")
                || r.display.contains("Block: Pre-Decode (PD)")
                || r.display.contains("Blocks"),
            "got `{}`",
            r.display
        );
    }

    #[test]
    fn fenced_code_blocks_do_not_yield_phantom_headings() {
        // A literal `# Not a heading` inside ``` fences MUST NOT be
        // picked up by the scanner.
        let body = "# Real Top\n\n```\n# Not a heading\n## Also not\n```\n\n## Real H2\n\nbody\n";
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.md"), body).unwrap();
        let r = ReadMarkdownTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "x.md" }))
            .unwrap();
        assert!(r.ok);
        // Outline contains the two real headings.
        assert!(r.display.contains("Real Top"));
        assert!(r.display.contains("Real H2"));
        // ...but NOT the in-fence imposters.
        assert!(!r.display.contains("Not a heading"));
        assert!(!r.display.contains("Also not"));
    }

    #[test]
    fn missing_path_arg_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadMarkdownTool
            .invoke(&ctx(tmp.path()), &json!({}))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn missing_file_returns_err_with_path() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ReadMarkdownTool
            .invoke(&ctx(tmp.path()), &json!({ "path": "absent.md" }))
            .unwrap();
        assert!(!r.ok);
        assert!(r.display.contains("absent.md"));
    }
}
