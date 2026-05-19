//! Stage 1: source loading.
//!
//! Dispatches by extension and produces a `LoadedSource` carrying
//! per-page structured layout the rest of the pipeline operates on.
//! PDF inputs retain the `pdf_oxide` document handle for stage 6
//! (figure rendering); markdown and text inputs are treated as a
//! single "page" of UTF-8 with BOM stripped.
//!
//! For PDFs each page is decomposed eagerly into spans, lines,
//! tables, plus path/image counts via `pdf_oxide`'s structured API
//! (`extract_spans`, `extract_text_lines`, `extract_tables`,
//! `extract_paths`, `extract_images`). A flat-text representation
//! (`PageLayout::flat_text`) is built by joining lines in reading
//! order so legacy consumers that still operate on a markdown-ish
//! string keep working; structured-spans consumers (chapters 7.8 /
//! milestones 9.3, 9.8, 9.9, 9.10) read the typed fields directly.

use std::path::Path;
use std::sync::Arc;

use pdf_oxide::PdfDocument;

use crate::{Error, Result};

use super::super::pipeline::{IngestRequest, IngestWarning, SourceKind};

/// Output of stage 1. The `pdf_oxide` document is owned here so its
/// lifetime spans every later stage that needs it (chrome, parse,
/// figures).
pub struct LoadedSource {
    pub kind: SourceKind,
    /// Per-page layout. For markdown / text inputs there is exactly
    /// one entry with structured fields empty and `flat_text`
    /// populated; for PDFs there is one entry per page with
    /// structured fields populated from `pdf_oxide`'s extractors
    /// and `flat_text` derived from the lines in reading order.
    pub pages: Vec<PageLayout>,
    /// PDF document handle. `Some` only when `kind == Pdf`. Stage 6
    /// uses this for page rendering. The Arc keeps the document
    /// alive across the pipeline without cloning the underlying
    /// PDF state.
    pub pdf: Option<LoadedPdf>,
}

/// Owned `pdf_oxide` document. Held as an `Arc` so it can be
/// borrowed from several pipeline stages without lifetime
/// gymnastics.
#[derive(Clone)]
pub struct LoadedPdf {
    pub(crate) document: Arc<PdfDocument>,
}

impl LoadedPdf {
    /// Borrow the document. Caller must ensure the returned
    /// reference does not outlive `self`.
    pub fn document(&self) -> &PdfDocument {
        &self.document
    }
}

/// Per-page layout record. `page_number` is 1-indexed for human
/// readability and matches PDF page numbering.
///
/// `spans`, `lines`, and `tables` carry the structured data needed
/// by downstream format-discovery stages (9.3 skeleton, 9.8 parse,
/// 9.9 classify). `path_count` / `image_count` provide a cheap
/// figure-presence hint without rerunning per-page extractors.
/// `flat_text` is the legacy-compatible flat string built by
/// joining `lines` in reading order; markdown / text inputs use it
/// as their primary content carrier.
#[derive(Debug, Clone)]
pub struct PageLayout {
    pub page_number: u32,
    /// Layout-aware spans: text + bbox + font size + weight +
    /// italic. Empty for non-PDF sources.
    pub spans: Vec<PageSpan>,
    /// Lines clustered from spans (one entry per
    /// `pdf_oxide::TextLine`). Empty for non-PDF sources.
    pub lines: Vec<PageLine>,
    /// Tables detected by `pdf_oxide`'s spatial projection. Empty
    /// for non-PDF sources.
    pub tables: Vec<PageTable>,
    /// Number of vector path objects on the page. Useful as a
    /// figure-presence hint without calling `extract_paths` again.
    /// Always 0 for non-PDF sources.
    pub path_count: u32,
    /// Number of embedded raster images on the page. Useful as a
    /// figure-presence hint without calling `extract_images`
    /// again. Always 0 for non-PDF sources.
    pub image_count: u32,
    /// Flat text dump. For PDFs this is the line texts joined with
    /// `\n` in reading order; for markdown / text inputs this is
    /// the raw UTF-8 body with any BOM stripped. Legacy consumers
    /// (chrome stripping, markdown-based parse, classify) read
    /// this; structured-data consumers ignore it.
    pub flat_text: String,
}

