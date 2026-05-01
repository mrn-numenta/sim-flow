//! Phase 4 spec ingestion: copies the source spec into the project,
//! chunks it into per-page markdown files under
//! `.sim-flow/spec-pages/<NNN>.md`, and writes a top-level TOC at
//! `.sim-flow/source-spec-toc.md` that the orchestrator inlines into
//! every session's system prompt. The agent fetches individual pages
//! via the existing `read_file` and `search` tools.
//!
//! Markdown specs are split at `#` / `##` headings; plain text and
//! anything else falls back to ~2 KB chunks at line boundaries.
//! Small specs (< `INLINE_THRESHOLD`) become a single chunk.
//!
//! Phase 5 (PDF) plugs in here by adding a third type-detection
//! branch that calls the PDF extractor; the rest of the pipeline
//! (per-page write, TOC) is shared.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Files smaller than this byte threshold land as a single page
/// rather than getting chunked. 32 KB comfortably fits a one-page
/// spec inside the orchestrator's later inlining without forcing
/// the agent to fetch pages one at a time.
pub const INLINE_THRESHOLD: usize = 32 * 1024;

/// Soft target for chunked pages. The chunker tries to keep page
/// bodies near this size; an oversized markdown section that has no
/// internal subheadings will be left as one chunk rather than split
/// mid-paragraph (the agent can still page through it).
pub const TARGET_CHUNK_BYTES: usize = 2 * 1024;

/// Result of ingesting a spec file into a project.
#[derive(Debug, Clone)]
pub struct SpecIngestSummary {
    /// Where the source spec was copied to inside the project's
    /// `.sim-flow/` directory.
    pub source_path: PathBuf,
    /// Directory containing per-page markdown files.
    pub pages_dir: PathBuf,
    /// TOC file path. Always written.
    pub toc_path: PathBuf,
    /// Number of page files written.
    pub page_count: u32,
}

/// Source kinds the ingester recognizes today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecKind {
    Markdown,
    PlainText,
    /// Reserved for Phase 5; today this returns an error so the user
    /// gets a clear "not yet supported" message instead of a silent
    /// no-op or a botched text extraction.
    Pdf,
}

impl SpecKind {
    fn detect(path: &Path) -> Result<SpecKind> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        match ext.as_deref() {
            Some("md") | Some("markdown") => Ok(SpecKind::Markdown),
            Some("txt") => Ok(SpecKind::PlainText),
            Some("pdf") => Ok(SpecKind::Pdf),
            other => Err(Error::State(format!(
                "spec ingestion: unsupported extension `{}`. Supported: .md, .markdown, .txt, .pdf",
                other.unwrap_or("<none>")
            ))),
        }
    }
}

/// Ingest `spec_path` into `<project_dir>/.sim-flow/`. Overwrites any
/// previously-ingested spec for the same project (the auto driver
/// re-ingests on every `/auto --spec ...`).
pub fn ingest_spec_file(spec_path: &Path, project_dir: &Path) -> Result<SpecIngestSummary> {
    let kind = SpecKind::detect(spec_path)?;
    let dot = project_dir.join(".sim-flow");
    let pages_dir = dot.join("spec-pages");
    let images_dir = pages_dir.join("images");
    let toc_path = dot.join("source-spec-toc.md");

    // Clean any prior ingestion artifacts so old pages don't leak
    // into a new run.
    if pages_dir.exists() {
        fs::remove_dir_all(&pages_dir)
            .map_err(|e| Error::State(format!("spec ingestion: clean pages dir: {e}")))?;
    }
    fs::create_dir_all(&pages_dir)
        .map_err(|e| Error::State(format!("spec ingestion: create pages dir: {e}")))?;

    // Copy the source into the project so it stays put even if the
    // user's original moves; downstream tools read this canonical
    // copy rather than the original path.
    let source_ext = spec_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("md");
    let source_path = dot.join(format!("source-spec.{source_ext}"));
    fs::copy(spec_path, &source_path)
        .map_err(|e| Error::State(format!("spec ingestion: copy source: {e}")))?;

    let pages = match kind {
        SpecKind::Markdown | SpecKind::PlainText => {
            let body = fs::read_to_string(&source_path)
                .map_err(|e| Error::State(format!("spec ingestion: read source: {e}")))?;
            chunk_spec(&body, kind)
        }
        SpecKind::Pdf => {
            fs::create_dir_all(&images_dir)
                .map_err(|e| Error::State(format!("spec ingestion: create images dir: {e}")))?;
            chunk_pdf(&source_path, &images_dir)?
        }
    };
    let page_count = u32::try_from(pages.len()).unwrap_or(u32::MAX);

    for (idx, page) in pages.iter().enumerate() {
        let n = idx + 1;
        let name = format!("{n:03}.md");
        let path = pages_dir.join(&name);
        fs::write(&path, &page.body)
            .map_err(|e| Error::State(format!("spec ingestion: write page {name}: {e}")))?;
    }

    let toc = build_toc(&pages, kind);
    fs::write(&toc_path, &toc)
        .map_err(|e| Error::State(format!("spec ingestion: write TOC: {e}")))?;

    Ok(SpecIngestSummary {
        source_path,
        pages_dir,
        toc_path,
        page_count,
    })
}

