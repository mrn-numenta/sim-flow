//! Stage 3: hierarchical parsing.
//!
//! Builds a `SectionTree` of `Section { heading, level, breadcrumb,
//! body, page_range, children }`.
//!
//! Markdown / text inputs flow through [`parse_markdown`], a tiny
//! line-based scanner over the flat-text representation produced by
//! loading.
//!
//! PDF inputs flow through [`parse_pdf_spans`]: it walks every
//! page's structured `PageLine` records (built in milestone 9.7
//! from `pdf_oxide::extract_text_lines`), clusters their dominant
//! font sizes across the whole document, treats lines whose size
//! lands in one of the top heading clusters as headings, and
//! assembles a `SectionTree` directly — no markdown round-trip and
//! no `<!-- spec-ingest:page=N -->` marker injection. Per-section
//! `page_range` is read natively from the page numbers of the
//! constituent lines.

use crate::Result;

use super::super::pipeline::{IngestWarning, SourceKind};
use super::loading::{BBox, LoadedSource, PageLine, PageTable};

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
            let tree = parse_pdf_spans(loaded, warnings)?;
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
// PDF span-based parser. Walks `LoadedSource::pages[].lines` (the
// structured per-line records built in milestone 9.7 from
// `pdf_oxide::extract_text_lines`), clusters their dominant font
// sizes across all pages, and treats lines whose size lands in one
// of the top heading-size clusters as headings. Body lines append
// to the current section's body. Per-section `page_range` is set
// natively from the page numbers of the constituent lines — no
// marker injection / stripping required.
// ---------------------------------------------------------------------

/// Bin width (PDF user-space points) used when grouping font sizes
/// into clusters. Sizes within `±FONT_BIN/2` of a cluster center
/// are folded into that cluster.
const FONT_BIN: f32 = 0.5;

/// Minimum number of lines a font-size cluster must contain before
/// it is considered as a heading candidate. A single oversized
/// glyph on one page should not turn the entire document into
/// chapters — but a small spec may legitimately have only a few
/// top-level sections, so we keep the floor at 2.
const MIN_CLUSTER_OCCURRENCES: usize = 2;

/// Maximum heading depth we surface (`Section::level` is capped at
/// 6 to mirror markdown H1-H6). Excess clusters collapse to H6.
const MAX_HEADING_LEVEL: u8 = 6;

/// Fraction of pages on which a line must repeat to be treated as
/// chrome (running header / footer) and skipped at parse time. The
/// dedicated chrome.rs pass (milestone 9.10) handles the broader
/// stripping; this is a narrow guard against page-chrome lines that
/// happen to share a heading-size cluster.
const CHROME_REPEAT_THRESHOLD: f32 = 0.6;

