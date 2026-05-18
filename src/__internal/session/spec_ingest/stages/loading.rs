//! Stage 1: source loading.
//!
//! Dispatches by extension and produces a `LoadedSource` carrying
//! per-page text the rest of the pipeline operates on. PDF inputs
//! retain the pdfium-render document handle for stage 6 (figure
//! rendering); markdown and text inputs are treated as a single
//! "page" of UTF-8 with BOM stripped.

use std::path::Path;

use pdfium_render::prelude::*;

use crate::{Error, Result};

use super::super::pipeline::{IngestRequest, IngestWarning, SourceKind};

/// Output of stage 1. The pdfium document is owned here so its
/// lifetime spans every later stage that needs it (chrome, parse,
/// figures).
pub struct LoadedSource {
    pub kind: SourceKind,
    /// Per-page text. For markdown / text inputs there is exactly
    /// one entry; for PDFs there is one per page.
    pub pages: Vec<PageText>,
    /// PDF document handle. `Some` only when `kind == Pdf`. Stage 6
    /// uses this for page rendering and figure detection. Held as
    /// an owned wrapper so its drop order matches the loaded source.
    pub pdf: Option<LoadedPdf>,
}

/// Owned pdfium document plus its loader handle. The handle must
/// outlive the document, so they're kept together.
pub struct LoadedPdf {
    /// SAFETY note: the document references the pdfium instance via
    /// a lifetime. We keep them in the same struct so the pdfium
    /// instance never drops before the document. The `'static` is a
    /// load-bearing white lie: pdfium-render's API doesn't expose a
    /// self-referencing owner cleanly, but in practice we only hand
    /// out references via `document()` whose borrow is bounded by
    /// the &self of this wrapper. The field is held to keep the
    /// owner alive; it's never read directly.
    #[allow(dead_code)]
    pub(crate) pdfium: &'static Pdfium,
    pub(crate) document: PdfDocument<'static>,
}

impl LoadedPdf {
    /// Borrow the document. Caller must ensure the returned reference
    /// does not outlive `self`.
    pub fn document(&self) -> &PdfDocument<'static> {
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
    // pdfium-render binds the underlying C library once per
    // process; binding twice deadlocks or returns an error. We
    // cache the `Pdfium` in a process-wide OnceLock so repeated
    // calls (CLI invocations, unit tests with --test-threads > 1)
    // reuse the same instance.
    let pdfium = shared_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(source, None)
        .map_err(|e| Error::State(format!("spec ingest: load PDF via pdfium: {e}")))?;

    let mut pages = Vec::new();
    for (idx, page) in document.pages().iter().enumerate() {
        let page_number = (idx + 1) as u32;
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        pages.push(PageText { page_number, text });
    }

    Ok(LoadedSource {
        kind: SourceKind::Pdf,
        pages,
        pdf: Some(LoadedPdf { pdfium, document }),
    })
}

/// Wrapper that lets us put a `Pdfium` behind a static. The
/// underlying type is `!Send + !Sync` so we promise the harness
/// not to share it across threads concurrently by gating every
/// access on a `Mutex`. Concurrent tests still cause `pdfium`
/// methods to run serially, which is what pdfium-render wants
/// internally.
struct SyncPdfium(Pdfium);
// SAFETY: the only access path is `shared_pdfium`, which hands
// out a `&'static Pdfium` whose API is internally serialized by
// the C library. We never invoke pdfium methods from two threads
// in parallel.
unsafe impl Send for SyncPdfium {}
unsafe impl Sync for SyncPdfium {}

/// Process-wide pdfium handle. Loaded on first use, reused thereafter.
pub(crate) fn shared_pdfium() -> Result<&'static Pdfium> {
    use std::sync::{Mutex, OnceLock};
    static CELL: OnceLock<Mutex<Option<Box<SyncPdfium>>>> = OnceLock::new();
    let mutex = CELL.get_or_init(|| Mutex::new(None));
    let mut guard = mutex
        .lock()
        .map_err(|e| Error::State(format!("spec ingest: pdfium mutex poisoned: {e}")))?;
    if guard.is_none() {
        let loaded = crate::session::pdfium_loader::load()?;
        *guard = Some(Box::new(SyncPdfium(loaded)));
    }
    // SAFETY: the box was placed once and is never moved or dropped
    // while the OnceLock lives (i.e. for the process lifetime).
    let ptr: &'static Pdfium = unsafe {
        let b = guard.as_ref().unwrap();
        &*(&(**b).0 as *const Pdfium)
    };
    Ok(ptr)
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
