//! Stage 6: figure detection and page-region rendering.

use pdf_oxide::PdfDocument;
use pdf_oxide::rendering::{RenderOptions, render_page};

use crate::{Error, Result};

use super::super::pipeline::{IngestConfig, IngestWarning};
use super::loading::LoadedSource;

#[derive(Debug, Clone)]
pub struct FigureOutput {
    pub page_number: u32,
    /// Relative path under `primary/`, e.g. `figures/page-013.png`.
    pub rel_png_path: String,
    /// Relative path of the caption stub. Same dir as the PNG.
    pub rel_caption_path: String,
    /// PNG bytes (held in memory so the emit stage writes them
    /// atomically alongside everything else).
    pub png_bytes: Vec<u8>,
    /// Caption stub body to be written verbatim.
    pub caption_body: String,
}

pub fn extract_figures(
    loaded: &LoadedSource,
    config: &IngestConfig,
    warnings: &mut Vec<IngestWarning>,
) -> Result<Vec<FigureOutput>> {
    let Some(pdf) = loaded.pdf.as_ref() else {
        return Ok(Vec::new());
    };
    let doc = pdf.document();
    let pages_with_figures = detect_figure_pages(doc, config)?;

    let mut out = Vec::new();
    for page_num in pages_with_figures {
        match render_figure_page(doc, page_num, config.figures.dpi) {
            Ok(bytes) => {
                let rel_png = format!("figures/page-{page_num:03}.png");
                let rel_cap = format!("figures/page-{page_num:03}.caption.md");
                out.push(FigureOutput {
                    page_number: page_num,
                    rel_png_path: rel_png.clone(),
                    rel_caption_path: rel_cap,
                    png_bytes: bytes,
                    caption_body: caption_stub(page_num, &rel_png),
                });
            }
            Err(err) => {
                warnings.push(IngestWarning::new(
                    "figure_render_failed",
                    format!("page {page_num}: {err}"),
                    6,
                ));
            }
        }
    }
    Ok(out)
}

/// Detect pages that contain figure content per the architecture
/// heuristic: at least one embedded image OR vector-path count above
/// the configured threshold.
pub fn detect_figure_pages(doc: &PdfDocument, config: &IngestConfig) -> Result<Vec<u32>> {
    let page_count = doc
        .page_count()
        .map_err(|e| Error::State(format!("spec ingest: page_count: {e}")))?;
    let mut pages = Vec::new();
    for idx in 0..page_count {
        let images = doc.extract_images(idx).map(|v| v.len() as u32).unwrap_or(0);
        let paths = doc.extract_paths(idx).map(|v| v.len() as u32).unwrap_or(0);
        if images >= 1 || paths >= config.figures.vector_op_threshold {
            pages.push((idx + 1) as u32);
        }
    }
    Ok(pages)
}

/// Render a single PDF page to PNG bytes.
pub fn render_figure_page(doc: &PdfDocument, page: u32, dpi: u32) -> Result<Vec<u8>> {
    let total = doc
        .page_count()
        .map_err(|e| Error::State(format!("spec ingest: page_count: {e}")))?;
    if page < 1 || (page as usize) > total {
        return Err(Error::State(format!(
            "spec ingest: figure render: page {page} out of range (1..={total})"
        )));
    }
    // `RenderOptions` has a private field so we cannot use the
    // struct-update shorthand; assigning `dpi` after default is the
    // accepted pattern from the upstream docs.
    #[allow(clippy::field_reassign_with_default)]
    let opts = {
        let mut o = RenderOptions::default();
        o.dpi = dpi;
        o
    };
    let img = render_page(doc, (page - 1) as usize, &opts)
        .map_err(|e| Error::State(format!("spec ingest: pdf_oxide render: {e}")))?;
    // `RenderedImage::data` is already PNG bytes when
    // `RenderOptions::format == ImageFormat::Png` (the default).
    Ok(img.data)
}

fn caption_stub(page: u32, rel_png: &str) -> String {
    format!(
        "---\nfigure_id: \"page-{page:03}\"\nsource_page: {page}\nsource_chunk_id: \"\"\nrole: \"\"\nreferenced_elements: []\n---\n\n<!-- caption stub for {rel_png}; populate by hand or via captioning hook -->\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_stub_includes_page_and_path() {
        let s = caption_stub(13, "figures/page-013.png");
        assert!(s.contains("figure_id: \"page-013\""));
        assert!(s.contains("source_page: 13"));
        assert!(s.contains("figures/page-013.png"));
    }

    // The detect_figure_pages / render_figure_page happy paths are
    // covered end-to-end by the milestone-2.11 RV12 ingest fixture
    // and the smoke_tests in spec_ingest::mod.
}
