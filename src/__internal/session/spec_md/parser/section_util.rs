//! Helpers shared by every per-section parser: H3 sub-sectioning,
//! property-block parsing, bullet-list extraction, prose-body
//! capture.

// Some helpers in this module are consumed by per-section parsers
// landing in later milestones (M1.5-1.12). Until those parsers reach
// the public dispatch path the compiler can't see the call sites.
#![allow(dead_code)]

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::cmark_options;

/// One H3-rooted sub-section within an H2 section: the heading text
/// plus the raw markdown body. Mirrors the H2 segmenter but at a
/// shallower nesting level so per-section parsers can pick out
/// `### Interface: Foo`, `### Block: Bar`, etc.
#[derive(Debug, Clone)]
pub(crate) struct SubSection {
    pub heading: String,
    pub body: String,
}

/// Split an H2 section body into (preamble, h3 sub-sections). The
/// preamble is everything between the H2 heading and the first H3
/// heading (or all body content when there are no H3 headings).
pub(crate) fn split_h3(body: &str) -> (String, Vec<SubSection>) {
    split_at_level(body, 3)
}

/// Split a section body into (preamble, h4 sub-sections). Used by
/// per-block parsers to peel off `#### State`, `#### Behavior
/// summary`, etc.
pub(crate) fn split_h4(body: &str) -> (String, Vec<SubSection>) {
    split_at_level(body, 4)
}

fn split_at_level(body: &str, level: u32) -> (String, Vec<SubSection>) {
    let parser = Parser::new_ext(body, cmark_options()).into_offset_iter();
    // (offset, heading_text)
    let mut marks: Vec<(usize, String)> = Vec::new();
    let mut current_h: Option<(u32, usize)> = None;
    let mut current_text = String::new();
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level: l, .. }) => {
                let n = match l {
                    pulldown_cmark::HeadingLevel::H1 => 1,
                    pulldown_cmark::HeadingLevel::H2 => 2,
                    pulldown_cmark::HeadingLevel::H3 => 3,
                    pulldown_cmark::HeadingLevel::H4 => 4,
                    pulldown_cmark::HeadingLevel::H5 => 5,
                    pulldown_cmark::HeadingLevel::H6 => 6,
                };
                current_h = Some((n, range.start));
                current_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((n, start)) = current_h.take()
                    && n == level
                {
                    marks.push((start, std::mem::take(&mut current_text).trim().to_string()));
                }
            }
            Event::Text(t) | Event::Code(t) if current_h.is_some() => {
                current_text.push_str(&t);
            }
            _ => {}
        }
    }
    let first = marks.first().map(|(s, _)| *s).unwrap_or(body.len());
    let preamble = body[..first].to_string();
    let mut subs: Vec<SubSection> = Vec::new();
    for (i, (start, heading)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map(|(s, _)| *s).unwrap_or(body.len());
        subs.push(SubSection {
            heading: heading.clone(),
            body: body[*start..end].to_string(),
        });
    }
    (preamble, subs)
}

/// Strip the leading `## <Heading>\n` (or `### <Heading>\n`, etc.)
/// from a section body so the prose collector sees only the body
/// text. We do this textually to keep the rest of the body byte-
/// identical to the source (round-trip stability).
pub(crate) fn strip_leading_heading(body: &str) -> &str {
    let trimmed = body.trim_start_matches('\n');
    // Skip the first line that starts with `#`.
    if let Some(rest) = trimmed.strip_prefix('#') {
        let after_hashes = rest.trim_start_matches('#');
        if let Some(nl) = after_hashes.find('\n') {
            return &after_hashes[nl + 1..];
        }
        return "";
    }
    trimmed
}

