//! Top-level pipeline types and orchestrator.
//!
//! See architecture chapter 1.9 for the public API contract.

use std::path::{Path, PathBuf};

use crate::Result;

use super::stages;

/// Kind of source the pipeline is invoked against. Tracked through
/// every stage; later stages skip work that doesn't apply (e.g.
/// page-chrome stripping is a no-op for markdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Pdf,
    Markdown,
    Text,
    /// Empty corpus: caller invoked the pipeline with no primary
    /// source. Stages 1-6 emit no-op outputs; stage 7 writes a
    /// manifest with `source_kind = "none"` and nothing else.
    None,
}

impl SourceKind {
    /// Best-effort detection from a path's extension. PDF / markdown /
    /// text only; unknown extensions are a hard error in stage 1.
    pub fn from_path(path: &Path) -> Option<SourceKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Some(SourceKind::Pdf),
            "md" | "markdown" => Some(SourceKind::Markdown),
            "txt" | "text" => Some(SourceKind::Text),
            _ => None,
        }
    }

    /// Tag string used inside `manifest.toml::source_kind`.
    pub fn as_manifest_tag(self) -> &'static str {
        match self {
            SourceKind::Pdf => "pdf",
            SourceKind::Markdown => "markdown",
            SourceKind::Text => "text",
            SourceKind::None => "none",
        }
    }
}

/// One source document passed to the pipeline. Holds the absolute
/// path the loader will open; the loader detects the kind by
/// extension at stage 1.
#[derive(Debug, Clone)]
pub struct SourceSpec {
    pub path: PathBuf,
}

impl SourceSpec {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

/// A peer spec registration. `id` is the user-facing label that
/// `references.toml.peer_id` reconciles against.
#[derive(Debug, Clone)]
pub struct PeerSpec {
    pub id: String,
    pub source: SourceSpec,
}

/// Full request to the pipeline.
#[derive(Debug, Clone)]
pub struct IngestRequest {
    /// Primary source spec; `None` means "no source" (empty-corpus
    /// path; the agent's spec.md authoring loop is the only writer
    /// of content for that project).
    pub primary: Option<SourceSpec>,
    pub peers: Vec<PeerSpec>,
    pub config: IngestConfig,
    /// Project root. The pipeline writes under
    /// `<project_root>/.sim-flow/spec-ingest/`.
    pub project_root: PathBuf,
}

/// Outcome of a successful pipeline run. Counts are taken from the
/// emitted manifest so the caller doesn't have to re-parse it.
#[derive(Debug, Clone, Default)]
pub struct IngestOutcome {
    pub manifest_path: PathBuf,
    pub primary_chunk_count: usize,
    pub primary_figure_count: usize,
    pub primary_signal_table_count: usize,
    pub primary_stub_count: usize,
    pub primary_tbd_count: usize,
    pub warnings: Vec<IngestWarning>,
}

/// A diagnostic surfaced by a stage. Aggregated into the manifest's
/// `warnings` table (§1.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestWarning {
    /// Short machine-readable identifier (e.g. `no_headings_detected`,
    /// `signal_table_misfire`).
    pub code: String,
    /// Free-form human-readable description.
    pub message: String,
    /// Stage that produced the warning (1-7).
    pub stage: u8,
}

impl IngestWarning {
    pub fn new(code: impl Into<String>, message: impl Into<String>, stage: u8) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            stage,
        }
    }
}

/// Ingest configuration. Loaded from
/// `<project>/.sim-flow/spec-ingest.config.toml` if present;
/// defaults apply for missing fields. See architecture §1.7.
#[derive(Debug, Clone, Default)]
pub struct IngestConfig {
    pub figures: FiguresConfig,
    pub chunking: ChunkingConfig,
    pub chrome_stripping: ChromeStrippingConfig,
    pub signal_table_detection: SignalTableConfig,
}

#[derive(Debug, Clone)]
pub struct FiguresConfig {
    pub dpi: u32,
    pub format: String,
    /// Page contains figure content if it has at least one image
    /// XObject OR its vector-drawing op count exceeds this threshold.
    pub vector_op_threshold: u32,
}

impl Default for FiguresConfig {
    fn default() -> Self {
        Self {
            dpi: 150,
            format: "png".into(),
            vector_op_threshold: 20,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    pub max_chunk_chars: usize,
    pub min_chunk_chars: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_chunk_chars: 8000,
            min_chunk_chars: 200,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChromeStrippingConfig {
    pub enabled: bool,
    pub appearance_threshold: f32,
}

impl Default for ChromeStrippingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            appearance_threshold: 0.5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignalTableConfig {
    pub enabled: bool,
    /// Each inner Vec is a canonical column-set match. Comparisons
    /// are case-insensitive and order-tolerant.
    pub header_aliases: Vec<Vec<String>>,
}

impl Default for SignalTableConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            header_aliases: vec![
                vec![
                    "Signal".into(),
                    "Direction".into(),
                    "To/From".into(),
                    "Description".into(),
                ],
                vec![
                    "Signal".into(),
                    "Dir".into(),
                    "From/To".into(),
                    "Description".into(),
                ],
                vec![
                    "Signal".into(),
                    "Direction".into(),
                    "From/To".into(),
                    "Description".into(),
                ],
            ],
        }
    }
}

/// Internal pipeline orchestrator. Runs the seven stages in order,
/// threading each stage's output into the next.
pub struct Pipeline {
    pub request: IngestRequest,
}

impl Pipeline {
    pub fn new(request: IngestRequest) -> Self {
        Self { request }
    }

    pub fn run(self) -> Result<IngestOutcome> {
        run_pipeline(self.request)
    }
}

/// Programmatic API per chapter 1.9. Thin wrapper over `Pipeline`.
pub fn run(request: IngestRequest) -> Result<IngestOutcome> {
    Pipeline::new(request).run()
}

/// Concrete top-down driver. Each stage is a pure function over the
/// previous stage's output; the orchestrator concatenates warnings
/// into a single per-run vector that lands in the manifest.
fn run_pipeline(request: IngestRequest) -> Result<IngestOutcome> {
    let mut warnings: Vec<IngestWarning> = Vec::new();

    let loaded = stages::loading::load_primary(&request, &mut warnings)?;
    let (pages, chrome) = stages::chrome::strip_chrome(loaded, &request.config, &mut warnings);
    let mut tree = stages::parse::parse_hierarchy(&pages, &mut warnings)?;
    stages::classify::classify(&mut tree, &request.config, &mut warnings);
    let refs = stages::references::parse_references(&tree, &mut warnings);
    let figures = stages::figures::extract_figures(&pages, &request.config, &mut warnings)?;
    let outcome =
        stages::emit::emit_corpus(&request, &tree, &chrome, &refs, &figures, warnings.clone())?;
    Ok(IngestOutcome {
        warnings,
        ..outcome
    })
}
