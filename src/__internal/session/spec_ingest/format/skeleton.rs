//! Structural skeleton builder (Phase 9 milestone 9.3).
//!
//! Walks a [`LoadedSource`] from the loading stage and emits a
//! compact deterministic structural map — `Skeleton` — that the
//! format-discovery LLM critique pass consumes alongside the
//! first-cut descriptor. The skeleton intentionally avoids raw
//! page text: it carries the headings (font-clustered), the
//! tables (first row + dimensions), the figures (page +
//! neighbouring heading), the parenthesised acronym candidates,
//! and the repeated chrome lines.
//!
//! The textual rendering [`Skeleton::render`] follows the
//! Architecture Chapter 7 §7.6 layout (`# DOCUMENT`,
//! `# HEADINGS`, `# TABLES`, `# FIGURES`,
//! `# ACRONYM CANDIDATES`). Output is deterministic — every
//! list is sorted by stable keys (page + line_y, table id,
//! figure id, acronym text) so two builds of the same loaded
//! source render byte-equal.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use regex::Regex;

use crate::session::spec_ingest::pipeline::IngestConfig;
use crate::session::spec_ingest::stages::loading::{BBox, FontWeight, LoadedSource, PageLayout};

/// Top-level structural skeleton (Chapter 7 §7.6).
///
/// Carries the deterministic information the LLM critique pass
/// needs: a per-document summary, the heading list, the table
/// list, the figure list, the parenthesised acronym candidates,
/// and the repeated chrome lines. The shape is intentionally
/// flat so the renderer can emit one section per field without
/// further traversal.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    pub document: DocumentSummary,
    pub headings: Vec<HeadingEntry>,
    pub tables: Vec<TableEntry>,
    pub figures: Vec<FigureEntry>,
    pub acronym_candidates: Vec<AcronymCandidate>,
    pub chrome_repeated_lines: Vec<String>,
}

/// Document-level summary: page count, font clusters,
/// source-kind tag. Matches the `# DOCUMENT` section in
/// Chapter 7 §7.6.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSummary {
    pub total_pages: u32,
    pub font_clusters: Vec<FontCluster>,
    /// One of `"pdf" | "markdown" | "text" | "none"` — mirrors
    /// [`crate::session::spec_ingest::pipeline::SourceKind::as_manifest_tag`].
    pub source_kind: String,
}

/// One font cluster: a binned (±0.5 pt) font size, the number
/// of spans observed at that size, the typical weight, and a
/// stable cluster id (0 = largest, 1 = next, ...). Heading
/// detection assigns level = cluster_id + 1.
#[derive(Debug, Clone, PartialEq)]
pub struct FontCluster {
    pub size: f32,
    pub frequency: u32,
    pub typical_weight: FontWeight,
    /// Cluster identifier (0 = largest, 1 = next, ...).
    pub cluster_id: u32,
}

/// One detected heading line. `level` is the cluster index +1
/// clamped to `[1, 6]`. `line_y` is the top-of-line Y in PDF
/// user-space points so the LLM can disambiguate stacked
/// headings on the same page.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadingEntry {
    pub page: u32,
    pub level: u8,
    pub text: String,
    pub font_size: f32,
    pub is_bold: bool,
    pub line_y: f32,
    pub cluster_id: u32,
}

/// One detected table. `id` is a stable `"T01"`-style label
/// assigned in document order. `header_row` is the first row's
/// cells; `first_data_row` is the second row's cells (or empty
/// when the table has only one row).
#[derive(Debug, Clone, PartialEq)]
pub struct TableEntry {
    pub id: String,
    pub page: u32,
    pub row_count: u32,
    pub col_count: u32,
    pub header_row: Vec<String>,
    pub first_data_row: Vec<String>,
    pub bbox: BBox,
}