/// A single layout span as extracted by `pdf_oxide::extract_spans`.
/// Carries text + bbox + the minimum font metadata format
/// discovery needs (size, weight, italic) so 9.3 / 9.8 can cluster
/// spans into headings and body text.
#[derive(Debug, Clone)]
pub struct PageSpan {
    pub text: String,
    pub bbox: BBox,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub is_italic: bool,
}

/// One line of text, clustered from `pdf_oxide::extract_text_lines`.
/// `dominant_font_size` is the largest span size in the line and
/// `is_bold` indicates whether any constituent span is bold-weight;
/// these summaries are enough for heading detection without
/// re-walking the spans.
#[derive(Debug, Clone)]
pub struct PageLine {
    pub text: String,
    pub bbox: BBox,
    pub dominant_font_size: f32,
    pub is_bold: bool,
}

/// A table detected by `pdf_oxide::extract_tables`. Carries the
/// table's bbox, dimensions, header flag, and row cell texts so
/// consumers (9.9 classify) can locate the descriptor's matching
/// table by `(page, header_row)` without rerunning the spatial
/// detector.
#[derive(Debug, Clone)]
pub struct PageTable {
    pub bbox: BBox,
    pub row_count: u32,
    pub col_count: u32,
    pub has_header: bool,
    /// First row's cell texts. Carried so consumers can identify
    /// the table by its header signature without rerunning
    /// `pdf_oxide::extract_tables`.
    pub header_row: Vec<String>,
    /// All rows' cell texts in document order (one row = one
    /// `Vec<String>`). Cell newlines are preserved verbatim.
    pub rows: Vec<Vec<String>>,
}

/// Axis-aligned bounding box in PDF user-space points. Mirrors the
/// `(x, y, width, height)` shape of `pdf_oxide::geometry::Rect`
/// without leaking the foreign type to downstream stages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BBox {
    fn from_rect(rect: pdf_oxide::geometry::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.width,
            h: rect.height,
        }
    }
}

/// Simplified font-weight classification. `pdf_oxide` exposes the
/// full 100-900 PDF scale; downstream format discovery only cares
/// whether a span is bold-or-bolder for heading detection, so we
/// collapse the scale into Normal vs Bold here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontWeight {
    #[default]
    Normal,
    Bold,
}

impl FontWeight {
    fn from_pdf_oxide(weight: pdf_oxide::layout::FontWeight) -> Self {
        if weight.is_bold() {
            FontWeight::Bold
        } else {
            FontWeight::Normal
        }
    }
}

/// Stage 1 entry point. Loads the primary source if any; the empty-
/// corpus case (`primary` = None) returns a `LoadedSource` with
/// `kind = None`.
pub fn load_primary(
    request: &IngestRequest,
    warnings: &mut Vec<IngestWarning>,
) -> Result<LoadedSource> {
    let Some(primary) = request.primary.as_ref() else {
        return Ok(LoadedSource {
            kind: SourceKind::None,
            pages: Vec::new(),
            pdf: None,
        });
    };
    load(&primary.path, warnings)
}

/// Dispatch by extension and load.
pub fn load(source: &Path, warnings: &mut Vec<IngestWarning>) -> Result<LoadedSource> {
    let kind = SourceKind::from_path(source).ok_or_else(|| {
        Error::State(format!(
            "spec ingest: unsupported source extension at {}; expected .pdf / .md / .markdown / .txt / .text",
            source.display()
        ))
    })?;
    match kind {
        SourceKind::Pdf => load_pdf(source),
        SourceKind::Markdown => load_text_like(source, SourceKind::Markdown),
        SourceKind::Text => load_text_like(source, SourceKind::Text),
        SourceKind::None => {
            warnings.push(IngestWarning::new(
                "no_source",
                "load() called with SourceKind::None; ignored",
                1,
            ));
            Ok(LoadedSource {
                kind: SourceKind::None,
                pages: Vec::new(),
                pdf: None,
            })
        }
    }
}

