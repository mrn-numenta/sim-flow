//! Parser for the `## Metadata` section (Chapter 2 §2.3.1).
//!
//! Metadata is a definition-list-style bullet list at the H2 level:
//!
//! ```markdown
//! ## Metadata
//!
//! - Design name: RV12 RISC-V CPU Core
//! - Version: 1.0
//! - Status: draft
//! - Authors: Mike Neilly <mneilly@numenta.com>
//! - Source documents:
//!   - primary: docs/RV12 RISC-V CPU Core.pdf
//!   - peer: tm-spec -> docs/temporal-memory.pdf
//! - Last updated: 2026-05-17
//! ```
//!
//! Each top-level bullet is a `key: value` pair. The `Source
//! documents` entry has nested bullets whose own `role: <path>` shape
//! drives [`crate::session::spec_md::types::SourceDocument`].

use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

use super::SpecMdParseError;
use super::cmark_options;
use crate::session::spec_md::types::{Metadata, SourceDocument, SourceDocumentRole};

/// Parse the body of a `## Metadata` section into [`Metadata`].
pub(crate) fn parse_metadata(body: &str) -> Result<Metadata, SpecMdParseError> {
    let mut md = Metadata::default();
    let bullets = collect_top_bullets_with_children(body);
    for item in bullets {
        let (key_raw, value_raw) = split_kv(&item.head);
        let key = key_raw.trim();
        let value = value_raw.trim();
        match key.to_ascii_lowercase().as_str() {
            "design name" => md.design_name = value.to_string(),
            "version" | "version / revision" => md.version = value.to_string(),
            "status" | "spec status" => md.status = value.to_string(),
            "authors" | "author(s)" | "author" => {
                md.authors = parse_authors(value);
            }
            "source documents" | "source document(s)" | "source documents:" => {
                for child in &item.children {
                    if let Some(doc) = parse_source_document_line(child)? {
                        md.source_documents.push(doc);
                    }
                }
            }
            "last updated" => md.last_updated = value.to_string(),
            _ => {
                // Forward-compatibility: ignore unknown keys here;
                // validation flags them as warnings (Phase 1.14).
            }
        }
    }
    Ok(md)
}

fn split_kv(item: &str) -> (&str, &str) {
    if let Some((k, v)) = item.split_once(':') {
        (k, v)
    } else {
        (item, "")
    }
}

fn parse_authors(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_source_document_line(line: &str) -> Result<Option<SourceDocument>, SpecMdParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let (role_raw, rest) = match trimmed.split_once(':') {
        Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim()),
        None => {
            return Err(SpecMdParseError::Other {
                message: format!("malformed source-document line: `{trimmed}`"),
                line: 0,
                column: 0,
            });
        }
    };
    let role = match role_raw.as_str() {
        "primary" => SourceDocumentRole::Primary,
        "peer" => SourceDocumentRole::Peer,
        other => {
            return Err(SpecMdParseError::Other {
                message: format!("unknown source-document role `{other}`"),
                line: 0,
                column: 0,
            });
        }
    };
    let (peer_id, path) = match role {
        SourceDocumentRole::Primary => (None, rest.to_string()),
        SourceDocumentRole::Peer => {
            // Accept both ASCII `->` and Unicode arrow `→`.
            let split = rest
                .split_once('\u{2192}') // U+2192 RIGHTWARDS ARROW
                .or_else(|| rest.split_once("->"));
            match split {
                Some((id, p)) => (Some(id.trim().to_string()), p.trim().to_string()),
                None => (None, rest.to_string()),
            }
        }
    };
    Ok(Some(SourceDocument {
        role,
        peer_id,
        path,
    }))
}

/// One bullet in a top-level list, plus the text of its immediate
/// nested-list children (used by Source-documents).
struct TopBullet {
    head: String,
    children: Vec<String>,
}