/// Span-based PDF parser. Builds a `SectionTree` directly from the
/// per-page `PageLine` records carried on `LoadedSource`.
pub(crate) fn parse_pdf_spans(
    loaded: &LoadedSource,
    warnings: &mut Vec<IngestWarning>,
) -> Result<SectionTree> {
    let total_pages = loaded.pages.len() as u32;

    // 1. Sweep all lines, sort each page top-to-bottom.
    // PDF user-space Y origin is at the bottom of the page, so
    // "first in reading order" is the line with the largest Y.
    let mut ordered: Vec<(u32, &PageLine, &[PageTable])> = Vec::new();
    for page in &loaded.pages {
        let mut page_lines: Vec<&PageLine> = page.lines.iter().collect();
        page_lines.sort_by(|a, b| {
            b.bbox
                .y
                .partial_cmp(&a.bbox.y)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for line in page_lines {
            ordered.push((page.page_number, line, page.tables.as_slice()));
        }
    }

    // 2. Cluster dominant font sizes.
    let clusters = cluster_font_sizes(&ordered);

    // 3. Identify chrome-candidate lines (repeated text on a large
    //    fraction of pages). We use this only to suppress heading
    //    promotion of running headers/footers — the body remains
    //    in place; chrome.rs (9.10) handles broader stripping.
    let chrome_lines = detect_chrome_lines(loaded);

    // 4. Walk lines in order, emit flat (level, heading, body,
    //    page_range) records.
    type FlatSection = (u8, String, String, (u32, u32));
    let mut flat: Vec<FlatSection> = Vec::new();
    let mut current: Option<FlatSection> = None;
    let mut preamble = String::new();
    let mut preamble_pages: Option<(u32, u32)> = None;

    for (page_no, line, tables) in &ordered {
        let trimmed = line.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let in_table = line_is_in_table(&line.bbox, tables);
        let is_chrome = chrome_lines.contains(trimmed);
        let level = heading_level(line.dominant_font_size, &clusters);

        if let Some(level) = level
            && !in_table
            && !is_chrome
        {
            if let Some(prev) = current.take() {
                flat.push(prev);
            }
            let range = (*page_no, *page_no);
            current = Some((level, trimmed.to_string(), String::new(), range));
            continue;
        }
        // Heading-size line that landed in a table or is chrome:
        // fall through and treat it as body.

        if let Some((_, _, body, range)) = current.as_mut() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&line.text);
            range.0 = range.0.min(*page_no);
            range.1 = range.1.max(*page_no);
        } else {
            if !preamble.is_empty() {
                preamble.push('\n');
            }
            preamble.push_str(&line.text);
            preamble_pages = Some(match preamble_pages {
                Some((lo, hi)) => (lo.min(*page_no), hi.max(*page_no)),
                None => (*page_no, *page_no),
            });
        }
    }
    if let Some(prev) = current.take() {
        flat.push(prev);
    }

    // 5. Degenerate case: no headings detected anywhere.
    if flat.is_empty() {
        warnings.push(IngestWarning::new(
            "no_headings_detected",
            "PDF source had no heading-cluster lines; emitting a single root section",
            3,
        ));
        let body_text = if !preamble.is_empty() {
            preamble
        } else {
            // No body either; assemble one from line texts.
            let mut acc = String::new();
            for (_page_no, line, _tables) in &ordered {
                if line.text.trim().is_empty() {
                    continue;
                }
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(&line.text);
            }
            acc
        };
        let range = preamble_pages.unwrap_or_else(|| (1, total_pages.max(1)));
        return Ok(SectionTree {
            roots: vec![make_section(
                "(document)",
                1,
                &["(document)".to_string()],
                body_text,
                range,
            )],
            ..Default::default()
        });
    }

    // 6. Attach preamble (lines before the first heading) as a
    //    synthetic first section if it has nontrivial content.
    if !preamble.trim().is_empty() {
        let range = preamble_pages.unwrap_or((1, 1));
        flat.insert(0, (1, "(front matter)".into(), preamble, range));
    }

    let roots = build_nested_with_ranges(flat);
    Ok(SectionTree {
        roots,
        ..Default::default()
    })
}

/// One font-size cluster: the representative size and its heading
/// level. Lower level = larger size = closer to H1.
#[derive(Debug, Clone, Copy)]
struct FontCluster {
    size: f32,
    level: u8,
}

