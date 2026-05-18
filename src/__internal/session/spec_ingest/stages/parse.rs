//! Stage 3: hierarchical parsing.
//!
//! Builds a `SectionTree` of `Section { heading, level, breadcrumb,
//! body, page_range, children }`. For PDFs we infer headings from
//! line patterns and font sizes; for markdown we walk the source.

use crate::Result;

use super::super::pipeline::{IngestWarning, SourceKind};
use super::loading::{LoadedSource, PageText};

/// One section node in the heading tree.
#[derive(Debug, Clone)]
pub struct Section {
    pub heading: String,
    pub level: u8,
    /// Full ancestor chain ending at this section's heading.
    pub breadcrumb: Vec<String>,
    pub body: String,
    /// Inclusive (start, end) page range. For markdown / text the
    /// pair is `(1, 1)`.
    pub page_range: (u32, u32),
    pub children: Vec<Section>,
    /// Annotations attached by stage 4 (classify).
    pub kind: SectionKind,
    pub contained_signal_tables: Vec<String>,
    pub contained_parameter_tables: Vec<String>,
    pub contained_error_tables: Vec<String>,
    pub contained_encoding_tables: Vec<String>,
    pub contained_fsm_tables: Vec<String>,
    pub contained_figures: Vec<String>,
    pub tbd_count: u32,
    pub stub_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Prose,
    Table,
    Stub,
    Figure,
    Mixed,
}

impl SectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SectionKind::Prose => "prose",
            SectionKind::Table => "table",
            SectionKind::Stub => "stub",
            SectionKind::Figure => "figure",
            SectionKind::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SectionTree {
    pub roots: Vec<Section>,
    pub source_kind: Option<SourceKind>,
    pub source_label: String,
    pub total_pages: u32,
}

impl SectionTree {
    /// Iterate every section in DFS pre-order. Borrowed.
    pub fn iter(&self) -> SectionIter<'_> {
        SectionIter {
            stack: self.roots.iter().rev().collect(),
        }
    }

    /// Iterate every section mutably in DFS pre-order.
    pub fn iter_mut(&mut self) -> SectionIterMut<'_> {
        let stack: Vec<*mut Section> = self.roots.iter_mut().rev().map(|s| s as *mut _).collect();
        SectionIterMut {
            stack,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn count(&self) -> usize {
        self.iter().count()
    }
}

pub struct SectionIter<'a> {
    stack: Vec<&'a Section>,
}

impl<'a> Iterator for SectionIter<'a> {
    type Item = &'a Section;
    fn next(&mut self) -> Option<Self::Item> {
        let s = self.stack.pop()?;
        for child in s.children.iter().rev() {
            self.stack.push(child);
        }
        Some(s)
    }
}

pub struct SectionIterMut<'a> {
    // Stored as raw pointers so we can drive a DFS without fighting
    // the borrow checker; safety relies on never aliasing two
    // pointers at once (we pop one before pushing children).
    stack: Vec<*mut Section>,
    _marker: std::marker::PhantomData<&'a mut Section>,
}

impl<'a> Iterator for SectionIterMut<'a> {
    type Item = &'a mut Section;
    fn next(&mut self) -> Option<Self::Item> {
        let ptr = self.stack.pop()?;
        // SAFETY: each pointer is uniquely owned by the original
        // tree; we hand it out once, then push pointers to its
        // children. The mutable iterator caller cannot ask for two
        // overlapping borrows because we move each pointer off the
        // stack before yielding.
        let section: &'a mut Section = unsafe { &mut *ptr };
        for child in section.children.iter_mut().rev() {
            self.stack.push(child as *mut _);
        }
        Some(section)
    }
}

pub fn parse_hierarchy(
    loaded: &LoadedSource,
    warnings: &mut Vec<IngestWarning>,
) -> Result<SectionTree> {
    match loaded.kind {
        SourceKind::None => Ok(SectionTree {
            roots: Vec::new(),
            source_kind: Some(SourceKind::None),
            source_label: String::new(),
            total_pages: 0,
        }),
        SourceKind::Markdown | SourceKind::Text => {
            let body = loaded.pages.first().map(|p| p.text.as_str()).unwrap_or("");
            let tree = parse_markdown(body, warnings)?;
            Ok(SectionTree {
                source_kind: Some(loaded.kind),
                source_label: String::new(),
                total_pages: 1,
                ..tree
            })
        }
        SourceKind::Pdf => {
            let tree = parse_pdf(&loaded.pages, warnings)?;
            Ok(SectionTree {
                source_kind: Some(SourceKind::Pdf),
                source_label: String::new(),
                total_pages: loaded.pages.len() as u32,
                ..tree
            })
        }
    }
}