fn collect_top_bullets_with_children(body: &str) -> Vec<TopBullet> {
    let parser = Parser::new_ext(body, cmark_options());
    let mut out: Vec<TopBullet> = Vec::new();
    let mut depth_list: usize = 0;
    let mut current_head = String::new();
    let mut current_children: Vec<String> = Vec::new();
    let mut current_child = String::new();
    let mut capturing_head = false;
    let mut capturing_child = false;
    // Email / URL autolinks render as `Start(Link { LinkType::Email,
    // .. })` / `Text(..)` / `End(Link)`. We re-wrap with `<>` so the
    // captured text matches the original source-form (e.g. "Mike
    // Neilly <mneilly@numenta.com>").
    let mut autolink: Option<&'static str> = None;
    for event in parser {
        match event {
            Event::Start(Tag::List(_)) => {
                depth_list += 1;
                if depth_list == 2 {
                    // Entering the nested list -- stop the outer head
                    // from absorbing nested-item text.
                    capturing_head = false;
                }
            }
            Event::End(TagEnd::List(_)) => {
                depth_list = depth_list.saturating_sub(1);
            }
            Event::Start(Tag::Item) if depth_list == 1 => {
                current_head.clear();
                current_children.clear();
                capturing_head = true;
            }
            Event::Start(Tag::Item) if depth_list == 2 => {
                current_child.clear();
                capturing_child = true;
            }
            Event::End(TagEnd::Item) if depth_list == 2 => {
                let trimmed = current_child.trim().to_string();
                if !trimmed.is_empty() {
                    current_children.push(trimmed);
                }
                capturing_child = false;
            }
            Event::End(TagEnd::Item) if depth_list == 1 => {
                let head = current_head.trim().to_string();
                if !head.is_empty() {
                    out.push(TopBullet {
                        head,
                        children: std::mem::take(&mut current_children),
                    });
                }
                capturing_head = false;
            }
            Event::Start(Tag::Link { link_type, .. }) => {
                let marker = match link_type {
                    LinkType::Email => "email",
                    LinkType::Autolink => "url",
                    _ => "",
                };
                if !marker.is_empty() {
                    autolink = Some(marker);
                    push_text(
                        "<",
                        capturing_child,
                        capturing_head,
                        &mut current_child,
                        &mut current_head,
                    );
                }
            }
            Event::End(TagEnd::Link) if autolink.is_some() => {
                push_text(
                    ">",
                    capturing_child,
                    capturing_head,
                    &mut current_child,
                    &mut current_head,
                );
                autolink = None;
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                push_text(
                    &t,
                    capturing_child,
                    capturing_head,
                    &mut current_child,
                    &mut current_head,
                );
            }
            Event::SoftBreak | Event::HardBreak => {
                if capturing_child {
                    current_child.push(' ');
                } else if capturing_head {
                    current_head.push(' ');
                }
            }
            _ => {}
        }
    }
    out
}

fn push_text(
    t: &str,
    capturing_child: bool,
    capturing_head: bool,
    child: &mut String,
    head: &mut String,
) {
    if capturing_child {
        child.push_str(t);
    } else if capturing_head {
        head.push_str(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_metadata() {
        let body = "\
## Metadata

- Design name: RV12 RISC-V CPU Core
- Version: 1.0
- Status: draft
- Authors: Mike Neilly <mneilly@numenta.com>
- Source documents:
  - primary: docs/RV12 RISC-V CPU Core.pdf
  - peer: tm-spec -> docs/temporal-memory.pdf
- Last updated: 2026-05-17
";
        let md = parse_metadata(body).expect("parses");
        assert_eq!(md.design_name, "RV12 RISC-V CPU Core");
        assert_eq!(md.version, "1.0");
        assert_eq!(md.status, "draft");
        assert_eq!(
            md.authors,
            vec!["Mike Neilly <mneilly@numenta.com>".to_string()]
        );
        assert_eq!(md.last_updated, "2026-05-17");
        assert_eq!(md.source_documents.len(), 2);
        assert_eq!(md.source_documents[0].role, SourceDocumentRole::Primary);
        assert_eq!(md.source_documents[0].path, "docs/RV12 RISC-V CPU Core.pdf");
        assert_eq!(md.source_documents[1].role, SourceDocumentRole::Peer);
        assert_eq!(md.source_documents[1].peer_id.as_deref(), Some("tm-spec"));
        assert_eq!(md.source_documents[1].path, "docs/temporal-memory.pdf");
    }

    #[test]
    fn unicode_arrow_in_peer_works() {
        let body = "\
## Metadata

- Source documents:
  - peer: tm-spec \u{2192} docs/tm.pdf
";
        let md = parse_metadata(body).expect("parses");
        assert_eq!(md.source_documents[0].peer_id.as_deref(), Some("tm-spec"));
        assert_eq!(md.source_documents[0].path, "docs/tm.pdf");
    }

    #[test]
    fn empty_metadata_is_default() {
        let body = "## Metadata\n";
        let md = parse_metadata(body).expect("parses");
        assert_eq!(md, Metadata::default());
    }
}
