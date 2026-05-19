//! Stage 2: page-chrome stripping.
//!
//! See architecture §1.4 stage 2 and chapter 7 §7.3.6. Chrome is the
//! set of running headers, running footers, page numbers, footer
//! links, and watermarks that repeat on most pages but are not part
//! of the spec's semantic content. Three filters compose with OR
//! semantics — a span is chrome if **any** filter says so:
//!
//! 1. **Positional Y-banding** (always on, PDF only): spans whose
//!    bbox top edge falls in a horizontal stripe that contains text
//!    on ≥ 60% of pages are chrome. The stripe is a 10-pt bucket so
//!    minor per-page Y jitter still clusters.
//! 2. **Repeated-line text similarity** (always on): the legacy
//!    pre-9.10 behaviour. Lines whose exact normalised text appears
//!    on ≥ 60% of pages are chrome regardless of position. PDFs with
//!    inconsistent header positioning rely on this twin.
//! 3. **Regex filter** (only when a `FormatJson` descriptor is
//!    supplied): each `ChromeEntry::regex` is compiled and matched
//!    against every line. If the entry pins `y_band_pt`, the match
//!    also requires the line's bbox top edge to fall inside that
//!    band; otherwise the regex match alone is enough.
//!
//! Tables, vector paths, and embedded images are never chrome
//! candidates; only `spans` / `lines` participate.

use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;

use super::super::format::FormatJson;
use super::super::pipeline::{IngestConfig, IngestWarning, SourceKind};
use super::loading::{LoadedSource, PageLayout};

/// Default bucket height used by the positional band detector (PDF
/// user-space points). Wide enough to absorb sub-pt Y jitter on
/// consistent running headers, narrow enough that body text rows
/// don't accidentally cluster into a single chrome band.
const POSITIONAL_BUCKET_PT: f32 = 10.0;

/// Default fraction of pages on which a band must contain text to
/// count as chrome.
const POSITIONAL_PAGE_COVERAGE: f32 = 0.6;

/// If any regex matches more than this fraction of the lines on any
/// single page we emit a `chrome_over_match` warning so the operator
/// can re-run `--rediscover-format` to fix the descriptor.
const REGEX_OVER_MATCH_FRACTION: f32 = 0.8;

/// Legacy chrome record consumed by `stages::emit`. Only the strings
/// removed by the repeated-line filter and the per-page strip counts
/// are preserved here so the existing manifest format stays stable.
/// The richer `ChromeReport` is available to format-aware callers.
#[derive(Debug, Clone, Default)]
pub struct ChromeRecord {
    /// Lines stripped because they appeared on enough pages.
    pub repeated_lines: Vec<String>,
    /// Per-page count of stripped lines (parallel to the input page
    /// vector). Useful for diagnostics; the manifest summarises it.
    pub per_page_stripped: Vec<u32>,
}

/// Detailed chrome-detection report (Chapter 7 §7.3.6 telemetry).
/// Returned by [`strip_chrome_with_format`] alongside the mutated
/// page set; consumed by callers that surface the detected bands in
/// the descriptor's `validation` block.
#[derive(Debug, Clone, Default)]
pub struct ChromeReport {
    /// Total number of lines stripped across all pages (sum of the
    /// per-page counts).
    pub total_lines_stripped: u32,
    /// Detected top Y-band `(low, high)` if any.
    pub positional_band_top: Option<[f32; 2]>,
    /// Detected bottom Y-band `(low, high)` if any.
    pub positional_band_bottom: Option<[f32; 2]>,
    /// Per-regex match counts (regex string → count). Always present
    /// when a `FormatJson` is supplied even if zero matches fired.
    pub regex_match_counts: BTreeMap<String, u32>,
}

/// Strip page chrome with positional + repeated-line filters only.
/// Preserves the pre-9.10 public signature so the pipeline + emit
/// stages don't need to know about the format-aware overload.
pub fn strip_chrome(
    loaded: LoadedSource,
    config: &IngestConfig,
    warnings: &mut Vec<IngestWarning>,
) -> (LoadedSource, ChromeRecord) {
    let (loaded, record, _report) = strip_chrome_with_format(loaded, config, None, warnings);
    (loaded, record)
}