// ---------------------------------------------------------------------
// Markdown parser. Tiny line-based scanner; we don't need
// pulldown_cmark for the simple H1-H6 / body separation. Heading
// detection: any line matching `^#{1,6}\s+...`. Body of a heading is
// the lines between its heading and the next heading at any level.
// ---------------------------------------------------------------------

pub(crate) fn parse_markdown(body: &str, warnings: &mut Vec<IngestWarning>) -> Result<SectionTree> {
    let heading_re = regex::Regex::new(r"^(#{1,6})\s+(.+?)\s*$").unwrap();
    let mut sections: Vec<(u8, String, String)> = Vec::new(); // (level, heading, body)
    let mut current: Option<(u8, String, String)> = None;
    let mut preamble = String::new();
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        if let Some(caps) = heading_re.captures(trimmed) {
            let level = caps.get(1).unwrap().as_str().len() as u8;
            let heading = caps.get(2).unwrap().as_str().trim().to_string();
            if let Some((lvl, hd, bd)) = current.take() {
                sections.push((lvl, hd, bd));
            }
            current = Some((level, heading, String::new()));
        } else if let Some((_, _, bd)) = current.as_mut() {
            bd.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    if let Some((lvl, hd, bd)) = current {
        sections.push((lvl, hd, bd));
    }

    if sections.is_empty() {
        // Degenerate: emit a single root spanning the body.
        warnings.push(IngestWarning::new(
            "no_headings_detected",
            "markdown source had no headings; emitting a single root section",
            3,
        ));
        let body_text = if preamble.is_empty() {
            body.to_string()
        } else {
            preamble
        };
        return Ok(SectionTree {
            roots: vec![make_section(
                "(document)",
                1,
                &["(document)".to_string()],
                body_text,
                (1, 1),
            )],
            ..Default::default()
        });
    }

    // Attach preamble (if any) as a synthetic first section if it
    // has nontrivial content.
    if !preamble.trim().is_empty() {
        sections.insert(0, (1, "(front matter)".into(), preamble));
    }

    let roots = build_nested(sections, (1, 1));
    Ok(SectionTree {
        roots,
        ..Default::default()
    })
}

fn make_section(
    heading: &str,
    level: u8,
    breadcrumb: &[String],
    body: String,
    page_range: (u32, u32),
) -> Section {
    Section {
        heading: heading.to_string(),
        level,
        breadcrumb: breadcrumb.to_vec(),
        body,
        page_range,
        children: Vec::new(),
        kind: SectionKind::Prose,
        contained_signal_tables: Vec::new(),
        contained_parameter_tables: Vec::new(),
        contained_error_tables: Vec::new(),
        contained_encoding_tables: Vec::new(),
        contained_fsm_tables: Vec::new(),
        contained_figures: Vec::new(),
        tbd_count: 0,
        stub_hint: None,
    }
}

fn build_nested(flat: Vec<(u8, String, String)>, page_range: (u32, u32)) -> Vec<Section> {
    // Build a nested tree given a list of (level, heading, body)
    // entries in document order.
    let mut roots: Vec<Section> = Vec::new();
    // Stack of (level, index path) into the section tree.
    let mut stack: Vec<*mut Section> = Vec::new();
    let roots_ptr: *mut Vec<Section> = &mut roots;
    for (level, heading, body) in flat {
        // Pop stack down to last entry with level < this.
        while let Some(top) = stack.last() {
            // SAFETY: top still points into `roots` (or one of its
            // descendant Vec<Section>s); we only mutate the tree
            // through unique pointers we own here.
            let top_level = unsafe { (**top).level };
            if top_level >= level {
                stack.pop();
            } else {
                break;
            }
        }
        let mut breadcrumb: Vec<String> = stack
            .iter()
            .map(|p| unsafe { (**p).heading.clone() })
            .collect();
        breadcrumb.push(heading.clone());
        let new_section = make_section(&heading, level, &breadcrumb, body, page_range);
        let parent_children: *mut Vec<Section> = match stack.last() {
            Some(top) => {
                let top_section: &mut Section = unsafe { &mut **top };
                &mut top_section.children as *mut _
            }
            None => roots_ptr,
        };
        // SAFETY: the pointer is into the tree we own; pushing a new
        // section never invalidates pointers in `stack` because we
        // only push to leaves we've just descended into.
        let parent_vec: &mut Vec<Section> = unsafe { &mut *parent_children };
        parent_vec.push(new_section);
        let added: *mut Section = parent_vec.last_mut().unwrap() as *mut _;
        stack.push(added);
    }
    roots
}

// ---------------------------------------------------------------------
// PDF parser. Without easy access to per-glyph font sizes through
// pdfium's high-level text API, we use a robust heuristic: lines
// whose content matches a heading-like pattern (TOC-style numbering
// or all-caps short lines) become headings. Body text is the lines
// between headings. The pdf's outline (if any) takes precedence.
// ---------------------------------------------------------------------

pub(crate) fn parse_pdf(
    pages: &[PageText],
    warnings: &mut Vec<IngestWarning>,
) -> Result<SectionTree> {
    // Flatten pages into a stream of (page_number, line). Track each
    // line's source page so headings carry an accurate page_range.
    let mut lines: Vec<(u32, String)> = Vec::new();
    for p in pages {
        for line in p.text.lines() {
            lines.push((p.page_number, line.to_string()));
        }
    }

    // Heading detection patterns:
    //   - "1 Title", "1.2 Title", "1.2.3 Title" (TOC numbering)
    let numbered_re = regex::Regex::new(r"^\s*(\d+(?:\.\d+){0,5})\s+(\S.*?)\s*$").unwrap();
    // Markdown-style headings inside PDFs (rare, but tolerated).
    let md_re = regex::Regex::new(r"^\s*(#{1,6})\s+(.+?)\s*$").unwrap();

    // We classify each line as either heading-start or body.
    enum LineRole {
        Heading { level: u8, text: String },
        Body,
    }
    let role_of = |line: &str| -> LineRole {
        let trimmed = line.trim();
        if let Some(caps) = md_re.captures(trimmed) {
            let level = caps.get(1).unwrap().as_str().len() as u8;
            return LineRole::Heading {
                level,
                text: caps.get(2).unwrap().as_str().to_string(),
            };
        }
        if let Some(caps) = numbered_re.captures(trimmed) {
            let number = caps.get(1).unwrap().as_str();
            let text = caps.get(2).unwrap().as_str();
            // Reject obvious false positives: lines ending in a period
            // are likely sentences; lines containing more than ~12
            // words are likely prose.
            if text.ends_with('.') {
                return LineRole::Body;
            }
            if text.split_whitespace().count() > 12 {
                return LineRole::Body;
            }
            let level = (number.matches('.').count() + 1).clamp(1, 6) as u8;
            return LineRole::Heading {
                level,
                text: format!("{number} {text}"),
            };
        }
        LineRole::Body
    };

    // Walk lines, building flat (level, heading, body, page_range).
    let mut flat: Vec<(u8, String, String, (u32, u32))> = Vec::new();
    let mut current: Option<(u8, String, String, u32, u32)> = None; // level, heading, body, start, end
    for (page, line) in &lines {
        match role_of(line) {
            LineRole::Heading { level, text } => {
                if let Some((lvl, hd, bd, st, en)) = current.take() {
                    flat.push((lvl, hd, bd, (st, en)));
                }
                current = Some((level, text, String::new(), *page, *page));
            }
            LineRole::Body => {
                if let Some((_, _, bd, _, en)) = current.as_mut() {
                    bd.push_str(line);
                    bd.push('\n');
                    *en = *page;
                }
                // Lines before the first heading are dropped on the
                // PDF path; the chrome-stripping step plus the
                // numbered-heading detector usually picks up the
                // title from the TOC.
            }
        }
    }
    if let Some((lvl, hd, bd, st, en)) = current {
        flat.push((lvl, hd, bd, (st, en)));
    }

    let total_pages = pages.last().map(|p| p.page_number).unwrap_or(1);

    if flat.is_empty() {
        warnings.push(IngestWarning::new(
            "no_headings_detected",
            "PDF had no inferrable headings; emitting a single root section",
            3,
        ));
        let body = pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(SectionTree {
            roots: vec![make_section(
                "(document)",
                1,
                &["(document)".to_string()],
                body,
                (1, total_pages),
            )],
            ..Default::default()
        });
    }

    let flat_for_nest: Vec<(u8, String, String)> = flat
        .iter()
        .map(|(l, h, b, _)| (*l, h.clone(), b.clone()))
        .collect();
    let page_ranges: Vec<(u32, u32)> = flat.iter().map(|(_, _, _, r)| *r).collect();
    let mut roots = build_nested(flat_for_nest, (1, 1));
    // Walk and patch in real page ranges (build_nested doesn't know
    // about per-entry pages). We rely on the DFS order matching the
    // flat entry order, which it does for build_nested.
    let mut idx = 0usize;
    apply_page_ranges(&mut roots, &page_ranges, &mut idx);

    Ok(SectionTree {
        roots,
        ..Default::default()
    })
}

fn apply_page_ranges(sections: &mut [Section], ranges: &[(u32, u32)], idx: &mut usize) {
    for s in sections.iter_mut() {
        if *idx < ranges.len() {
            s.page_range = ranges[*idx];
            *idx += 1;
        }
        apply_page_ranges(&mut s.children, ranges, idx);
        // Extend the section's page_range over its descendants for
        // accuracy (a parent section's range covers all its
        // children's pages).
        let mut max_end = s.page_range.1;
        for c in &s.children {
            max_end = max_end.max(c.page_range.1);
        }
        s.page_range.1 = max_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_simple_h1_h2_tree() {
        let body = "# Top\nintro\n\n## Sub\nsubbody\n";
        let mut warnings = Vec::new();
        let tree = parse_markdown(body, &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "Top");
        assert_eq!(tree.roots[0].children.len(), 1);
        assert_eq!(tree.roots[0].children[0].heading, "Sub");
        assert_eq!(
            tree.roots[0].children[0].breadcrumb,
            vec!["Top".to_string(), "Sub".to_string()]
        );
    }

    #[test]
    fn markdown_no_headings_produces_degenerate_root_with_warning() {
        let mut warnings = Vec::new();
        let tree = parse_markdown("just text\nmore text\n", &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "(document)");
        assert!(warnings.iter().any(|w| w.code == "no_headings_detected"));
    }

    #[test]
    fn pdf_numbered_headings_build_tree() {
        let pages = vec![
            PageText {
                page_number: 1,
                text: "1 Introduction\nsome body\nmore body\n".into(),
            },
            PageText {
                page_number: 2,
                text: "1.1 Background\nbg body\n2 Architecture\narch body\n".into(),
            },
        ];
        let mut warnings = Vec::new();
        let tree = parse_pdf(&pages, &mut warnings).unwrap();
        // Roots: Introduction (level 1), Architecture (level 1).
        let root_headings: Vec<&str> = tree.roots.iter().map(|s| s.heading.as_str()).collect();
        assert!(root_headings.iter().any(|h| h.contains("Introduction")));
        assert!(root_headings.iter().any(|h| h.contains("Architecture")));
        // 1.1 Background is a child of Introduction.
        let intro = tree
            .roots
            .iter()
            .find(|s| s.heading.contains("Introduction"))
            .unwrap();
        assert_eq!(intro.children.len(), 1);
        assert!(intro.children[0].heading.contains("Background"));
    }

    #[test]
    fn pdf_no_headings_emits_warning() {
        let pages = vec![PageText {
            page_number: 1,
            text: "this is just prose\nwith no heading-like lines\n".into(),
        }];
        let mut warnings = Vec::new();
        let tree = parse_pdf(&pages, &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "(document)");
        assert!(warnings.iter().any(|w| w.code == "no_headings_detected"));
    }
}