/// One detected figure candidate (vector-rich page or page with
/// embedded raster). `id` is a stable `"F01"`-style label.
/// `raster_path` is the rendered PNG's relative path inside the
/// emit corpus.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureEntry {
    pub id: String,
    pub page: u32,
    pub raster_path: String,
    pub neighbouring_heading: Option<String>,
    pub vector_path_count: u32,
    pub embedded_image_count: u32,
}

/// One parenthesised acronym candidate detected via the
/// `<Name> (<ACR>)` regex. `later_usage_count` is the number of
/// plain-acronym matches after the first parenthesised mention.
#[derive(Debug, Clone, PartialEq)]
pub struct AcronymCandidate {
    pub acronym: String,
    pub expansion: String,
    pub first_page: u32,
    /// Count of plain-acronym mentions AFTER the first
    /// parenthesised mention (i.e. on the first page after the
    /// match, plus every later page).
    pub later_usage_count: u32,
}

/// Build a [`Skeleton`] from the loaded source using built-in
/// defaults. Convenience wrapper over [`build_skeleton_with`]
/// that supplies a zero-config `IngestConfig`; the default
/// `figures.vector_op_threshold` of 20 is used to decide which
/// pages qualify as figure candidates.
pub fn build_skeleton(loaded: &LoadedSource) -> Skeleton {
    build_skeleton_with(loaded, &IngestConfig::default())
}

/// Build a [`Skeleton`] from the loaded source honouring the
/// caller's `IngestConfig` (only `figures.vector_op_threshold`
/// is consulted today). The function is deterministic: the
/// same `(loaded, config)` pair produces identical output.
pub fn build_skeleton_with(loaded: &LoadedSource, config: &IngestConfig) -> Skeleton {
    let total_pages = loaded.pages.len() as u32;
    let source_kind = loaded.kind.as_manifest_tag().to_string();

    let font_clusters = build_font_clusters(&loaded.pages);
    let chrome_repeated_lines = build_chrome_repeated_lines(&loaded.pages);

    let headings = build_headings(&loaded.pages, &font_clusters, &chrome_repeated_lines);
    let tables = build_tables(&loaded.pages);
    let figures = build_figures(&loaded.pages, &headings, config.figures.vector_op_threshold);
    let acronym_candidates = build_acronym_candidates(&loaded.pages);

    Skeleton {
        document: DocumentSummary {
            total_pages,
            font_clusters,
            source_kind,
        },
        headings,
        tables,
        figures,
        acronym_candidates,
        chrome_repeated_lines,
    }
}

impl Skeleton {
    /// Render the skeleton in the Chapter 7 §7.6 textual format
    /// the LLM consumes. Output is deterministic: every list is
    /// already sorted by stable keys at build time so two calls
    /// on the same skeleton return byte-equal strings.
    pub fn render(&self) -> String {
        let mut out = String::new();

        out.push_str("# DOCUMENT\n");
        out.push_str(&format!("total_pages: {}\n", self.document.total_pages));
        out.push_str("font_clusters: [");
        let mut first = true;
        for cluster in &self.document.font_clusters {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(&format!(
                "{{size: {:.1}, freq: {}, weight: {}, cluster: {}}}",
                cluster.size,
                cluster.frequency,
                weight_str(cluster.typical_weight),
                cluster.cluster_id,
            ));
        }
        out.push_str("]\n");
        out.push_str(&format!("source_kind: {}\n", self.document.source_kind));
        out.push_str("detected_chrome_repeated_lines: [");
        let mut first = true;
        for line in &self.chrome_repeated_lines {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push('"');
            out.push_str(&escape_quoted(line));
            out.push('"');
        }
        out.push_str("]\n");

        out.push_str("\n# HEADINGS (font-clustered)\n");
        for h in &self.headings {
            out.push_str(&format!(
                "[L{}] p{} \"{}\"  (size={:.1}, {})\n",
                h.level,
                h.page,
                escape_quoted(&h.text),
                h.font_size,
                if h.is_bold { "bold" } else { "normal" },
            ));
        }

        out.push_str("\n# TABLES\n");
        for t in &self.tables {
            let header_preview = t.header_row.join(" | ");
            out.push_str(&format!(
                "[{}] p{} {}x{} \"{}\"\n",
                t.id, t.page, t.row_count, t.col_count, header_preview,
            ));
            if !t.first_data_row.is_empty() {
                let first_data = t.first_data_row.join(" | ");
                out.push_str(&format!("        first row: \"{first_data}\"\n"));
            }
        }

        out.push_str("\n# FIGURES\n");
        for f in &self.figures {
            let neighbour = f.neighbouring_heading.as_deref().unwrap_or("");
            out.push_str(&format!(
                "[{}] p{} {}   neighbouring_heading: \"{}\"\n",
                f.id,
                f.page,
                f.raster_path,
                escape_quoted(neighbour),
            ));
        }

        out.push_str("\n# ACRONYM CANDIDATES\n");
        for a in &self.acronym_candidates {
            out.push_str(&format!(
                "\"{}\" ({}) @ p{} — uses {} times after\n",
                escape_quoted(&a.expansion),
                a.acronym,
                a.first_page,
                a.later_usage_count,
            ));
        }

        out
    }
}