#[derive(Debug, Clone)]
struct Page {
    /// The chunk's body as written to disk.
    body: String,
    /// One-line title used in the TOC. Falls back to a "(no
    /// heading)" placeholder when the chunk has no obvious title.
    title: String,
}

fn chunk_spec(body: &str, kind: SpecKind) -> Vec<Page> {
    if body.len() <= INLINE_THRESHOLD {
        return vec![Page {
            body: body.to_string(),
            title: first_heading_or_line(body, kind),
        }];
    }
    match kind {
        SpecKind::Markdown => chunk_markdown(body),
        SpecKind::PlainText => chunk_plaintext(body),
        SpecKind::Pdf => unreachable!("guarded by ingest_spec_file"),
    }
}

/// PDF-specific chunker, backed by PDFium via `pdfium-render`.
/// PDFium handles every CMap and font encoding (including Identity-H
/// which lopdf chokes on), and gives us proper per-page text plus
/// image / vector-page rasterization. Each PDF page becomes one
/// markdown file; embedded raster images are saved under `images/`
/// and referenced from the page's markdown via standard
/// `![](images/...)` links.
fn chunk_pdf(source_path: &Path, images_dir: &Path) -> Result<Vec<Page>> {
    use pdfium_render::prelude::*;

    let pdfium = crate::session::pdfium_loader::load()?;
    let document = pdfium
        .load_pdf_from_file(source_path, None)
        .map_err(|e| Error::State(format!("spec ingestion: load PDF via pdfium: {e}")))?;

    let mut pages: Vec<Page> = Vec::new();
    for (idx, page) in document.pages().iter().enumerate() {
        let page_num = (idx + 1) as u32;
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        let trimmed = text.trim().to_string();
        let title = first_pdf_heading(&trimmed, page_num);

        let mut figures: Vec<String> = Vec::new();
        for (obj_idx, object) in page.objects().iter().enumerate() {
            if let Some(image) = object.as_image_object() {
                if let Some(rel) = save_pdfium_image(image, page_num, obj_idx + 1, images_dir)? {
                    figures.push(format!("- ![]({rel})"));
                } else {
                    figures.push(format!(
                        "- _Figure on page {page_num} (image #{n}) could not be encoded._",
                        n = obj_idx + 1
                    ));
                }
            }
        }

        let mut body = String::new();
        body.push_str(&format!("# Page {page_num}\n\n"));
        if trimmed.is_empty() {
            body.push_str("_(no extractable text on this page)_\n\n");
        } else {
            body.push_str(&trimmed);
            body.push_str("\n\n");
        }
        if !figures.is_empty() {
            body.push_str("## Figures on this page\n\n");
            for f in figures {
                body.push_str(&f);
                body.push('\n');
            }
        }
        pages.push(Page { body, title });
    }
    if pages.is_empty() {
        pages.push(Page {
            body: "_(empty PDF)_\n".into(),
            title: "(empty)".into(),
        });
    }
    Ok(pages)
}

