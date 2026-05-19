//! Stage 3: hierarchical parsing.
//!
//! Builds a `SectionTree` of `Section { heading, level, breadcrumb,
//! body, page_range, children }`. PDF and markdown inputs both flow
//! through [`parse_markdown`]: `pdf_oxide`'s `to_markdown()`
//! converts each PDF page into markdown with `#`-prefixed headings
//! (driven by font-size clustering), and the loading stage joins
//! those per-page markdowns with `<!-- spec-ingest:page=N -->`
//! markers that this stage uses to recover per-section page ranges
//! before stripping them from the chunked bodies.

use crate::Result;

use super::super::pipeline::{IngestWarning, SourceKind};
use super::loading::{LoadedSource, PageLayout};

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
            let body = loaded
                .pages
                .first()
                .map(|p| p.flat_text.as_str())
                .unwrap_or("");
            let tree = parse_markdown(body, warnings)?;
            Ok(SectionTree {
                source_kind: Some(loaded.kind),
                source_label: String::new(),
                total_pages: 1,
                ..tree
            })
        }
        SourceKind::Pdf => {
            let total_pages = loaded.pages.len() as u32;
            let joined = join_pdf_pages_for_markdown(&loaded.pages);
            let mut tree = parse_markdown(&joined, warnings)?;
            recover_page_ranges_and_strip_markers(&mut tree, total_pages);
            Ok(SectionTree {
                source_kind: Some(SourceKind::Pdf),
                source_label: String::new(),
                total_pages,
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
// PDF parser. `pdf_oxide::PdfDocument::to_markdown` runs heading
// clustering (by font size + weight), bold/italic preservation, and
// simple table detection on each page, producing per-page markdown
// that we splice together with `<!-- spec-ingest:page=N -->` markers.
// `parse_markdown` handles the resulting concatenated document; this
// stage's only PDF-specific work is recovering per-section page
// ranges from those markers and stripping the markers from chunked
// section bodies.
// ---------------------------------------------------------------------

/// HTML-comment marker emitted between PDF pages so we can recover
/// per-section page ranges after running the markdown parser. The
/// markdown parser treats these as opaque body text.
const PAGE_MARKER_PREFIX: &str = "<!-- spec-ingest:page=";
const PAGE_MARKER_SUFFIX: &str = " -->";

/// Build the synthetic markdown body the markdown parser consumes.
/// Appends a `<!-- spec-ingest:page=N -->` marker AFTER each page's
/// content so it always lands inside the section whose body it
/// belongs to (rather than in document preamble when the first
/// page has no heading). The post-pass scans every section's body
/// for these markers and pins section.page_range from them.
pub(crate) fn join_pdf_pages_for_markdown(pages: &[PageLayout]) -> String {
    let mut out = String::new();
    for p in pages {
        out.push_str(&p.flat_text);
        if !p.flat_text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(PAGE_MARKER_PREFIX);
        out.push_str(&p.page_number.to_string());
        out.push_str(PAGE_MARKER_SUFFIX);
        out.push('\n');
    }
    out
}

/// Walk the section tree, scan each section's body for
/// `<!-- spec-ingest:page=N -->` markers, set page_range from
/// min/max markers seen, and strip the markers from the body so
/// downstream chunk emission sees clean text. Sections with no
/// marker fall back to `(1, total_pages)`.
pub(crate) fn recover_page_ranges_and_strip_markers(tree: &mut SectionTree, total_pages: u32) {
    for section in tree.iter_mut() {
        let (range, cleaned) = extract_and_strip(&section.body, total_pages);
        section.page_range = range;
        section.body = cleaned;
    }
}

fn extract_and_strip(body: &str, total_pages: u32) -> ((u32, u32), String) {
    let mut min_page: Option<u32> = None;
    let mut max_page: Option<u32> = None;
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix(PAGE_MARKER_PREFIX)
            .and_then(|s| s.strip_suffix(PAGE_MARKER_SUFFIX))
        {
            if let Ok(n) = rest.parse::<u32>() {
                min_page = Some(min_page.map(|m| m.min(n)).unwrap_or(n));
                max_page = Some(max_page.map(|m| m.max(n)).unwrap_or(n));
            }
            // Drop the marker line; do not append to `out`.
            continue;
        }
        out.push_str(line);
    }
    let range = match (min_page, max_page) {
        (Some(lo), Some(hi)) => (lo, hi),
        _ => (1, total_pages.max(1)),
    };
    (range, out)
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

    fn flat_page(n: u32, text: &str) -> PageLayout {
        PageLayout {
            page_number: n,
            spans: Vec::new(),
            lines: Vec::new(),
            tables: Vec::new(),
            path_count: 0,
            image_count: 0,
            flat_text: text.into(),
        }
    }

    #[test]
    fn pdf_pages_join_with_page_markers_then_recover_ranges() {
        // pdf_oxide produces markdown per page; the loader joins them
        // with `<!-- spec-ingest:page=N -->` markers. parse_hierarchy
        // runs the markdown parser and the post-pass strips markers
        // while recovering per-section page ranges.
        let pages = vec![
            flat_page(1, "# Introduction\n\nintro body\n"),
            flat_page(2, "more intro body on page 2\n"),
            flat_page(3, "## Background\n\nbackground body\n"),
        ];
        let joined = join_pdf_pages_for_markdown(&pages);
        let mut warnings = Vec::new();
        let mut tree = parse_markdown(&joined, &mut warnings).unwrap();
        recover_page_ranges_and_strip_markers(&mut tree, 3);
        assert_eq!(tree.roots.len(), 1);
        let intro = &tree.roots[0];
        assert_eq!(intro.heading, "Introduction");
        // Introduction spans pages 1-2.
        assert_eq!(intro.page_range, (1, 2));
        // Markers were stripped from body.
        assert!(!intro.body.contains("spec-ingest:page="));
        // Background is page 3 only.
        assert_eq!(intro.children.len(), 1);
        assert_eq!(intro.children[0].heading, "Background");
        assert_eq!(intro.children[0].page_range, (3, 3));
        assert!(!intro.children[0].body.contains("spec-ingest:page="));
    }

    #[test]
    fn pdf_no_headings_emits_warning() {
        let pages = vec![flat_page(
            1,
            "this is just prose\nwith no heading-like lines\n",
        )];
        let joined = join_pdf_pages_for_markdown(&pages);
        let mut warnings = Vec::new();
        let mut tree = parse_markdown(&joined, &mut warnings).unwrap();
        recover_page_ranges_and_strip_markers(&mut tree, 1);
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "(document)");
        assert!(warnings.iter().any(|w| w.code == "no_headings_detected"));
    }
}