fn weight_str(w: FontWeight) -> &'static str {
    match w {
        FontWeight::Bold => "bold",
        FontWeight::Normal => "normal",
    }
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Walk every span in every page, bin by rounded font size,
/// pick the typical weight per bin, sort descending by
/// frequency, and assign cluster ids.
fn build_font_clusters(pages: &[PageLayout]) -> Vec<FontCluster> {
    // Bin by font size rounded to one decimal (≈ ±0.5 pt
    // tolerance is implicit in the round-half-to-even step the
    // platform's `(x * 10).round() / 10` performs).
    let mut bins: BTreeMap<i32, (f32, u32, u32)> = BTreeMap::new();
    for page in pages {
        for span in &page.spans {
            let bin_key = (span.font_size * 10.0).round() as i32;
            let bin_size = (bin_key as f32) / 10.0;
            let entry = bins.entry(bin_key).or_insert((bin_size, 0, 0));
            entry.1 += 1;
            if matches!(span.font_weight, FontWeight::Bold) {
                entry.2 += 1;
            }
        }
    }

    let mut clusters: Vec<(f32, u32, FontWeight)> = bins
        .into_values()
        .map(|(size, freq, bold_count)| {
            let weight = if bold_count * 2 > freq {
                FontWeight::Bold
            } else {
                FontWeight::Normal
            };
            (size, freq, weight)
        })
        .collect();

    // Largest font size first (the spec's heading clusters are
    // bigger than body text); break ties by frequency. This
    // matches Chapter 7 §7.6 where `[L1]` is the largest cluster.
    clusters.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
    });

    clusters
        .into_iter()
        .enumerate()
        .map(|(idx, (size, freq, weight))| FontCluster {
            size,
            frequency: freq,
            typical_weight: weight,
            cluster_id: idx as u32,
        })
        .collect()
}