/// Group dominant font sizes seen across all lines into clusters,
/// keep those with at least `MIN_CLUSTER_OCCURRENCES` lines, sort
/// descending by size, and assign heading levels 1..=MAX_HEADING_LEVEL.
fn cluster_font_sizes(ordered: &[(u32, &PageLine, &[PageTable])]) -> Vec<FontCluster> {
    // Greedy single-pass clustering: keep a list of (center, count);
    // for each new size pick the closest center within FONT_BIN/2,
    // otherwise start a new cluster.
    let mut centers: Vec<(f32, usize)> = Vec::new();
    for (_page, line, _tables) in ordered {
        let size = line.dominant_font_size;
        if size <= 0.0 {
            continue;
        }
        let mut best: Option<(usize, f32)> = None;
        for (idx, (center, _count)) in centers.iter().enumerate() {
            let d = (center - size).abs();
            if d <= FONT_BIN / 2.0 && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((idx, d));
            }
        }
        match best {
            Some((idx, _)) => {
                let (center, count) = centers[idx];
                let new_count = count + 1;
                // Running average of the cluster center so it
                // tracks the true mean as new samples land.
                let new_center = (center * count as f32 + size) / new_count as f32;
                centers[idx] = (new_center, new_count);
            }
            None => centers.push((size, 1)),
        }
    }

    // Keep only well-populated clusters.
    let mut kept: Vec<(f32, usize)> = centers
        .into_iter()
        .filter(|(_, count)| *count >= MIN_CLUSTER_OCCURRENCES)
        .collect();
    // Sort descending by size — largest sizes become H1.
    kept.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Determine the body-text cluster: the most populous cluster is
    // body, and any cluster at the same size as body (or smaller)
    // is also body. Heading clusters are those with size strictly
    // larger than the body cluster's size.
    let body_size = kept
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(s, _)| *s)
        .unwrap_or(0.0);

    kept.into_iter()
        .filter(|(size, _)| *size > body_size + FONT_BIN / 2.0)
        .enumerate()
        .map(|(idx, (size, _))| FontCluster {
            size,
            level: ((idx as u8) + 1).clamp(1, MAX_HEADING_LEVEL),
        })
        .collect()
}

/// Return the heading level a line's size maps to, if any.
fn heading_level(size: f32, clusters: &[FontCluster]) -> Option<u8> {
    if size <= 0.0 {
        return None;
    }
    for c in clusters {
        if (c.size - size).abs() <= FONT_BIN / 2.0 {
            return Some(c.level);
        }
    }
    None
}

/// True if the line's bounding box vertically overlaps any table on
/// the same page. Tables are not headings — their first row often
/// has bold / mid-size text that would otherwise look heading-like.
fn line_is_in_table(line_bbox: &BBox, tables: &[PageTable]) -> bool {
    let line_top = line_bbox.y + line_bbox.h;
    let line_bottom = line_bbox.y;
    for t in tables {
        let t_top = t.bbox.y + t.bbox.h;
        let t_bottom = t.bbox.y;
        // Treat any vertical overlap (with a small tolerance) as
        // "inside this table".
        if line_top >= t_bottom - 1.0 && line_bottom <= t_top + 1.0 {
            return true;
        }
    }
    false
}

/// Detect lines whose trimmed text repeats on a high fraction of
/// pages. These are running headers / footers that we suppress as
/// heading candidates.
fn detect_chrome_lines(loaded: &LoadedSource) -> std::collections::HashSet<String> {
    use std::collections::HashMap;

    let page_count = loaded.pages.len();
    if page_count == 0 {
        return Default::default();
    }
    let mut per_page_seen: HashMap<String, usize> = HashMap::new();
    for page in &loaded.pages {
        let mut seen_this_page: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for line in &page.lines {
            let trimmed = line.text.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            if seen_this_page.insert(trimmed.clone()) {
                *per_page_seen.entry(trimmed).or_insert(0) += 1;
            }
        }
    }
    let threshold = (page_count as f32 * CHROME_REPEAT_THRESHOLD).ceil() as usize;
    per_page_seen
        .into_iter()
        .filter(|(_, c)| *c >= threshold.max(2))
        .map(|(s, _)| s)
        .collect()
}