/// Strip page chrome, optionally driven by a `FormatJson` descriptor.
/// The descriptor's `chrome` regexes compose with the positional and
/// repeated-line filters. Returns a [`ChromeReport`] with detected
/// band positions + per-regex match counts; the mutated `LoadedSource`
/// has chrome spans / lines stripped and `flat_text` rederived from
/// the surviving lines.
pub fn strip_chrome_with_format(
    loaded: LoadedSource,
    config: &IngestConfig,
    format: Option<&FormatJson>,
    warnings: &mut Vec<IngestWarning>,
) -> (LoadedSource, ChromeRecord, ChromeReport) {
    if !matches!(loaded.kind, SourceKind::Pdf) {
        return (loaded, ChromeRecord::default(), ChromeReport::default());
    }
    let LoadedSource { kind, pages, pdf } = loaded;
    let (stripped_pages, record, report) = strip_chrome_pages(pages, config, format, warnings);
    (
        LoadedSource {
            kind,
            pages: stripped_pages,
            pdf,
        },
        record,
        report,
    )
}

fn strip_chrome_pages(
    pages: Vec<PageLayout>,
    config: &IngestConfig,
    format: Option<&FormatJson>,
    warnings: &mut Vec<IngestWarning>,
) -> (Vec<PageLayout>, ChromeRecord, ChromeReport) {
    if !config.chrome_stripping.enabled || pages.is_empty() {
        return (pages, ChromeRecord::default(), ChromeReport::default());
    }

    let threshold = config.chrome_stripping.appearance_threshold.clamp(0.0, 1.0);

    // ---- Repeated-line filter (legacy, text-similarity) -------------
    let repeated = detect_repeated_lines(&pages, threshold);

    // ---- Positional Y-banding filter --------------------------------
    let bands = detect_positional_bands(&pages, POSITIONAL_PAGE_COVERAGE);

    // ---- Regex filter (format-driven) -------------------------------
    let regex_filters = compile_regex_filters(format, warnings);

    let mut report = ChromeReport {
        positional_band_top: bands.top,
        positional_band_bottom: bands.bottom,
        ..ChromeReport::default()
    };
    for entry in &regex_filters {
        report
            .regex_match_counts
            .entry(entry.source.clone())
            .or_insert(0);
    }

    let mut repeated_lines_seen: HashSet<String> = HashSet::new();
    let mut per_page_stripped = Vec::with_capacity(pages.len());
    let mut out_pages = Vec::with_capacity(pages.len());

    for page in pages {
        let total_lines_on_page = page.lines.len().max(page.flat_text.lines().count());
        let (new_page, stripped_count, page_per_regex) = strip_page(
            page,
            &repeated,
            &bands,
            &regex_filters,
            &mut repeated_lines_seen,
        );

        for (regex, count) in page_per_regex {
            if total_lines_on_page > 0 {
                let frac = count as f32 / total_lines_on_page as f32;
                if frac > REGEX_OVER_MATCH_FRACTION {
                    warnings.push(IngestWarning::new(
                        "chrome_over_match",
                        format!(
                            "regex `{}` matched {} of {} lines on page {} (over {:.0}%)",
                            regex,
                            count,
                            total_lines_on_page,
                            new_page.page_number,
                            REGEX_OVER_MATCH_FRACTION * 100.0
                        ),
                        5,
                    ));
                }
            }
            *report.regex_match_counts.entry(regex).or_insert(0) += count;
        }

        per_page_stripped.push(stripped_count);
        report.total_lines_stripped += stripped_count;
        out_pages.push(new_page);
    }

    let mut repeated_lines: Vec<String> = repeated_lines_seen.into_iter().collect();
    repeated_lines.sort();

    let record = ChromeRecord {
        repeated_lines,
        per_page_stripped,
    };
    (out_pages, record, report)
}

// -------------------------------------------------------------------
// Repeated-line filter
// -------------------------------------------------------------------

fn detect_repeated_lines(pages: &[PageLayout], threshold: f32) -> HashSet<String> {
    let total_pages = pages.len();
    if total_pages == 0 {
        return HashSet::new();
    }
    let need = ((total_pages as f32 * threshold).ceil() as usize).max(2);

    let mut freq: HashMap<String, usize> = HashMap::new();
    for page in pages {
        let mut seen_on_page: HashSet<String> = HashSet::new();
        for line in page.flat_text.lines() {
            let norm = normalize_line(line);
            if norm.is_empty() {
                continue;
            }
            if seen_on_page.insert(norm.clone()) {
                *freq.entry(norm).or_insert(0) += 1;
            }
        }
    }
    freq.into_iter()
        .filter(|(_, n)| *n >= need)
        .map(|(k, _)| k)
        .collect()
}