fn load_pdf(source: &Path) -> Result<LoadedSource> {
    let path_str = source
        .to_str()
        .ok_or_else(|| Error::State(format!("spec ingest: non-UTF-8 PDF path: {source:?}")))?;
    let document = PdfDocument::open(path_str)
        .map_err(|e| Error::State(format!("spec ingest: open PDF via pdf_oxide: {e}")))?;
    let page_count = document
        .page_count()
        .map_err(|e| Error::State(format!("spec ingest: read PDF page count: {e}")))?;

    let mut pages: Vec<PageLayout> = Vec::with_capacity(page_count);
    for idx in 0..page_count {
        pages.push(build_page_layout(&document, idx)?);
    }

    Ok(LoadedSource {
        kind: SourceKind::Pdf,
        pages,
        pdf: Some(LoadedPdf {
            document: Arc::new(document),
        }),
    })
}

fn build_page_layout(document: &PdfDocument, idx: usize) -> Result<PageLayout> {
    let page_number = (idx + 1) as u32;

    // `extract_spans` is the canonical entry point for layout-aware
    // text; `extract_text_lines` reuses the same span set under the
    // hood to cluster into reading-order lines.
    let spans_raw = document
        .extract_spans(idx)
        .map_err(|e| Error::State(format!("spec ingest: extract spans page {idx}: {e}")))?;
    let lines_raw = document
        .extract_text_lines(idx)
        .map_err(|e| Error::State(format!("spec ingest: extract lines page {idx}: {e}")))?;
    // Tables can be expensive on dense pages; we cache them so 9.9
    // classify can match descriptors without rerunning the spatial
    // detector. Failures here are surfaced as hard errors — a page
    // we can't structurally inspect produces a broken downstream
    // chain we'd rather catch loudly.
    let tables_raw = document
        .extract_tables(idx)
        .map_err(|e| Error::State(format!("spec ingest: extract tables page {idx}: {e}")))?;
    let paths_raw = document
        .extract_paths(idx)
        .map_err(|e| Error::State(format!("spec ingest: extract paths page {idx}: {e}")))?;
    let images_raw = document
        .extract_images(idx)
        .map_err(|e| Error::State(format!("spec ingest: extract images page {idx}: {e}")))?;

    let spans: Vec<PageSpan> = spans_raw
        .into_iter()
        .map(|s| PageSpan {
            text: s.text,
            bbox: BBox::from_rect(s.bbox),
            font_size: s.font_size,
            font_weight: FontWeight::from_pdf_oxide(s.font_weight),
            is_italic: s.is_italic,
        })
        .collect();

    let lines: Vec<PageLine> = lines_raw
        .into_iter()
        .map(|l| {
            let mut dominant: f32 = 0.0;
            let mut is_bold = false;
            for w in &l.words {
                if w.avg_font_size > dominant {
                    dominant = w.avg_font_size;
                }
                if w.is_bold {
                    is_bold = true;
                }
            }
            PageLine {
                text: l.text,
                bbox: BBox::from_rect(l.bbox),
                dominant_font_size: dominant,
                is_bold,
            }
        })
        .collect();

    let tables: Vec<PageTable> = tables_raw
        .into_iter()
        .map(|t| {
            let col_count = t.col_count as u32;
            let has_header = t.has_header;
            let bbox = t.bbox.map(BBox::from_rect).unwrap_or(BBox {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            });
            let rows: Vec<Vec<String>> = t
                .rows
                .iter()
                .map(|r| r.cells.iter().map(|c| c.text.clone()).collect())
                .collect();
            let header_row = rows.first().cloned().unwrap_or_default();
            let row_count = rows.len() as u32;
            PageTable {
                bbox,
                row_count,
                col_count,
                has_header,
                header_row,
                rows,
            }
        })
        .collect();

    // Reading-order flat text: join line texts with `\n`. Trailing
    // newline keeps the legacy line-oriented consumers (chrome
    // stripping) happy.
    let mut flat_text = String::new();
    for line in &lines {
        flat_text.push_str(&line.text);
        flat_text.push('\n');
    }

    Ok(PageLayout {
        page_number,
        spans,
        lines,
        tables,
        path_count: paths_raw.len() as u32,
        image_count: images_raw.len() as u32,
        flat_text,
    })
}