fn save_pdfium_image(
    image: &pdfium_render::prelude::PdfPageImageObject<'_>,
    page_num: u32,
    image_idx: usize,
    images_dir: &Path,
) -> Result<Option<String>> {
    let bitmap = match image.get_raw_bitmap() {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let dynimg = bitmap.as_image();
    let filename = format!("page-{page_num:03}-img-{image_idx}.png");
    let abs = images_dir.join(&filename);
    if let Err(err) = dynimg.save_with_format(&abs, image::ImageFormat::Png) {
        return Err(Error::State(format!(
            "spec ingestion: write image {}: {err}",
            abs.display()
        )));
    }
    Ok(Some(format!("images/{filename}")))
}

#[expect(
    dead_code,
    reason = "lopdf-based image extraction is retained for reference"
)]
enum PdfImage {
    /// Image was extracted to disk; `rel_path` is relative to the
    /// per-page markdown file (i.e. `images/<file>`).
    Saved { rel_path: String, kind: String },
    /// Image is present in the PDF but extraction is not yet
    /// supported (non-JPEG filter chain).
    Skipped { reason: String, kind: String },
}

#[expect(
    dead_code,
    reason = "lopdf-based image extraction is retained for reference"
)]
fn extract_images_for_page(
    doc: &lopdf::Document,
    page_num: u32,
    images_dir: &Path,
) -> Result<Vec<PdfImage>> {
    use lopdf::{Object, ObjectId};
    let mut out = Vec::new();
    let page_id: ObjectId = match doc.get_pages().get(&page_num).copied() {
        Some(id) => id,
        None => return Ok(out),
    };
    // `get_page_resources` returns (Option<&Dictionary>, Vec<ObjectId>):
    // the first element is populated only when /Resources is inline
    // on the page; the second collects references walked from the
    // page up through any inherited /Pages parent. Try inline first,
    // then walk the references.
    let (inline_dict, resource_ids) = doc.get_page_resources(page_id);
    let mut resources_chain: Vec<lopdf::Dictionary> = Vec::new();
    if let Some(d) = inline_dict {
        resources_chain.push(d.clone());
    }
    for rid in resource_ids {
        if let Ok(d) = doc.get_dictionary(rid) {
            resources_chain.push(d.clone());
        }
    }
    if resources_chain.is_empty() {
        return Ok(out);
    }
    // Find the first /XObject entry across the chain.
    let mut xobjects: Option<lopdf::Dictionary> = None;
    for resources in &resources_chain {
        match resources.get(b"XObject") {
            Ok(Object::Dictionary(d)) => {
                xobjects = Some(d.clone());
                break;
            }
            Ok(Object::Reference(id)) => {
                if let Ok(d) = doc.get_dictionary(*id) {
                    xobjects = Some(d.clone());
                    break;
                }
            }
            _ => {}
        }
    }
    let xobjects = match xobjects {
        Some(d) => d,
        None => return Ok(out),
    };
    let mut img_idx = 0u32;
    for (_name, obj) in xobjects.iter() {
        let stream_id = match obj {
            Object::Reference(id) => *id,
            _ => continue,
        };
        let stream = match doc.get_object(stream_id) {
            Ok(Object::Stream(s)) => s,
            _ => continue,
        };
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|v| v.as_name_str().ok());
        if subtype != Some("Image") {
            continue;
        }
        img_idx += 1;
        let filters: Vec<String> = match stream.dict.get(b"Filter") {
            Ok(Object::Name(n)) => vec![String::from_utf8_lossy(n).into_owned()],
            Ok(Object::Array(arr)) => arr
                .iter()
                .filter_map(|f| match f {
                    Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let kind = filters.join("+");
        if filters.iter().any(|f| f == "DCTDecode") {
            // The raw stream is a complete JPEG file; save as-is.
            let name = format!("page-{page_num:03}-img-{img_idx}.jpg");
            let abs = images_dir.join(&name);
            fs::write(&abs, &stream.content)
                .map_err(|e| Error::State(format!("spec ingestion: write image {name}: {e}")))?;
            out.push(PdfImage::Saved {
                rel_path: format!("images/{name}"),
                kind: kind.clone(),
            });
        } else {
            out.push(PdfImage::Skipped {
                reason: "non-JPEG filter chain (Phase 6 will add proper decoding)".into(),
                kind: if kind.is_empty() {
                    "(unknown)".into()
                } else {
                    kind
                },
            });
        }
    }
    Ok(out)
}

/// Detect lopdf's "I cannot decode this CMap" failure mode. lopdf
/// returns the literal string `?Identity-H Unimplemented?` for every
/// glyph it can't decode -- a single sentence becomes thousands of
/// repeats of that placeholder and the whole page is garbage. We
/// refuse ingestion if more than half the extracted text is the
/// placeholder; an empty / no-text PDF won't trigger this (the
/// chunker's per-page "(no extractable text)" branch handles that
/// case separately).
#[allow(dead_code)]
fn is_pdf_text_garbage(extracted: &str) -> bool {
    if extracted.is_empty() {
        return false;
    }
    const PLACEHOLDER: &str = "?Identity-H Unimplemented?";
    let placeholder_chars = extracted.matches(PLACEHOLDER).count() * PLACEHOLDER.len();
    placeholder_chars * 2 >= extracted.len()
}

fn first_pdf_heading(text: &str, page_num: u32) -> String {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        return t.chars().take(120).collect();
    }
    format!("Page {page_num} (no extractable text)")
}

fn chunk_markdown(body: &str) -> Vec<Page> {
    // Split at top-level (`#`) and second-level (`##`) heading lines.
    // Anything before the first heading goes into a "front matter"
    // chunk so a YAML/preamble block doesn't get attached to chapter 1.
    let mut pages: Vec<Page> = Vec::new();
    let mut current = String::new();
    let mut current_title = String::from("Front matter");
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        let is_h1 = trimmed.starts_with("# ") && !trimmed.starts_with("## ");
        let is_h2 = trimmed.starts_with("## ") && !trimmed.starts_with("### ");
        if is_h1 || is_h2 {
            if !current.trim().is_empty() {
                pages.push(Page {
                    body: std::mem::take(&mut current),
                    title: current_title.clone(),
                });
            }
            current_title = trimmed.trim_start_matches('#').trim().to_string();
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        pages.push(Page {
            body: current,
            title: current_title,
        });
    }
    if pages.is_empty() {
        // Fallback: source had no headings at all.
        pages.push(Page {
            body: body.to_string(),
            title: "(no heading)".into(),
        });
    }
    pages
}

fn chunk_plaintext(body: &str) -> Vec<Page> {
    // Greedy line-grouping into ~TARGET_CHUNK_BYTES chunks; never
    // splits mid-line.
    let mut pages: Vec<Page> = Vec::new();
    let mut current = String::new();
    for line in body.split_inclusive('\n') {
        if !current.is_empty() && current.len() + line.len() > TARGET_CHUNK_BYTES {
            pages.push(Page {
                body: std::mem::take(&mut current),
                title: first_heading_or_line(&current, SpecKind::PlainText),
            });
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        let title = first_heading_or_line(&current, SpecKind::PlainText);
        pages.push(Page {
            body: current,
            title,
        });
    }
    if pages.is_empty() {
        pages.push(Page {
            body: body.to_string(),
            title: "(empty)".into(),
        });
    }
    // Re-derive titles after the fact since the take()-based
    // accumulator lost them. Use the first non-blank line of each
    // chunk as the title.
    for page in &mut pages {
        page.title = first_heading_or_line(&page.body, SpecKind::PlainText);
    }
    pages
}

fn first_heading_or_line(s: &str, kind: SpecKind) -> String {
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if kind == SpecKind::Markdown
            && let Some(rest) = t.strip_prefix('#')
        {
            return rest.trim_start_matches('#').trim().to_string();
        }
        return t.chars().take(120).collect();
    }
    "(empty)".into()
}

fn build_toc(pages: &[Page], kind: SpecKind) -> String {
    let mut out = String::new();
    out.push_str("# Source spec TOC\n\n");
    out.push_str(
        "The full spec is staged at `.sim-flow/source-spec.<ext>` and chunked into per-page \
        markdown under `.sim-flow/spec-pages/`. Read pages on demand with the existing tools \
        (do NOT inline the whole spec yourself):\n\n",
    );
    out.push_str(
        "- `read_file(\".sim-flow/spec-pages/042.md\")` -- single page (zero-padded NNN).\n",
    );
    out.push_str(
        "- `search(\"<pattern>\", \".sim-flow/spec-pages/\")` -- regex across all pages.\n",
    );
    out.push_str("- `list_dir(\".sim-flow/spec-pages/\")` -- list every page file.\n\n");
    let kind_label = match kind {
        SpecKind::Markdown => "markdown",
        SpecKind::PlainText => "plain text",
        SpecKind::Pdf => "PDF",
    };
    out.push_str(&format!(
        "Source kind: **{kind_label}**. {} page(s).\n\n",
        pages.len()
    ));
    out.push_str("## Page index\n\n");
    for (idx, page) in pages.iter().enumerate() {
        let n = idx + 1;
        out.push_str(&format!("- `{n:03}.md` -- {}\n", page.title));
    }
    out
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_markdown_lands_as_one_page() {
        let body = "# Title\n\nA short spec.\n";
        let pages = chunk_spec(body, SpecKind::Markdown);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].body, body);
    }

    #[test]
    fn large_markdown_splits_at_top_level_headings() {
        let big_section = "x".repeat(40 * 1024);
        let body = format!("# Intro\nfoo\n\n# Architecture\n{big_section}\n\n## Pipelines\nbar\n",);
        let pages = chunk_spec(&body, SpecKind::Markdown);
        // Chunked at # Intro, # Architecture, ## Pipelines.
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].title, "Intro");
        assert_eq!(pages[1].title, "Architecture");
        assert_eq!(pages[2].title, "Pipelines");
    }

    #[test]
    fn plaintext_chunks_at_line_boundaries() {
        let line = "this is some plain text content. ".repeat(50);
        let body: String = std::iter::repeat_n(line, 100)
            .collect::<Vec<_>>()
            .join("\n");
        let pages = chunk_spec(&body, SpecKind::PlainText);
        assert!(pages.len() > 1, "expected multi-chunk output");
        for p in &pages {
            // Soft target; allow some slack for the last line that
            // would otherwise spill over.
            assert!(
                p.body.len() <= TARGET_CHUNK_BYTES * 2,
                "chunk too big: {} bytes",
                p.body.len()
            );
        }
    }

    #[test]
    fn ingest_writes_pages_and_toc() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let big_section = "y".repeat(40 * 1024);
        let spec_body = format!("# A\nintro\n\n# B\n{big_section}\n");
        let spec = tmp.path().join("spec.md");
        std::fs::write(&spec, &spec_body).unwrap();

        let summary = ingest_spec_file(&spec, project).unwrap();
        assert!(summary.toc_path.exists());
        assert!(summary.source_path.exists());
        assert_eq!(summary.page_count, 2);
        let toc = std::fs::read_to_string(&summary.toc_path).unwrap();
        assert!(toc.contains("Source spec TOC"));
        assert!(toc.contains("`001.md` -- A"));
        assert!(toc.contains("`002.md` -- B"));
        assert!(project.join(".sim-flow/spec-pages/001.md").exists());
        assert!(project.join(".sim-flow/spec-pages/002.md").exists());
    }

    #[test]
    fn identity_h_garbage_detected() {
        let placeholder = "?Identity-H Unimplemented?";
        // 100 placeholder copies + tiny preamble.
        let body = format!("Title\n{}", placeholder.repeat(100));
        assert!(is_pdf_text_garbage(&body));
        // Real text with one stray placeholder is fine.
        let real = format!("Section 1\nThis is the spec body.\n{placeholder}");
        assert!(!is_pdf_text_garbage(&real));
        // Empty input is not garbage; the per-page "no extractable
        // text" branch handles it.
        assert!(!is_pdf_text_garbage(""));
    }

    #[test]
    fn ingest_invalid_pdf_returns_load_error() {
        // An obviously-invalid file should fail at load time with a
        // clear message rather than silently producing empty pages.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let spec = tmp.path().join("ds.pdf");
        std::fs::write(&spec, b"not actually a pdf").unwrap();
        let err = ingest_spec_file(&spec, project).unwrap_err();
        let msg = format!("{err}").to_lowercase();
        assert!(
            msg.contains("pdf") && (msg.contains("load") || msg.contains("pdfium")),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn ingest_minimal_pdf_writes_one_page_per_pdf_page() {
        // Construct a tiny single-page PDF with one bit of text using
        // lopdf's builder, ingest it, and verify the per-page file
        // and TOC are written. Image extraction is exercised by the
        // dedicated DCTDecode test below.
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
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let spec = tmp.path().join("ds.pdf");
        doc.save(&spec).unwrap();

        let summary = ingest_spec_file(&spec, project).unwrap();
        assert_eq!(summary.page_count, 1);
        assert!(project.join(".sim-flow/spec-pages/001.md").exists());
        let body = std::fs::read_to_string(project.join(".sim-flow/spec-pages/001.md")).unwrap();
        assert!(
            body.contains("Hello, sim-flow!"),
            "extracted text missing: {body}",
        );
        let toc = std::fs::read_to_string(&summary.toc_path).unwrap();
        assert!(toc.contains("Source kind: **PDF**"));
        assert!(toc.contains("`001.md` -- Hello"));
    }

    #[test]
    #[ignore = "lopdf-era image fixture; superseded by pdfium which re-encodes images as PNG. Kept for reference."]
    fn dctdecode_image_extraction_saves_jpeg() {
        // Construct a 1-page PDF with an XObject image whose Filter
        // is DCTDecode and whose stream is a tiny valid JPEG. The
        // chunker should save the bytes verbatim to images/.
        use lopdf::content::Content;
        use lopdf::dictionary;
        use lopdf::{Document, Object, Stream};

        // 60-byte minimum-shape JPEG (start-of-image, JFIF marker,
        // end-of-image). Not a valid renderable image but lopdf
        // copies stream bytes verbatim so the file written to disk
        // round-trips. We only verify the bytes survive.
        let jpeg_bytes: Vec<u8> = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
        ];

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut img_stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            jpeg_bytes.clone(),
        );
        // lopdf would otherwise add a /Length entry; ensure raw
        // content survives.
        img_stream.allows_compression = false;
        let img_id = doc.add_object(img_stream);
        let resources_id = doc.add_object(dictionary! {
            "XObject" => dictionary! {
                "Im0" => img_id,
            },
        });
        let content = Content { operations: vec![] };
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
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".sim-flow")).unwrap();
        let spec = tmp.path().join("ds.pdf");
        doc.save(&spec).unwrap();

        let _summary = ingest_spec_file(&spec, project).unwrap();
        // The image landed under images/ and the page markdown
        // references it.
        let img_path = project.join(".sim-flow/spec-pages/images/page-001-img-1.jpg");
        assert!(img_path.exists(), "expected JPEG at {}", img_path.display());
        let body = std::fs::read_to_string(project.join(".sim-flow/spec-pages/001.md")).unwrap();
        assert!(
            body.contains("images/page-001-img-1.jpg"),
            "page 001 markdown should reference the extracted image:\n{body}",
        );
        // And the bytes round-tripped.
        let written = std::fs::read(&img_path).unwrap();
        assert_eq!(written, jpeg_bytes);
    }
}