fn normalize_line(line: &str) -> String {
    let trimmed = line.trim();
    let collapsed: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    // Strip "Page X of Y" / "X / Y" patterns so a page-numbered
    // banner still collapses to the same key across pages.
    let no_page_of = Regex::new(r"(?i)Page\s+\d+\s+of\s+\d+")
        .unwrap()
        .replace_all(&collapsed, "")
        .into_owned();
    let no_slash_num = Regex::new(r"\b\d+\s*/\s*\d+\b")
        .unwrap()
        .replace_all(&no_page_of, "")
        .into_owned();
    no_slash_num.trim().to_string()
}

fn is_page_number_chrome(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let page_re = Regex::new(r"(?i)^\s*Page\s+\d+\s+of\s+\d+\s*$").unwrap();
    let slash_re = Regex::new(r"^\s*\d+\s*/\s*\d+\s*$").unwrap();
    let bare_re = Regex::new(r"^\s*\d+\s*$").unwrap();
    page_re.is_match(trimmed) || slash_re.is_match(trimmed) || bare_re.is_match(trimmed)
}

// -------------------------------------------------------------------
// Positional Y-band detection
// -------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct PositionalBands {
    /// All chrome bands (`(low, high)`) in ascending Y order.
    all: Vec<[f32; 2]>,
    /// Top-most chrome band (highest Y in PDF user space). PDF
    /// coordinates put the page origin at the bottom-left so the
    /// running header has the largest Y.
    top: Option<[f32; 2]>,
    /// Bottom-most chrome band (smallest Y) — running footer / page
    /// number band.
    bottom: Option<[f32; 2]>,
}

impl PositionalBands {
    fn contains(&self, y: f32) -> bool {
        self.all
            .iter()
            .any(|band| y >= band[0] - f32::EPSILON && y <= band[1] + f32::EPSILON)
    }
}

fn detect_positional_bands(pages: &[PageLayout], page_coverage: f32) -> PositionalBands {
    let total_pages = pages.len();
    if total_pages == 0 {
        return PositionalBands::default();
    }
    let need = ((total_pages as f32 * page_coverage).ceil() as usize).max(2);

    // Bucket every span's top-edge Y into 10-pt buckets and track
    // which pages contributed. A page contributing 1+ spans to a
    // bucket counts once.
    let mut bucket_pages: HashMap<i32, HashSet<u32>> = HashMap::new();
    for page in pages {
        let mut seen_on_page: HashSet<i32> = HashSet::new();
        for span in &page.spans {
            let bucket = (span.bbox.y / POSITIONAL_BUCKET_PT).floor() as i32;
            if seen_on_page.insert(bucket) {
                bucket_pages
                    .entry(bucket)
                    .or_default()
                    .insert(page.page_number);
            }
        }
    }

    // Buckets present on >= page_coverage of pages are chrome.
    let mut chrome_buckets: Vec<i32> = bucket_pages
        .iter()
        .filter(|(_, pages_set)| pages_set.len() >= need)
        .map(|(b, _)| *b)
        .collect();
    chrome_buckets.sort();

    // Merge adjacent buckets into contiguous bands. Two buckets are
    // adjacent if their integer indices differ by one — together they
    // span a 20-pt-tall band etc.
    let mut bands: Vec<[f32; 2]> = Vec::new();
    for &b in &chrome_buckets {
        let low = b as f32 * POSITIONAL_BUCKET_PT;
        let high = low + POSITIONAL_BUCKET_PT;
        if let Some(last) = bands.last_mut()
            && (last[1] - low).abs() < f32::EPSILON
        {
            last[1] = high;
            continue;
        }
        bands.push([low, high]);
    }

    let bottom = bands.first().copied();
    let top = bands.last().copied();
    PositionalBands {
        all: bands,
        top,
        bottom,
    }
}

// -------------------------------------------------------------------
// Regex filter
// -------------------------------------------------------------------

struct CompiledChromeRegex {
    /// Original regex string (for reporting + telemetry).
    source: String,
    compiled: Regex,
    /// Optional `[low, high]` Y band the regex is constrained to.
    /// `None` means the regex matches regardless of position.
    y_band_pt: Option<[f32; 2]>,
}

