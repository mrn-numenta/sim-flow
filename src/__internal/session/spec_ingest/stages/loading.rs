//! Stage 1: source loading.
//!
//! Dispatches by extension and produces a `LoadedSource` carrying
//! per-page text the rest of the pipeline operates on. PDF inputs
//! retain the `pdf_oxide` document handle for stage 6 (figure
//! rendering); markdown and text inputs are treated as a single
//! "page" of UTF-8 with BOM stripped.

use std::path::Path;
use std::sync::Arc;

use pdf_oxide::PdfDocument;
use pdf_oxide::converters::{ConversionOptions, ReadingOrderMode};

use crate::{Error, Result};

use super::super::pipeline::{IngestRequest, IngestWarning, SourceKind};

/// Output of stage 1. The `pdf_oxide` document is owned here so its
/// lifetime spans every later stage that needs it (chrome, parse,
/// figures).
pub struct LoadedSource {
    pub kind: SourceKind,
    /// Per-page text. For markdown / text inputs there is exactly
    /// one entry; for PDFs there is one per page. The text content
    /// is layout-aware markdown for PDFs (produced by
    /// `pdf_oxide::PdfDocument::to_markdown`) — headings, bold,
    /// italic, and simple pipe tables are preserved.
    pub pages: Vec<PageText>,
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

/// Per-page text record. `page_number` is 1-indexed for human
/// readability and matches PDF page numbering.
#[derive(Debug, Clone)]
pub struct PageText {
    pub page_number: u32,
    pub text: String,
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

    // Layout-aware markdown per page. `TopToBottomLeftToRight`
    // produces deterministic, row-major output that downstream
    // stages can chunk by heading. Heading clustering by font size
    // is the default and what the chunker depends on; table
    // extraction is opt-in and not on for the chunker (multi-line
    // cells make it unreliable on real specs).
    let opts = ConversionOptions {
        reading_order_mode: ReadingOrderMode::TopToBottomLeftToRight,
        ..ConversionOptions::default()
    };
    let mut pages = Vec::with_capacity(page_count);
    for idx in 0..page_count {
        let md = document
            .to_markdown(idx, &opts)
            .map_err(|e| Error::State(format!("spec ingest: extract page {idx} markdown: {e}")))?;
        pages.push(PageText {
            page_number: (idx + 1) as u32,
            text: md,
        });
    }

    Ok(LoadedSource {
        kind: SourceKind::Pdf,
        pages,
        pdf: Some(LoadedPdf {
            document: Arc::new(document),
        }),
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
        pages: vec![PageText {
            page_number: 1,
            text,
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
        assert!(loaded.pages[0].text.starts_with("# Title"));
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
        assert_eq!(loaded.pages[0].text, "hello\nworld\n");
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
}