/// Collect a markdown prose body into a normalized string: paragraphs
/// are joined by blank lines, leading / trailing whitespace stripped,
/// hard line breaks within a paragraph collapsed to spaces. Lists
/// and other block elements pass through as their textual content
/// only. Used by the prose sections (Purpose / Scope / Non-goals /
/// per-block Behavior summary / etc.) where the spec records the
/// meaning, not the markdown rendering.
pub(crate) fn collect_prose(body: &str) -> String {
    let parser = Parser::new_ext(body, cmark_options());
    let mut out = String::new();
    let mut paragraph = String::new();
    let mut in_paragraph = false;
    for event in parser {
        match event {
            Event::Start(Tag::Paragraph) => {
                in_paragraph = true;
                paragraph.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                in_paragraph = false;
                if !paragraph.trim().is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(paragraph.trim());
                }
            }
            Event::Text(t) if in_paragraph => paragraph.push_str(&t),
            Event::Code(t) if in_paragraph => {
                paragraph.push('`');
                paragraph.push_str(&t);
                paragraph.push('`');
            }
            Event::SoftBreak | Event::HardBreak if in_paragraph => {
                paragraph.push(' ');
            }
            _ => {}
        }
    }
    out
}

/// Extract the top-level bullet list items as plain strings (one
/// entry per bullet). The text of each item is its first paragraph;
/// any nested list / further block content is discarded so callers
/// that only want the bullet's headline get exactly that.
pub(crate) fn collect_top_level_bullets(body: &str) -> Vec<String> {
    let parser = Parser::new_ext(body, cmark_options());
    let mut items: Vec<String> = Vec::new();
    let mut depth_list: usize = 0;
    let mut current = String::new();
    let mut capturing = false;
    for event in parser {
        match event {
            Event::Start(Tag::List(_)) => {
                depth_list += 1;
                if depth_list >= 2 {
                    // Entering a nested list: stop appending to the
                    // outer item's headline.
                    capturing = false;
                }
            }
            Event::End(TagEnd::List(_)) => {
                depth_list = depth_list.saturating_sub(1);
            }
            Event::Start(Tag::Item) if depth_list == 1 => {
                current.clear();
                capturing = true;
            }
            Event::End(TagEnd::Item) if depth_list == 1 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    items.push(trimmed);
                }
                capturing = false;
            }
            Event::Text(t) if capturing => current.push_str(&t),
            Event::Html(t) | Event::InlineHtml(t) if capturing => current.push_str(&t),
            Event::Code(t) if capturing => {
                current.push('`');
                current.push_str(&t);
                current.push('`');
            }
            Event::SoftBreak | Event::HardBreak if capturing => current.push(' '),
            _ => {}
        }
    }
    items
}

/// Parse a bold-key property block of the form
///
/// ```markdown
/// **Direction:** bidirectional
/// **Protocol:** AHB / Wishbone (parameterized)
/// ```
///
/// returning each `(key, value)` pair in document order with the
/// `**` markers stripped and the value trimmed. Lines that don't
/// match the `**key:** value` shape are ignored.
pub(crate) fn parse_bold_properties(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with("**") {
            continue;
        }
        let inner = &line[2..];
        let Some(close) = inner.find("**") else {
            continue;
        };
        let key = inner[..close].trim_end_matches(':').trim().to_string();
        let rest = inner[close + 2..].trim();
        let value = rest.strip_prefix(':').unwrap_or(rest).trim().to_string();
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_h3_basic() {
        let body = "## Sec\n\npre\n\n### A\n\nbody-a\n\n### B\n\nbody-b\n";
        let (pre, subs) = split_h3(body);
        assert!(pre.contains("pre"));
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].heading, "A");
        assert!(subs[0].body.contains("body-a"));
        assert_eq!(subs[1].heading, "B");
    }

    #[test]
    fn prose_collects_paragraphs() {
        let body = "## P\n\nFirst paragraph.\n\nSecond paragraph with `code`.\n";
        let prose = collect_prose(body);
        assert_eq!(prose, "First paragraph.\n\nSecond paragraph with `code`.");
    }

    #[test]
    fn collect_top_level_bullets_basic() {
        let body = "- one\n- two\n  - nested (ignored)\n- three\n";
        let items = collect_top_level_bullets(body);
        assert_eq!(
            items,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn parses_bold_properties() {
        let body = "**Direction:** out\n**Protocol:** AHB\n";
        let props = parse_bold_properties(body);
        assert_eq!(
            props,
            vec![
                ("Direction".to_string(), "out".to_string()),
                ("Protocol".to_string(), "AHB".to_string()),
            ]
        );
    }
}