fn load_text_like(source: &Path, kind: SourceKind) -> Result<LoadedSource> {
    let bytes = std::fs::read(source).map_err(|e| {
        Error::State(format!(
            "spec ingest: read source {}: {e}",
            source.display()
        ))
    })?;
    let text = strip_bom(&bytes);
    Ok(LoadedSource {
        kind,
        pages: vec![PageLayout {
            page_number: 1,
            spans: Vec::new(),
            lines: Vec::new(),
            tables: Vec::new(),
            path_count: 0,
            image_count: 0,
            flat_text: text,
        }],
        pdf: None,
    })
}

fn strip_bom(bytes: &[u8]) -> String {
    const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    let slice = if bytes.starts_with(BOM) {
        &bytes[BOM.len()..]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_markdown_strips_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("spec.md");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
        f.write_all(b"# Title\n\nbody\n").unwrap();
        drop(f);
        let mut warnings = Vec::new();
        let loaded = load(&path, &mut warnings).unwrap();
        assert_eq!(loaded.kind, SourceKind::Markdown);
        assert_eq!(loaded.pages.len(), 1);
        assert!(loaded.pages[0].flat_text.starts_with("# Title"));
        assert!(loaded.pages[0].spans.is_empty());
        assert!(loaded.pages[0].lines.is_empty());
        assert!(loaded.pages[0].tables.is_empty());
        assert_eq!(loaded.pages[0].path_count, 0);
        assert_eq!(loaded.pages[0].image_count, 0);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_plain_text_marks_kind_text() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();
        let mut warnings = Vec::new();
        let loaded = load(&path, &mut warnings).unwrap();
        assert_eq!(loaded.kind, SourceKind::Text);
        assert_eq!(loaded.pages.len(), 1);
        assert_eq!(loaded.pages[0].flat_text, "hello\nworld\n");
        assert!(loaded.pages[0].spans.is_empty());
    }

    #[test]
    fn load_unknown_extension_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("blob.xyz");
        std::fs::write(&path, "stuff").unwrap();
        let mut warnings = Vec::new();
        match load(&path, &mut warnings) {
            Err(err) => {
                let msg = format!("{err}");
                assert!(msg.contains("unsupported source extension"), "{msg}");
            }
            Ok(_) => panic!("expected unsupported-extension error"),
        }
    }

    #[test]
    fn load_invalid_pdf_errors_hard() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.pdf");
        std::fs::write(&path, b"definitely not a pdf").unwrap();
        let mut warnings = Vec::new();
        match load(&path, &mut warnings) {
            Err(err) => {
                let msg = format!("{err}").to_lowercase();
                assert!(msg.contains("pdf"), "{msg}");
            }
            Ok(_) => panic!("expected invalid-PDF error"),
        }
    }

    /// Build a minimal single-page PDF with one text op via lopdf
    /// and check that `load_pdf` populates the structured fields:
    /// at least one span carrying "Hello", a non-empty `flat_text`,
    /// and zero tables / paths / images.
    #[test]
    fn load_pdf_populates_spans_and_flat_text() {
        use lopdf::content::{Content, Operation};
        use lopdf::dictionary;
        use lopdf::{Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! {
                "F1" => font_id,
            },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![100.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal("Hello, sim-flow!")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let tmp = tempfile::tempdir().unwrap();
        let pdf_path = tmp.path().join("hello.pdf");
        doc.save(&pdf_path).unwrap();

        let mut warnings = Vec::new();
        let loaded = load(&pdf_path, &mut warnings).unwrap();
        assert_eq!(loaded.kind, SourceKind::Pdf);
        assert_eq!(loaded.pages.len(), 1);
        let page = &loaded.pages[0];
        assert_eq!(page.page_number, 1);
        assert!(!page.spans.is_empty(), "expected at least one span");
        let span_text_joined: String = page.spans.iter().map(|s| s.text.as_str()).collect();
        assert!(
            span_text_joined.contains("Hello"),
            "spans did not contain 'Hello': {span_text_joined:?}"
        );
        assert!(
            page.flat_text.contains("Hello"),
            "flat_text missing 'Hello': {:?}",
            page.flat_text
        );
        assert!(
            page.tables.is_empty(),
            "expected no tables on a plain-text fixture"
        );
        assert_eq!(page.path_count, 0);
        assert_eq!(page.image_count, 0);
        assert!(loaded.pdf.is_some());
    }
}