fn compile_regex_filters(
    format: Option<&FormatJson>,
    warnings: &mut Vec<IngestWarning>,
) -> Vec<CompiledChromeRegex> {
    let Some(format) = format else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(format.chrome.len());
    for entry in &format.chrome {
        match Regex::new(&entry.regex) {
            Ok(re) => out.push(CompiledChromeRegex {
                source: entry.regex.clone(),
                compiled: re,
                y_band_pt: entry.y_band_pt,
            }),
            Err(err) => {
                warnings.push(IngestWarning::new(
                    "chrome_regex_invalid",
                    format!("regex `{}` failed to compile: {}", entry.regex, err),
                    5,
                ));
            }
        }
    }
    out
}

fn line_y_top(line_y: f32, band: [f32; 2]) -> bool {
    line_y >= band[0] - f32::EPSILON && line_y <= band[1] + f32::EPSILON
}

// -------------------------------------------------------------------
// Per-page application
// -------------------------------------------------------------------

fn strip_page(
    page: PageLayout,
    repeated: &HashSet<String>,
    bands: &PositionalBands,
    regex_filters: &[CompiledChromeRegex],
    repeated_lines_seen: &mut HashSet<String>,
) -> (PageLayout, u32, Vec<(String, u32)>) {
    let PageLayout {
        page_number,
        spans,
        lines,
        tables,
        path_count,
        image_count,
        flat_text,
    } = page;

    let mut per_regex_counts: HashMap<String, u32> = HashMap::new();
    for entry in regex_filters {
        per_regex_counts.insert(entry.source.clone(), 0);
    }

    let mut stripped_count: u32 = 0;
    let had_structured_lines = !lines.is_empty();

    // ---- Structured spans (positional only — text-similarity is
    // applied at the line granularity below since spans don't carry
    // a normalised line key).
    let kept_spans: Vec<_> = spans
        .into_iter()
        .filter(|s| !bands.contains(s.bbox.y))
        .collect();

    // ---- Structured lines (PDF path) -------------------------------
    //
    // Filter precedence: format-supplied regexes run FIRST so their
    // match counts are accurate telemetry; the descriptor expresses
    // intent ("these patterns are chrome") and we want the report to
    // reflect that even if a legacy filter would have caught the line
    // too. Positional band + repeated-line + page-number patterns
    // back-stop everything else.
    let mut kept_lines = Vec::with_capacity(lines.len());
    for line in lines {
        let norm = normalize_line(&line.text);
        let mut is_chrome = false;

        // Regex filter (format-driven; runs first for accurate counts).
        for entry in regex_filters {
            if entry.compiled.is_match(&line.text) {
                if let Some(band) = entry.y_band_pt
                    && !line_y_top(line.bbox.y, band)
                {
                    continue;
                }
                *per_regex_counts.entry(entry.source.clone()).or_insert(0) += 1;
                is_chrome = true;
                break;
            }
        }
        // Positional band.
        if !is_chrome && bands.contains(line.bbox.y) {
            is_chrome = true;
        }
        // Repeated-line.
        if !is_chrome && !norm.is_empty() && repeated.contains(&norm) {
            is_chrome = true;
            repeated_lines_seen.insert(norm.clone());
        } else if is_chrome && !norm.is_empty() && repeated.contains(&norm) {
            // Still record so the legacy `ChromeRecord.repeated_lines`
            // surfaces this banner for the manifest, even though a
            // higher-priority filter already classified it.
            repeated_lines_seen.insert(norm);
        }
        // Bare page-number patterns (legacy behaviour).
        if !is_chrome && is_page_number_chrome(&line.text) {
            is_chrome = true;
        }

        if is_chrome {
            stripped_count += 1;
        } else {
            kept_lines.push(line);
        }
    }

    // If the page carried structured lines (PDF path) we rebuild
    // `flat_text` from the survivors. Otherwise (text-only fixtures
    // or pages with empty `lines`) we filter the raw flat_text in
    // place; positional bands cannot fire there, but repeated-line +
    // page-number + position-unconstrained regexes still apply.
    let new_flat_text = if had_structured_lines {
        let mut s = String::with_capacity(flat_text.len());
        for line in &kept_lines {
            s.push_str(&line.text);
            s.push('\n');
        }
        s
    } else {
        filter_flat_text(
            &flat_text,
            repeated,
            regex_filters,
            &mut per_regex_counts,
            repeated_lines_seen,
            &mut stripped_count,
        )
    };

    let page_per_regex: Vec<(String, u32)> = per_regex_counts.into_iter().collect();

    (
        PageLayout {
            page_number,
            spans: kept_spans,
            lines: kept_lines,
            tables,
            path_count,
            image_count,
            flat_text: new_flat_text,
        },
        stripped_count,
        page_per_regex,
    )
}