/// Like `build_nested`, but each entry carries its own page_range
/// so PDF sections retain native page numbers rather than being
/// pinned to a single document-wide range.
fn build_nested_with_ranges(flat: Vec<(u8, String, String, (u32, u32))>) -> Vec<Section> {
    let mut roots: Vec<Section> = Vec::new();
    let mut stack: Vec<*mut Section> = Vec::new();
    let roots_ptr: *mut Vec<Section> = &mut roots;
    for (level, heading, body, page_range) in flat {
        while let Some(top) = stack.last() {
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
        // SAFETY: see `build_nested` — same pattern, with pointers
        // into a tree we exclusively own.
        let parent_vec: &mut Vec<Section> = unsafe { &mut *parent_children };
        parent_vec.push(new_section);
        let added: *mut Section = parent_vec.last_mut().unwrap() as *mut _;
        stack.push(added);
    }
    roots
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

    use super::super::loading::PageLayout;

    fn line(text: &str, size: f32, y: f32) -> PageLine {
        PageLine {
            text: text.into(),
            bbox: BBox {
                x: 50.0,
                y,
                w: 400.0,
                h: size,
            },
            dominant_font_size: size,
            is_bold: false,
        }
    }

    fn page(n: u32, lines: Vec<PageLine>) -> PageLayout {
        page_with_tables(n, lines, Vec::new())
    }

    fn page_with_tables(n: u32, lines: Vec<PageLine>, tables: Vec<PageTable>) -> PageLayout {
        PageLayout {
            page_number: n,
            spans: Vec::new(),
            lines,
            tables,
            path_count: 0,
            image_count: 0,
            flat_text: String::new(),
        }
    }

    /// Build a `LoadedSource` whose lines collectively put body-size
    /// 12.0 above the `MIN_CLUSTER_OCCURRENCES` floor (otherwise no
    /// cluster would qualify and the heading detector trivially
    /// matches anything). Repeats the requested body line so the
    /// 12pt cluster crosses the threshold.
    fn loaded_with_body_padding(pages: Vec<PageLayout>) -> LoadedSource {
        let mut padded = pages;
        // Append a final page of body filler to ensure the body
        // cluster (size 12) has enough samples to be detected as
        // the body baseline.
        let last_page_no = padded.last().map(|p| p.page_number).unwrap_or(1) + 1;
        let filler_lines: Vec<PageLine> = (0..MIN_CLUSTER_OCCURRENCES + 2)
            .map(|i| line("filler body line", 12.0, 700.0 - i as f32 * 15.0))
            .collect();
        padded.push(page(last_page_no, filler_lines));
        LoadedSource {
            kind: SourceKind::Pdf,
            pages: padded,
            pdf: None,
        }
    }

    #[test]
    fn pdf_spans_two_top_level_sections_with_native_page_ranges() {
        // Introduction (size 20) appears on page 1 with one body line,
        // body continues onto page 2, Architecture (size 20) opens on
        // page 3 with its own body line.
        let pages = vec![
            page(
                1,
                vec![
                    line("Introduction", 20.0, 720.0),
                    line("body line one", 12.0, 700.0),
                ],
            ),
            page(2, vec![line("body line two", 12.0, 720.0)]),
            page(
                3,
                vec![
                    line("Architecture", 20.0, 720.0),
                    line("arch body", 12.0, 700.0),
                ],
            ),
        ];
        let loaded = loaded_with_body_padding(pages);
        let mut warnings = Vec::new();
        let tree = parse_pdf_spans(&loaded, &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 2, "{:?}", tree.roots);
        assert_eq!(tree.roots[0].heading, "Introduction");
        assert_eq!(tree.roots[0].page_range, (1, 2));
        assert!(tree.roots[0].body.contains("body line one"));
        assert!(tree.roots[0].body.contains("body line two"));
        assert_eq!(tree.roots[1].heading, "Architecture");
        assert_eq!(tree.roots[1].page_range.0, 3);
        assert!(tree.roots[1].body.contains("arch body"));
    }

    #[test]
    fn pdf_spans_nests_h2_under_h1_by_cluster_id() {
        // H1 (size 24) > H2 (size 16) > body (size 12). Repeat each
        // heading several times across pages so every heading-size
        // cluster crosses MIN_CLUSTER_OCCURRENCES.
        let mut pages = Vec::new();
        // Five H1s, each followed by body.
        for p in 1..=5 {
            pages.push(page(
                p,
                vec![
                    line(&format!("Chapter {p}"), 24.0, 720.0),
                    line("intro paragraph", 12.0, 700.0),
                    line(&format!("Section {p}.1"), 16.0, 680.0),
                    line("sub paragraph", 12.0, 660.0),
                    line("more sub paragraph", 12.0, 640.0),
                    line("more body", 12.0, 620.0),
                    line("more body", 12.0, 600.0),
                ],
            ));
        }
        let loaded = LoadedSource {
            kind: SourceKind::Pdf,
            pages,
            pdf: None,
        };
        let mut warnings = Vec::new();
        let tree = parse_pdf_spans(&loaded, &mut warnings).unwrap();
        // Expect five chapters, each containing a child section.
        assert_eq!(tree.roots.len(), 5);
        let first = &tree.roots[0];
        assert_eq!(first.heading, "Chapter 1");
        assert_eq!(first.level, 1);
        assert_eq!(first.children.len(), 1);
        let sub = &first.children[0];
        assert_eq!(sub.heading, "Section 1.1");
        assert_eq!(sub.level, 2);
        assert_eq!(
            sub.breadcrumb,
            vec!["Chapter 1".to_string(), "Section 1.1".to_string()]
        );
    }

    #[test]
    fn pdf_spans_skips_table_header_lines_for_heading_promotion() {
        // A table sits on page 1 with bold mid-size header text in
        // the first row. The parser must not promote the table
        // header line into a heading — there are no other
        // heading-size clusters, so the result is the degenerate
        // (document) root.
        let table_bbox = BBox {
            x: 40.0,
            y: 600.0,
            w: 500.0,
            h: 80.0,
        };
        let header_line = PageLine {
            text: "Signal | Direction | To/From | Description".into(),
            bbox: BBox {
                x: 40.0,
                y: 660.0,
                w: 500.0,
                h: 16.0,
            },
            // Bigger than body so it would otherwise tempt the
            // heading detector — table guard must suppress it.
            dominant_font_size: 16.0,
            is_bold: true,
        };
        let body_lines: Vec<PageLine> = (0..MIN_CLUSTER_OCCURRENCES + 1)
            .map(|i| line("table cell text", 12.0, 640.0 - i as f32 * 15.0))
            .collect();
        let mut p1_lines = vec![header_line];
        p1_lines.extend(body_lines);
        let table = PageTable {
            bbox: table_bbox,
            row_count: 3,
            col_count: 4,
            has_header: true,
            header_row: vec![
                "Signal".into(),
                "Direction".into(),
                "To/From".into(),
                "Description".into(),
            ],
            rows: vec![vec![
                "Signal".into(),
                "Direction".into(),
                "To/From".into(),
                "Description".into(),
            ]],
        };
        let pages = vec![page_with_tables(1, p1_lines, vec![table])];
        let loaded = LoadedSource {
            kind: SourceKind::Pdf,
            pages,
            pdf: None,
        };
        let mut warnings = Vec::new();
        let tree = parse_pdf_spans(&loaded, &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "(document)");
        assert!(warnings.iter().any(|w| w.code == "no_headings_detected"));
    }

    #[test]
    fn pdf_spans_no_heading_clusters_emits_degenerate_root_with_warning() {
        // Every line is body-size; no cluster qualifies as a
        // heading. We expect the degenerate `(document)` root and
        // a `no_headings_detected` warning.
        let body_lines: Vec<PageLine> = (0..(MIN_CLUSTER_OCCURRENCES + 3))
            .map(|i| line(&format!("body line {i}"), 12.0, 700.0 - i as f32 * 15.0))
            .collect();
        let pages = vec![page(1, body_lines)];
        let loaded = LoadedSource {
            kind: SourceKind::Pdf,
            pages,
            pdf: None,
        };
        let mut warnings = Vec::new();
        let tree = parse_pdf_spans(&loaded, &mut warnings).unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].heading, "(document)");
        assert!(warnings.iter().any(|w| w.code == "no_headings_detected"));
    }
}
