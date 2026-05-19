//! Stage 2: page-chrome stripping.
//!
//! See architecture §1.4 stage 2. Removes lines that appear on at
//! least `appearance_threshold` fraction of pages (default 0.5)
//! plus `Page N of M` / `N / M` patterns. Markdown / text inputs
//! pass through untouched.

use super::super::pipeline::{IngestConfig, IngestWarning, SourceKind};
use super::loading::{LoadedSource, PageLayout};

#[derive(Debug, Clone, Default)]
pub struct ChromeRecord {
    /// Lines stripped because they appeared on enough pages.
    pub repeated_lines: Vec<String>,
    /// Per-page count of stripped lines (parallel to the input page
    /// vector). Useful for diagnostics; the manifest summarises it.
    pub per_page_stripped: Vec<u32>,
}

pub fn strip_chrome(
    loaded: LoadedSource,
    _config: &IngestConfig,
    _warnings: &mut Vec<IngestWarning>,
) -> (LoadedSource, ChromeRecord) {
    if !matches!(loaded.kind, SourceKind::Pdf) {
        return (loaded, ChromeRecord::default());
    }
    let LoadedSource { kind, pages, pdf } = loaded;
    let (stripped_pages, record) = strip_chrome_pages(pages, _config);
    (
        LoadedSource {
            kind,
            pages: stripped_pages,
            pdf,
        },
        record,
    )
}

fn strip_chrome_pages(
    pages: Vec<PageLayout>,
    config: &IngestConfig,
) -> (Vec<PageLayout>, ChromeRecord) {
    if !config.chrome_stripping.enabled || pages.is_empty() {
        return (pages, ChromeRecord::default());
    }

    let threshold = config.chrome_stripping.appearance_threshold.clamp(0.0, 1.0);
    let total_pages = pages.len();
    let need = ((total_pages as f32 * threshold).ceil() as usize).max(2);

    // Count normalized line frequencies across pages. Chrome
    // stripping still operates on `flat_text`; the structured
    // `spans` / `lines` fields are left untouched for now and will
    // be filtered properly in milestone 9.10's hybrid positional +
    // regex rewrite.
    let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in &pages {
        let mut seen_on_page: std::collections::HashSet<String> = std::collections::HashSet::new();
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

    let repeated: std::collections::HashSet<String> = freq
        .into_iter()
        .filter(|(_, n)| *n >= need)
        .map(|(k, _)| k)
        .collect();

    let mut repeated_lines: Vec<String> = repeated.iter().cloned().collect();
    repeated_lines.sort();

    let mut per_page_stripped = Vec::with_capacity(total_pages);
    let mut out_pages = Vec::with_capacity(total_pages);
    for page in pages {
        let mut count = 0u32;
        let mut new_text = String::with_capacity(page.flat_text.len());
        for line in page.flat_text.lines() {
            let norm = normalize_line(line);
            let is_page_num_pattern = is_page_number_chrome(line);
            if (!norm.is_empty() && repeated.contains(&norm)) || is_page_num_pattern {
                count += 1;
                continue;
            }
            new_text.push_str(line);
            new_text.push('\n');
        }
        per_page_stripped.push(count);
        out_pages.push(PageLayout {
            flat_text: new_text,
            ..page
        });
    }

    (
        out_pages,
        ChromeRecord {
            repeated_lines,
            per_page_stripped,
        },
    )
}

fn normalize_line(line: &str) -> String {
    let trimmed = line.trim();
    let collapsed: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    // Strip "Page X of Y" / "X / Y" patterns so a page-numbered
    // banner still collapses to the same key across pages.
    let no_page_of = regex::Regex::new(r"(?i)Page\s+\d+\s+of\s+\d+")
        .unwrap()
        .replace_all(&collapsed, "")
        .into_owned();
    let no_slash_num = regex::Regex::new(r"\b\d+\s*/\s*\d+\b")
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
    let page_re = regex::Regex::new(r"(?i)^\s*Page\s+\d+\s+of\s+\d+\s*$").unwrap();
    let slash_re = regex::Regex::new(r"^\s*\d+\s*/\s*\d+\s*$").unwrap();
    let bare_re = regex::Regex::new(r"^\s*\d+\s*$").unwrap();
    page_re.is_match(trimmed) || slash_re.is_match(trimmed) || bare_re.is_match(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn strips_repeated_banner_across_pages() {
        let pages = vec![
            page(1, "Numenta SoC\nIntroduction\nBody page 1\nPage 1 of 3\n"),
            page(2, "Numenta SoC\nArchitecture\nBody page 2\nPage 2 of 3\n"),
            page(3, "Numenta SoC\nReferences\nBody page 3\nPage 3 of 3\n"),
        ];
        let config = IngestConfig::default();
        let (out, rec) = strip_chrome_pages(pages, &config);
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
        let (out, _rec) = strip_chrome_pages(pages, &config);
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
}