/// Filter `flat_text` line-by-line when the page carried no
/// structured `lines` records (text-only fixtures or pages where
/// `pdf_oxide` clustered nothing). Positional bands cannot fire here
/// — there are no bboxes — but repeated-line + page-number + regex
/// (band-unconstrained) filters still apply.
fn filter_flat_text(
    flat_text: &str,
    repeated: &HashSet<String>,
    regex_filters: &[CompiledChromeRegex],
    per_regex_counts: &mut HashMap<String, u32>,
    repeated_lines_seen: &mut HashSet<String>,
    stripped_count: &mut u32,
) -> String {
    let mut new_text = String::with_capacity(flat_text.len());
    for line in flat_text.lines() {
        let norm = normalize_line(line);
        let mut is_chrome = false;
        // Regex filter (format-driven; runs first for accurate counts).
        for entry in regex_filters {
            if entry.y_band_pt.is_some() {
                // Band-constrained regex needs a bbox; skip on
                // text-only paths.
                continue;
            }
            if entry.compiled.is_match(line) {
                *per_regex_counts.entry(entry.source.clone()).or_insert(0) += 1;
                is_chrome = true;
                break;
            }
        }
        if !is_chrome && !norm.is_empty() && repeated.contains(&norm) {
            is_chrome = true;
            repeated_lines_seen.insert(norm);
        }
        if !is_chrome && is_page_number_chrome(line) {
            is_chrome = true;
        }
        if is_chrome {
            *stripped_count += 1;
            continue;
        }
        new_text.push_str(line);
        new_text.push('\n');
    }
    new_text
}

#[cfg(test)]
mod tests {
    use super::super::super::format::descriptor::{ChromeEntry, ChromeKind};
    use super::super::loading::{BBox, FontWeight, PageLine, PageSpan};
    use super::*;
    use chrono::Utc;