/// Collect lines that repeat on ≥ 60% of pages. Comparison is
/// case-insensitive on trimmed text; the returned strings are
/// the lower-cased canonical form, deduplicated and sorted.
fn build_chrome_repeated_lines(pages: &[PageLayout]) -> Vec<String> {
    if pages.len() < 2 {
        return Vec::new();
    }
    let total = pages.len();
    let mut per_line_pages: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for page in pages {
        let mut seen_this_page: BTreeSet<String> = BTreeSet::new();
        for line in &page.lines {
            let key = line.text.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            if seen_this_page.insert(key.clone()) {
                per_line_pages
                    .entry(key)
                    .or_default()
                    .insert(page.page_number);
            }
        }
    }
    let threshold = (total as f32) * 0.6;
    let mut out: Vec<String> = per_line_pages
        .into_iter()
        .filter(|(_, set)| (set.len() as f32) >= threshold)
        .map(|(k, _)| k)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Detect heading candidates by font cluster. A line is a
/// heading if its dominant font size matches one of the top K
/// clusters by size (K=4) that ALSO clears the
/// `MIN_CLUSTER_FREQUENCY` floor AND is smaller-or-equal in
/// frequency to body text. Concretely: the most-frequent
/// cluster (body) is excluded — heading clusters are larger
/// than body but occur less often.  Lines whose trimmed text
/// matches a chrome repeated line are rejected.
fn build_headings(
    pages: &[PageLayout],
    font_clusters: &[FontCluster],
    chrome_repeated_lines: &[String],
) -> Vec<HeadingEntry> {
    const MAX_HEADING_CLUSTERS: usize = 4;
    const MIN_CLUSTER_FREQUENCY: u32 = 5;

    let chrome_set: BTreeSet<&str> = chrome_repeated_lines.iter().map(|s| s.as_str()).collect();

    // Identify body text: the cluster with the highest
    // frequency. Heading clusters are strictly less frequent
    // than body (or equal — only if there's a single cluster
    // total, in which case the document has no headings).
    let max_freq = font_clusters.iter().map(|c| c.frequency).max().unwrap_or(0);

    // Pick the heading clusters: top K (by size desc, which is
    // the order in `font_clusters`) that have frequency
    // strictly less than body's. Cluster IDs are preserved so
    // heading levels (= cluster_id + 1) stay stable.
    let heading_clusters: Vec<&FontCluster> = font_clusters
        .iter()
        .filter(|c| c.frequency >= MIN_CLUSTER_FREQUENCY && c.frequency < max_freq)
        .take(MAX_HEADING_CLUSTERS)
        .collect();

    let mut out: Vec<HeadingEntry> = Vec::new();
    for page in pages {
        for line in &page.lines {
            let trimmed = line.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if chrome_set.contains(trimmed.to_lowercase().as_str()) {
                continue;
            }
            // Find a matching heading cluster (font size within
            // 0.05 pt — same tolerance as the binning step).
            let Some(cluster) = heading_clusters
                .iter()
                .find(|c| (c.size - line.dominant_font_size).abs() < 0.05)
            else {
                continue;
            };
            let level = ((cluster.cluster_id as u8) + 1).clamp(1, 6);
            // `bbox.y` is the top of the line in pdf_oxide's
            // coordinate space (Y grows downward in the line
            // boxes pdf_oxide reports; we forward as-is).
            out.push(HeadingEntry {
                page: page.page_number,
                level,
                text: trimmed.to_string(),
                font_size: line.dominant_font_size,
                is_bold: line.is_bold,
                line_y: line.bbox.y,
                cluster_id: cluster.cluster_id,
            });
        }
    }

    // Sort by (page, descending line_y) — top of page first.
    // pdf_oxide reports PDF user-space where Y grows upward, so
    // a larger Y is higher on the page; this matches the §7.6
    // example which lists headings top-to-bottom per page.
    out.sort_by(|a, b| {
        a.page.cmp(&b.page).then_with(|| {
            b.line_y
                .partial_cmp(&a.line_y)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    out
}

/// Walk every page's tables and assign stable `"T01"`-style
/// ids in document order. The first row becomes `header_row`;
/// the second (if any) becomes `first_data_row`.
fn build_tables(pages: &[PageLayout]) -> Vec<TableEntry> {
    let mut out: Vec<TableEntry> = Vec::new();
    let mut counter: u32 = 0;
    for page in pages {
        for table in &page.tables {
            counter += 1;
            let header_row = table.header_row.clone();
            let first_data_row = table.rows.get(1).cloned().unwrap_or_default();
            out.push(TableEntry {
                id: format!("T{counter:02}"),
                page: page.page_number,
                row_count: table.row_count,
                col_count: table.col_count,
                header_row,
                first_data_row,
                bbox: table.bbox,
            });
        }
    }
    out
}

/// Detect figure candidates. A page is a figure candidate
/// when its `path_count >= threshold` OR `image_count >= 1`.
/// `neighbouring_heading` is the most-recent heading whose
/// page ≤ this page in reading order — preferring the first
/// (topmost) heading on the same page when one exists, else
/// the last heading on an earlier page (the §7.6 example's
/// behaviour: F02 on p13 neighbours the page-11 IF heading
/// when no heading lives on p13 itself).
fn build_figures(
    pages: &[PageLayout],
    headings: &[HeadingEntry],
    vector_op_threshold: u32,
) -> Vec<FigureEntry> {
    let mut out: Vec<FigureEntry> = Vec::new();
    let mut counter: u32 = 0;
    for page in pages {
        if page.path_count < vector_op_threshold && page.image_count == 0 {
            continue;
        }
        counter += 1;
        // Same-page heading (first one in reading order — the
        // headings list is sorted by ascending page then
        // descending line_y, so `.find` on the same page picks
        // the topmost heading).
        let neighbouring_heading =
            if let Some(h) = headings.iter().find(|h| h.page == page.page_number) {
                Some(h.text.clone())
            } else {
                headings
                    .iter()
                    .rfind(|h| h.page < page.page_number)
                    .map(|h| h.text.clone())
            };
        out.push(FigureEntry {
            id: format!("F{counter:02}"),
            page: page.page_number,
            raster_path: format!("figures/page-{:03}.png", page.page_number),
            neighbouring_heading,
            vector_path_count: page.path_count,
            embedded_image_count: page.image_count,
        });
    }
    out
}

/// Detect parenthesised acronym candidates and count later
/// plain-acronym usages. The regex matches `<Expansion> (ACR)`
/// where `<Expansion>` is one or more capitalised words and
/// `ACR` is 2-6 upper-case alnum chars. Later usage counts
/// only fire after the first parenthesised mention (same page
/// or later).
fn build_acronym_candidates(pages: &[PageLayout]) -> Vec<AcronymCandidate> {
    // First mention regex: an expansion (1+ capitalised words,
    // optional internal hyphens / multi-word) followed by an
    // acronym in parentheses.
    let first_re =
        Regex::new(r"\b([A-Z][A-Za-z\-]+(?:\s+[A-Z][A-Za-z\-]+)*)\s+\(([A-Z][A-Z0-9]{1,5})\)")
            .expect("acronym regex");

    // Per-page concatenated text. Using line text (not spans)
    // keeps the regex from matching across span boundaries that
    // wouldn't be visible to a human reader.
    let page_texts: Vec<(u32, String)> = pages
        .iter()
        .map(|p| {
            let mut buf = String::new();
            // Prefer line text (more readable boundaries); fall
            // back to flat_text for non-PDF sources where the
            // structured lines are empty.
            if p.lines.is_empty() {
                buf.push_str(&p.flat_text);
            } else {
                for line in &p.lines {
                    buf.push_str(&line.text);
                    buf.push('\n');
                }
            }
            (p.page_number, buf)
        })
        .collect();

    // Record first parenthesised match per acronym.
    let mut first_mentions: BTreeMap<String, (String, u32)> = BTreeMap::new();
    for (page_num, text) in &page_texts {
        for cap in first_re.captures_iter(text) {
            let expansion = cap
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            let acronym = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            if acronym.is_empty() {
                continue;
            }
            first_mentions
                .entry(acronym)
                .or_insert((expansion, *page_num));
        }
    }

    let mut out: Vec<AcronymCandidate> = Vec::new();
    for (acronym, (expansion, first_page)) in first_mentions {
        // Plain-acronym regex must escape the acronym text.
        let plain_re = match Regex::new(&format!(r"\b{}\b", regex::escape(&acronym))) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Count plain-acronym usages strictly after the first
        // parenthesised mention. We scan from the first-mention
        // page's text after the first match's end through every
        // later page.
        let mut later_count: u32 = 0;
        for (page_num, text) in &page_texts {
            if *page_num < first_page {
                continue;
            }
            if *page_num == first_page {
                // Find the first parenthesised match's end and
                // count plain matches after it.
                if let Some(first) = first_re.find_iter(text).find(|m| {
                    // Match must be the parenthesised mention
                    // for THIS acronym, not some other one on
                    // the same page.
                    m.as_str().ends_with(&format!("({acronym})"))
                }) {
                    let after = &text[first.end()..];
                    later_count += plain_re.find_iter(after).count() as u32;
                } else {
                    // No parenthesised match on this page even
                    // though first_mentions says there should
                    // be — count all plain matches as a fallback.
                    later_count += plain_re.find_iter(text).count() as u32;
                }
            } else {
                later_count += plain_re.find_iter(text).count() as u32;
            }
        }
        out.push(AcronymCandidate {
            acronym,
            expansion,
            first_page,
            later_usage_count: later_count,
        });
    }

    // Stable order: by acronym ascending.
    out.sort_by(|a, b| a.acronym.cmp(&b.acronym));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_ingest::pipeline::SourceKind;
    use crate::session::spec_ingest::stages::loading::{
        BBox as LBBox, PageLine, PageSpan, PageTable,
    };

    fn span(text: &str, size: f32, weight: FontWeight, y: f32) -> PageSpan {
        PageSpan {
            text: text.to_string(),
            bbox: LBBox {
                x: 50.0,
                y,
                w: 100.0,
                h: size,
            },
            font_size: size,
            font_weight: weight,
            is_italic: false,
        }
    }

    fn line(text: &str, size: f32, bold: bool, y: f32) -> PageLine {
        PageLine {
            text: text.to_string(),
            bbox: LBBox {
                x: 50.0,
                y,
                w: 200.0,
                h: size,
            },
            dominant_font_size: size,
            is_bold: bold,
        }
    }

    fn make_table(header: &[&str], data_rows: &[&[&str]]) -> PageTable {
        let mut rows: Vec<Vec<String>> = Vec::new();
        rows.push(header.iter().map(|c| c.to_string()).collect());
        for r in data_rows {
            rows.push(r.iter().map(|c| c.to_string()).collect());
        }
        let row_count = rows.len() as u32;
        let col_count = header.len() as u32;
        PageTable {
            bbox: LBBox {
                x: 40.0,
                y: 200.0,
                w: 400.0,
                h: 80.0,
            },
            row_count,
            col_count,
            has_header: true,
            header_row: header.iter().map(|c| c.to_string()).collect(),
            rows,
        }
    }

    /// Two-page synthetic source: each page has three large
    /// heading spans (so the heading cluster has ≥ 5 occurrences
    /// across the document) plus body spans and a table or
    /// image. The headings carry the §7.6 parenthesised-acronym
    /// shape so the acronym detector also has something to find.
    fn synthetic_two_page_source() -> LoadedSource {
        // Page 1: three heading-sized spans plus several body
        // spans (so the body cluster strictly out-frequents
        // the heading cluster, the canonical spec layout) and
        // a signal-table-shaped table.
        let page1_spans = vec![
            // Heading-sized spans (large font, bold).
            span("Instruction Fetch (IF)", 18.0, FontWeight::Bold, 700.0),
            span("Overview", 18.0, FontWeight::Bold, 500.0),
            span("Details", 18.0, FontWeight::Bold, 300.0),
            // Body spans (smaller font).
            span("The IF stage fetches.", 11.0, FontWeight::Normal, 660.0),
            span("More body text.", 11.0, FontWeight::Normal, 640.0),
            span("Yet more body.", 11.0, FontWeight::Normal, 620.0),
            span("Even more body.", 11.0, FontWeight::Normal, 600.0),
            span("Body continues here.", 11.0, FontWeight::Normal, 580.0),
            span("And keeps going.", 11.0, FontWeight::Normal, 560.0),
        ];
        let page1_lines = vec![
            line("Instruction Fetch (IF)", 18.0, true, 700.0),
            line("The IF stage fetches.", 11.0, false, 660.0),
            line("More body text.", 11.0, false, 640.0),
            line("Yet more body.", 11.0, false, 620.0),
            line("Overview", 18.0, true, 500.0),
            line("Details", 18.0, true, 300.0),
        ];
        let page1_tables = vec![make_table(
            &["Signal", "Direction", "To/From", "Description"],
            &[&["if_pc", "out", "Bus", "Next PC value"]],
        )];

        // Page 2: three heading-sized spans plus several body
        // spans (mirrors page 1) and a figure (image_count=1).
        let page2_spans = vec![
            span("Pre-Decode (PD)", 18.0, FontWeight::Bold, 700.0),
            span("Behaviour", 18.0, FontWeight::Bold, 500.0),
            span("Notes", 18.0, FontWeight::Bold, 300.0),
            span("PD stage decodes.", 11.0, FontWeight::Normal, 660.0),
            span("IF feeds into PD.", 11.0, FontWeight::Normal, 640.0),
            span("Body continues.", 11.0, FontWeight::Normal, 620.0),
            span("Page 2 body line.", 11.0, FontWeight::Normal, 600.0),
            span("More page 2 prose.", 11.0, FontWeight::Normal, 580.0),
            span("Yet more page 2.", 11.0, FontWeight::Normal, 560.0),
        ];
        let page2_lines = vec![
            line("Pre-Decode (PD)", 18.0, true, 700.0),
            line("PD stage decodes.", 11.0, false, 660.0),
            line("IF feeds into PD.", 11.0, false, 640.0),
            line("Body continues.", 11.0, false, 620.0),
            line("Behaviour", 18.0, true, 500.0),
            line("Notes", 18.0, true, 300.0),
        ];

        let pages = vec![
            PageLayout {
                page_number: 1,
                spans: page1_spans,
                lines: page1_lines,
                tables: page1_tables,
                path_count: 0,
                image_count: 0,
                flat_text: "Instruction Fetch (IF)\nThe IF stage fetches.\nMore body text.\n"
                    .to_string(),
            },
            PageLayout {
                page_number: 2,
                spans: page2_spans,
                lines: page2_lines,
                tables: Vec::new(),
                path_count: 0,
                image_count: 1,
                flat_text: "Pre-Decode (PD)\nPD stage decodes.\nIF feeds into PD.\n".to_string(),
            },
        ];

        LoadedSource {
            kind: SourceKind::Pdf,
            pages,
            pdf: None,
        }
    }

    #[test]
    fn build_skeleton_extracts_pages_and_clusters() {
        let loaded = synthetic_two_page_source();
        let skel = build_skeleton(&loaded);
        assert_eq!(skel.document.total_pages, 2);
        assert_eq!(skel.document.source_kind, "pdf");
        // Two font sizes observed: 18.0 (2 spans) and 11.0
        // (4 spans). Body-text cluster has the higher freq.
        assert!(skel.document.font_clusters.len() >= 2);
        // The largest size is cluster 0.
        assert_eq!(skel.document.font_clusters[0].size, 18.0);
    }

    #[test]
    fn build_skeleton_detects_headings() {
        let loaded = synthetic_two_page_source();
        let skel = build_skeleton(&loaded);
        // Six heading lines (three per page); they sort by
        // (page, descending line_y), so the page-1 top heading
        // ("Instruction Fetch (IF)") leads.
        assert_eq!(skel.headings.len(), 6, "headings: {:?}", skel.headings);
        assert_eq!(skel.headings[0].page, 1);
        assert_eq!(skel.headings[0].text, "Instruction Fetch (IF)");
        assert_eq!(skel.headings[0].level, 1);
        assert!(skel.headings[0].is_bold);
        // The first heading on page 2 (top of page) is
        // "Pre-Decode (PD)".
        let first_p2 = skel
            .headings
            .iter()
            .find(|h| h.page == 2)
            .expect("page-2 heading");
        assert_eq!(first_p2.text, "Pre-Decode (PD)");
    }

    #[test]
    fn build_skeleton_extracts_tables() {
        let loaded = synthetic_two_page_source();
        let skel = build_skeleton(&loaded);
        assert_eq!(skel.tables.len(), 1);
        assert_eq!(skel.tables[0].id, "T01");
        assert_eq!(skel.tables[0].page, 1);
        assert_eq!(skel.tables[0].col_count, 4);
        assert_eq!(skel.tables[0].header_row[0], "Signal");
        assert_eq!(skel.tables[0].first_data_row[0], "if_pc");
    }

    #[test]
    fn build_skeleton_extracts_figures() {
        let loaded = synthetic_two_page_source();
        let skel = build_skeleton(&loaded);
        // Page 2 has image_count=1 so it qualifies as a figure
        // candidate regardless of vector_op_threshold.
        assert_eq!(skel.figures.len(), 1);
        assert_eq!(skel.figures[0].id, "F01");
        assert_eq!(skel.figures[0].page, 2);
        assert_eq!(skel.figures[0].raster_path, "figures/page-002.png");
        assert_eq!(skel.figures[0].embedded_image_count, 1);
        // Neighbouring heading is the most-recent heading whose
        // page ≤ 2 — that's the page-2 heading itself.
        assert_eq!(
            skel.figures[0].neighbouring_heading.as_deref(),
            Some("Pre-Decode (PD)")
        );
    }

    #[test]
    fn acronym_detection_counts_later_usages() {
        // Single-page source with the §7.6 acronym shape and
        // two plain post-mention usages: "IF stage" and "IF ".
        let lines = vec![
            line(
                "Instruction Fetch (IF) is the first stage.",
                11.0,
                false,
                700.0,
            ),
            line("The IF stage fetches.", 11.0, false, 680.0),
            line("Later in IF the PC advances.", 11.0, false, 660.0),
        ];
        let spans = vec![span(
            "Instruction Fetch (IF) is the first stage.",
            11.0,
            FontWeight::Normal,
            700.0,
        )];
        let flat = "Instruction Fetch (IF) is the first stage.\nThe IF stage fetches.\nLater in IF the PC advances.\n";
        let loaded = LoadedSource {
            kind: SourceKind::Pdf,
            pages: vec![PageLayout {
                page_number: 1,
                spans,
                lines,
                tables: Vec::new(),
                path_count: 0,
                image_count: 0,
                flat_text: flat.to_string(),
            }],
            pdf: None,
        };
        let skel = build_skeleton(&loaded);
        assert_eq!(skel.acronym_candidates.len(), 1);
        let ac = &skel.acronym_candidates[0];
        assert_eq!(ac.acronym, "IF");
        assert_eq!(ac.expansion, "Instruction Fetch");
        assert_eq!(ac.first_page, 1);
        assert_eq!(ac.later_usage_count, 2);
    }

    #[test]
    fn render_emits_section_headers() {
        let loaded = synthetic_two_page_source();
        let skel = build_skeleton(&loaded);
        let rendered = skel.render();
        assert!(rendered.contains("# DOCUMENT"), "{rendered}");
        assert!(rendered.contains("# HEADINGS"), "{rendered}");
        assert!(rendered.contains("# TABLES"), "{rendered}");
        assert!(rendered.contains("# FIGURES"), "{rendered}");
        assert!(rendered.contains("# ACRONYM CANDIDATES"), "{rendered}");
        assert!(rendered.contains("source_kind: pdf"), "{rendered}");
        assert!(rendered.contains("total_pages: 2"), "{rendered}");
        assert!(rendered.contains("[T01]"), "{rendered}");
        assert!(rendered.contains("[F01]"), "{rendered}");
    }

    #[test]
    fn render_is_deterministic() {
        let loaded = synthetic_two_page_source();
        let first = build_skeleton(&loaded).render();
        let second = build_skeleton(&loaded).render();
        assert_eq!(first, second);
    }
}
