//! Stage 6: figure detection and page-region rendering.

use pdfium_render::prelude::*;

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
    let pages_with_figures = detect_figure_pages(pdf.document(), config);

    let mut out = Vec::new();
    for page_num in pages_with_figures {
        match render_figure_page(pdf.document(), page_num, config.figures.dpi) {
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
/// heuristic: at least one image XObject OR vector-op count above
/// the configured threshold.
pub fn detect_figure_pages(doc: &PdfDocument<'_>, config: &IngestConfig) -> Vec<u32> {
    let mut pages = Vec::new();
    for (idx, page) in doc.pages().iter().enumerate() {
        let page_num = (idx + 1) as u32;
        let mut images = 0u32;
        let mut paths = 0u32;
        for object in page.objects().iter() {
            if object.as_image_object().is_some() {
                images += 1;
            } else if object.as_path_object().is_some() {
                paths += 1;
            }
        }
        if images >= 1 || paths >= config.figures.vector_op_threshold {
            pages.push(page_num);
        }
    }
    pages
}

/// Render a single PDF page to PNG bytes.
pub fn render_figure_page(doc: &PdfDocument<'_>, page: u32, dpi: u32) -> Result<Vec<u8>> {
    let pages = doc.pages();
    let total = pages.len();
    if page < 1 || (page as u16) > total {
        return Err(Error::State(format!(
            "spec ingest: figure render: page {page} out of range (1..={total})"
        )));
    }
    let page_obj = pages
        .get(page as u16 - 1)
        .map_err(|e| Error::State(format!("spec ingest: pdfium page {page}: {e}")))?;
    // pdfium DPI -> pixel scale: 72 PDF user units == 1 inch.
    let scale = dpi as f32 / 72.0;
    let width_px = (page_obj.width().value * scale).ceil() as i32;
    let height_px = (page_obj.height().value * scale).ceil() as i32;
    let config = PdfRenderConfig::new()
        .set_target_width(width_px)
        .set_target_height(height_px);
    let bitmap = page_obj
        .render_with_config(&config)
        .map_err(|e| Error::State(format!("spec ingest: pdfium render: {e}")))?;
    let dynimg = bitmap.as_image();
    let mut buf = std::io::Cursor::new(Vec::new());
    dynimg
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| Error::State(format!("spec ingest: png encode: {e}")))?;
    Ok(buf.into_inner())
}

fn caption_stub(page: u32, rel_png: &str) -> String {
    format!(
        "---\nfigure_id: \"page-{page:03}\"\nsource_page: {page}\nsource_chunk_id: \"\"\nrole: \"\"\nreferenced_elements: []\n---\n\n<!-- caption stub for {rel_png}; populate by hand or via captioning hook -->\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn caption_stub_includes_page_and_path() {
        let s = caption_stub(13, "figures/page-013.png");
        assert!(s.contains("figure_id: \"page-013\""));
        assert!(s.contains("source_page: 13"));
        assert!(s.contains("figures/page-013.png"));
    }

    /// Build a tiny one-page PDF with no figure content and verify
    /// `detect_figure_pages` returns empty. The figure-extraction
    /// success path is exercised by the milestone-2.11 integration
    /// test (RV12 fixture).
    #[test]
    fn detect_figure_pages_returns_empty_for_text_only() {
        use lopdf::content::{Content, Operation};
        use lopdf::dictionary;
        use lopdf::{Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
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
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal("just text")]),
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
        let path = tmp.path().join("text.pdf");
        let mut f = std::fs::File::create(&path).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        f.write_all(&bytes).unwrap();
        drop(f);

        let pdfium = crate::session::spec_ingest::stages::loading::shared_pdfium().unwrap();
        let document = pdfium.load_pdf_from_file(&path, None).unwrap();
        let config = IngestConfig::default();
        let pages_with_figures = detect_figure_pages(&document, &config);
        assert!(pages_with_figures.is_empty());
    }
}