    fn page(n: u32, text: &str) -> PageLayout {
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

    /// Build a structured PDF-style page with a header span / line at
    /// `header_y` plus a body line at y=400. `flat_text` is derived
    /// from the lines in order.
    fn structured_page(n: u32, header_y: f32, header: &str, body: &str) -> PageLayout {
        let header_line = PageLine {
            text: header.into(),
            bbox: BBox {
                x: 50.0,
                y: header_y,
                w: 200.0,
                h: 12.0,
            },
            dominant_font_size: 10.0,
            is_bold: true,
        };
        let body_line = PageLine {
            text: body.into(),
            bbox: BBox {
                x: 50.0,
                y: 400.0,
                w: 400.0,
                h: 12.0,
            },
            dominant_font_size: 11.0,
            is_bold: false,
        };
        let header_span = PageSpan {
            text: header.into(),
            bbox: BBox {
                x: 50.0,
                y: header_y,
                w: 200.0,
                h: 12.0,
            },
            font_size: 10.0,
            font_weight: FontWeight::Normal,
            is_italic: false,
        };
        let body_span = PageSpan {
            text: body.into(),
            bbox: BBox {
                x: 50.0,
                y: 400.0,
                w: 400.0,
                h: 12.0,
            },
            font_size: 11.0,
            font_weight: FontWeight::Normal,
            is_italic: false,
        };
        let flat_text = format!("{header}\n{body}\n");
        PageLayout {
            page_number: n,
            spans: vec![header_span, body_span],
            lines: vec![header_line, body_line],
            tables: Vec::new(),
            path_count: 0,
            image_count: 0,
            flat_text,
        }
    }

    fn empty_format(chrome: Vec<ChromeEntry>) -> FormatJson {
        FormatJson {
            schema_version: 1,
            model: "test".into(),
            prompt_version: "test".into(),
            source_sha256: "test".into(),
            discovered_at: Utc::now(),
            section_roles: Vec::new(),
            tables: Vec::new(),
            figures: Vec::new(),
            glossary: Vec::new(),
            chrome,
            validation: Default::default(),
        }
    }

    fn loaded_pdf(pages: Vec<PageLayout>) -> LoadedSource {
        LoadedSource {
            kind: SourceKind::Pdf,
            pages,
            pdf: None,
        }
    }

    #[test]
    fn strips_repeated_banner_across_pages() {
        let pages = vec![
            page(1, "Numenta SoC\nIntroduction\nBody page 1\nPage 1 of 3\n"),
            page(2, "Numenta SoC\nArchitecture\nBody page 2\nPage 2 of 3\n"),
            page(3, "Numenta SoC\nReferences\nBody page 3\nPage 3 of 3\n"),
        ];
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (out, rec, _report) = strip_chrome_pages(pages, &config, None, &mut warnings);
        for p in &out {
            assert!(
                !p.flat_text.contains("Numenta SoC"),
                "page text: {}",
                p.flat_text
            );
            assert!(!p.flat_text.contains("Page "), "page text: {}", p.flat_text);
        }
        assert!(rec.repeated_lines.iter().any(|l| l == "Numenta SoC"));
        // Every page should have at least 2 lines stripped (banner +
        // page number).
        for n in &rec.per_page_stripped {
            assert!(*n >= 2, "per-page count: {n}");
        }
    }

    #[test]
    fn strips_bare_page_numbers() {
        let pages = vec![
            page(1, "Body 1\n1\n"),
            page(2, "Body 2\n2\n"),
            page(3, "Body 3\n3\n"),
        ];
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (out, _rec, _report) = strip_chrome_pages(pages, &config, None, &mut warnings);
        for p in &out {
            assert!(
                p.flat_text
                    .lines()
                    .all(|l| !l.trim().chars().all(|c| c.is_ascii_digit()) || l.trim().is_empty()),
                "leftover page-number line: {}",
                p.flat_text
            );
        }
    }

    #[test]
    fn markdown_passes_through_untouched() {
        let loaded = LoadedSource {
            kind: SourceKind::Markdown,
            pages: vec![page(1, "# Title\n\nbody\n")],
            pdf: None,
        };
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (out, rec) = strip_chrome(loaded, &config, &mut warnings);
        assert_eq!(out.pages[0].flat_text, "# Title\n\nbody\n");
        assert!(rec.repeated_lines.is_empty());
    }

    /// 5 structured PDF-style pages with a "RV12 RISC-V" line at the
    /// top (y=770). The positional Y-banding filter detects the band
    /// and strips the header span + line from every page.
    #[test]
    fn positional_band_strips_top_header() {
        let pages: Vec<PageLayout> = (1..=5)
            .map(|i| {
                structured_page(
                    i,
                    770.0,
                    "RV12 RISC-V",
                    &format!("Unique body content for page {i}"),
                )
            })
            .collect();
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (loaded, _rec, report) =
            strip_chrome_with_format(loaded_pdf(pages), &config, None, &mut warnings);
        for page in &loaded.pages {
            assert!(
                !page.flat_text.contains("RV12 RISC-V"),
                "page {}: {}",
                page.page_number,
                page.flat_text
            );
            assert!(
                page.spans.iter().all(|s| !s.text.contains("RV12 RISC-V")),
                "page {} retained chrome span",
                page.page_number
            );
            assert!(
                page.lines.iter().all(|l| !l.text.contains("RV12 RISC-V")),
                "page {} retained chrome line",
                page.page_number
            );
        }
        assert!(
            report.positional_band_top.is_some(),
            "expected positional top band to be detected"
        );
    }

    /// Same as above but the header Y jitters within a 10-pt bucket
    /// (770, 771, 769, 770, 770). The bucketing must still cluster
    /// every header into the same band.
    #[test]
    fn positional_band_tolerates_y_jitter() {
        let ys = [770.0_f32, 771.0, 769.0, 770.0, 770.0];
        let pages: Vec<PageLayout> = ys
            .iter()
            .enumerate()
            .map(|(i, y)| {
                structured_page(
                    (i + 1) as u32,
                    *y,
                    "RV12 RISC-V",
                    &format!("Body {}", i + 1),
                )
            })
            .collect();
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (loaded, _rec, report) =
            strip_chrome_with_format(loaded_pdf(pages), &config, None, &mut warnings);
        for page in &loaded.pages {
            assert!(
                !page.flat_text.contains("RV12 RISC-V"),
                "page {}: {}",
                page.page_number,
                page.flat_text
            );
        }
        assert!(report.positional_band_top.is_some());
    }

    /// Regex filter with a `y_band_pt` constraint. We inject a
    /// "Page N of 95" line at y=45 on each of 5 pages plus the
    /// running header at y=770, then point the descriptor at
    /// `^Page \d+ of \d+$` constrained to `[40, 50]`. Both filters
    /// should fire.
    #[test]
    fn regex_filter_strips_band_constrained_match() {
        let mut pages = Vec::new();
        for i in 1..=5 {
            let mut page = structured_page(i, 770.0, "RV12 RISC-V", &format!("Body {i}"));
            let footer_text = format!("Page {i} of 95");
            let footer_line = PageLine {
                text: footer_text.clone(),
                bbox: BBox {
                    x: 50.0,
                    y: 45.0,
                    w: 100.0,
                    h: 10.0,
                },
                dominant_font_size: 9.0,
                is_bold: false,
            };
            let footer_span = PageSpan {
                text: footer_text.clone(),
                bbox: BBox {
                    x: 50.0,
                    y: 45.0,
                    w: 100.0,
                    h: 10.0,
                },
                font_size: 9.0,
                font_weight: FontWeight::Normal,
                is_italic: false,
            };
            page.spans.push(footer_span);
            page.lines.push(footer_line);
            // Append footer to flat_text so the legacy text filter
            // also sees it.
            page.flat_text.push_str(&footer_text);
            page.flat_text.push('\n');
            pages.push(page);
        }
        let format = empty_format(vec![ChromeEntry {
            regex: r"^Page \d+ of \d+$".into(),
            kind: ChromeKind::PageNumber,
            y_band_pt: Some([40.0, 50.0]),
            match_count: 0,
        }]);
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (loaded, _rec, report) =
            strip_chrome_with_format(loaded_pdf(pages), &config, Some(&format), &mut warnings);
        for page in &loaded.pages {
            assert!(
                !page.flat_text.contains("Page "),
                "page {}: {}",
                page.page_number,
                page.flat_text
            );
        }
        let count = report
            .regex_match_counts
            .get(r"^Page \d+ of \d+$")
            .copied()
            .unwrap_or(0);
        assert_eq!(count, 5, "expected 5 regex matches across 5 pages");
    }

    /// An over-match regex (`^.*$`) matches every line on the page;
    /// emit the `chrome_over_match` warning.
    #[test]
    fn over_match_regex_emits_warning() {
        // 5 lines per page so over-match > 80% is well-defined.
        let lines = (0..5)
            .map(|i| PageLine {
                text: format!("body line {i}"),
                bbox: BBox {
                    x: 50.0,
                    y: 400.0 - (i as f32) * 12.0,
                    w: 400.0,
                    h: 10.0,
                },
                dominant_font_size: 11.0,
                is_bold: false,
            })
            .collect::<Vec<_>>();
        let flat = lines
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let page = PageLayout {
            page_number: 1,
            spans: Vec::new(),
            lines,
            tables: Vec::new(),
            path_count: 0,
            image_count: 0,
            flat_text: flat,
        };
        let format = empty_format(vec![ChromeEntry {
            regex: r"^.*$".into(),
            kind: ChromeKind::Watermark,
            y_band_pt: None,
            match_count: 0,
        }]);
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (_loaded, _rec, _report) = strip_chrome_with_format(
            loaded_pdf(vec![page]),
            &config,
            Some(&format),
            &mut warnings,
        );
        assert!(
            warnings.iter().any(|w| w.code == "chrome_over_match"),
            "expected chrome_over_match warning; got {:?}",
            warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
        );
    }

    /// A regex that fails to compile surfaces `chrome_regex_invalid`
    /// and is skipped. Other filters still run.
    #[test]
    fn invalid_regex_emits_warning_and_is_skipped() {
        let pages = vec![structured_page(1, 770.0, "Header", "Body")];
        let format = empty_format(vec![ChromeEntry {
            regex: "[invalid".into(),
            kind: ChromeKind::RunningHeader,
            y_band_pt: None,
            match_count: 0,
        }]);
        let config = IngestConfig::default();
        let mut warnings = Vec::new();
        let (_loaded, _rec, report) =
            strip_chrome_with_format(loaded_pdf(pages), &config, Some(&format), &mut warnings);
        assert!(
            warnings.iter().any(|w| w.code == "chrome_regex_invalid"),
            "expected chrome_regex_invalid warning; got {:?}",
            warnings.iter().map(|w| &w.code).collect::<Vec<_>>()
        );
        // The invalid regex never registered, so its count is absent.
        assert!(!report.regex_match_counts.contains_key("[invalid"));
    }
}
