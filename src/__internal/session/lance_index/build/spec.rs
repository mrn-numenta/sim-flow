//! Spec-tree build pipeline (Chapter 3 §3.9.2).
//!
//! Three sub-builds, all writing into `<project>/.sim-flow/lance-index/`:
//!
//! - `build_spec_chunks` walks `spec-ingest/primary/chunks/*.md` plus
//!   `spec-ingest/peers/<id>/chunks/*.md` and writes the
//!   `spec_chunks` table.
//! - `build_signal_table_rows` aggregates every
//!   `spec-ingest/**/tables/signals/*.toml` plus, if present, the
//!   `Block`-section signal rows from `docs/spec.md`.
//! - `build_cross_spec_refs` aggregates
//!   `spec-ingest/**/references.toml`.
//!
//! Each writes its `*.lance/` directory atomically (`.tmp/` → rename)
//! and updates the per-tree `manifest.toml` + `embedder.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    FixedSizeListArray, Float32Array, ListArray, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
    builder::{ListBuilder, StringBuilder},
};
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::session::embedder::EmbeddingClient;
use crate::session::spec_md::parser::parse as parse_spec_md;
use crate::session::spec_md::types::{Block, SpecMd};

use super::super::lock::LanceLock;
use super::super::manifests::{EmbedderManifest, SpecIndexManifest};
use super::super::schemas::{cross_spec_refs_schema, signal_table_rows_schema, spec_chunks_schema};
use super::framework::run_async;

/// Common options passed to every spec-side build.
#[derive(Debug, Clone)]
pub struct SpecBuildOpts {
    pub project_root: PathBuf,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecBuildOutcome {
    pub spec_chunks_rows: u64,
    pub signal_table_rows: u64,
    pub cross_spec_refs_rows: u64,
    pub manifest_path: PathBuf,
    pub embedder_path: PathBuf,
}

#[derive(Debug)]
pub enum SpecBuildError {
    Io(std::io::Error),
    MissingIngestManifest(PathBuf),
    Toml(String),
    SpecMd(crate::session::spec_md::parser::SpecMdParseError),
    Manifest(super::super::manifests::ManifestError),
    Embed(crate::session::embedder::EmbedError),
    Lance(lancedb::Error),
    Arrow(arrow_schema::ArrowError),
    Lock(super::super::lock::LockError),
}

impl std::fmt::Display for SpecBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecBuildError::Io(e) => write!(f, "spec-index build I/O: {e}"),
            SpecBuildError::MissingIngestManifest(p) => write!(
                f,
                "spec-index build: missing spec-ingest manifest at {} \
                 (run `sim-flow ingest` first)",
                p.display()
            ),
            SpecBuildError::Toml(m) => write!(f, "spec-index build toml: {m}"),
            SpecBuildError::SpecMd(e) => write!(f, "spec-index spec.md parse: {e}"),
            SpecBuildError::Manifest(e) => write!(f, "spec-index manifest: {e}"),
            SpecBuildError::Embed(e) => write!(f, "spec-index embed: {e}"),
            SpecBuildError::Lance(e) => write!(f, "spec-index lance: {e}"),
            SpecBuildError::Arrow(e) => write!(f, "spec-index arrow: {e}"),
            SpecBuildError::Lock(e) => write!(f, "spec-index lock: {e}"),
        }
    }
}

impl std::error::Error for SpecBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SpecBuildError::Io(e) => Some(e),
            SpecBuildError::SpecMd(e) => Some(e),
            SpecBuildError::Manifest(e) => Some(e),
            SpecBuildError::Embed(e) => Some(e),
            SpecBuildError::Lance(e) => Some(e),
            SpecBuildError::Arrow(e) => Some(e),
            SpecBuildError::Lock(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SpecBuildError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<crate::session::spec_md::parser::SpecMdParseError> for SpecBuildError {
    fn from(e: crate::session::spec_md::parser::SpecMdParseError) -> Self {
        Self::SpecMd(e)
    }
}
impl From<super::super::manifests::ManifestError> for SpecBuildError {
    fn from(e: super::super::manifests::ManifestError) -> Self {
        Self::Manifest(e)
    }
}
impl From<crate::session::embedder::EmbedError> for SpecBuildError {
    fn from(e: crate::session::embedder::EmbedError) -> Self {
        Self::Embed(e)
    }
}
impl From<lancedb::Error> for SpecBuildError {
    fn from(e: lancedb::Error) -> Self {
        Self::Lance(e)
    }
}
impl From<arrow_schema::ArrowError> for SpecBuildError {
    fn from(e: arrow_schema::ArrowError) -> Self {
        Self::Arrow(e)
    }
}
impl From<super::super::lock::LockError> for SpecBuildError {
    fn from(e: super::super::lock::LockError) -> Self {
        Self::Lock(e)
    }
}

/// One pending row before embedding.
#[derive(Debug, Clone)]
struct PendingSpecChunk {
    id: String,
    source_id: String,
    breadcrumb: Vec<String>,
    section_heading: String,
    source_page_start: u32,
    source_page_end: u32,
    kind: String,
    text: String,
    text_sha256: String,
    contained_signal_tables: Vec<String>,
    contained_figures: Vec<String>,
    /// Phase 9 milestone 9.13: semantic role from `format.json`.
    /// `"unknown"` when the chunk's front-matter has no
    /// `spec_md_role` key or it's empty.
    spec_md_role: String,
    /// `"architectural" | "micro" | "mixed" | "unknown"`.
    layer: String,
    /// Possibly-empty list of acronym strings.
    acronyms_referenced: Vec<String>,
    /// Optional domain refs. `None` when absent or empty in
    /// front matter so the column can stay nullable.
    clock_domain: Option<String>,
    power_domain: Option<String>,
    reset_domain: Option<String>,
}

/// On-disk shape of a chunk-md file's YAML front matter. Mirrors the
/// fields Phase 2's emit stage writes; unknown / extra fields are
/// tolerated.
#[derive(Debug, Default, Deserialize)]
struct ChunkFrontMatter {
    #[serde(default)]
    chunk_id: String,
    #[serde(default)]
    breadcrumb: Vec<String>,
    #[serde(default)]
    section_heading: String,
    #[serde(default)]
    source_page_start: Option<u32>,
    #[serde(default)]
    source_page_end: Option<u32>,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    contained_signal_tables: Vec<String>,
    #[serde(default)]
    contained_figures: Vec<String>,
    /// Phase 9 milestone 9.13: role-tag front matter (Chapter 7 §7.8).
    #[serde(default)]
    spec_md_role: String,
    #[serde(default)]
    layer: String,
    #[serde(default)]
    acronyms_referenced: Vec<String>,
    #[serde(default)]
    clock_domain: String,
    #[serde(default)]
    power_domain: String,
    #[serde(default)]
    reset_domain: String,
}

/// Full spec-index build: chunks + signals + refs, all under a
/// single writer lock against the per-project index root.
pub fn build_spec_index(
    opts: &SpecBuildOpts,
    embedder: &Arc<dyn EmbeddingClient>,
) -> Result<SpecBuildOutcome, SpecBuildError> {
    let index_root = opts.project_root.join(".sim-flow").join("lance-index");
    std::fs::create_dir_all(&index_root)?;
    let lock_path = index_root.join("spec-index.lock");
    let _lock = LanceLock::acquire(&lock_path)?;

    let spec_chunks_rows = build_spec_chunks(opts, embedder, &index_root)?;
    let signal_table_rows = build_signal_table_rows(opts, &index_root)?;
    let cross_spec_refs_rows = build_cross_spec_refs(opts, &index_root)?;

    // Manifests.
    let spec_ingest_manifest_path = opts
        .project_root
        .join(".sim-flow")
        .join("spec-ingest")
        .join("manifest.toml");
    let spec_ingest_source_sha256 =
        read_ingest_source_sha(&spec_ingest_manifest_path).unwrap_or_default();
    let spec_md_path = opts.project_root.join("docs").join("spec.md");
    let spec_md_sha256 = if spec_md_path.is_file() {
        sha256_hex(&std::fs::read_to_string(&spec_md_path)?)
    } else {
        String::new()
    };

    let mut counts = BTreeMap::new();
    counts.insert("spec_chunks".into(), spec_chunks_rows);
    counts.insert("signal_table_rows".into(), signal_table_rows);
    counts.insert("cross_spec_refs".into(), cross_spec_refs_rows);

    // schema_version is bumped to 2 in Phase 9 milestone 9.13 because
    // the `spec_chunks` Arrow schema gained six new columns
    // (`spec_md_role`, `layer`, `acronyms_referenced`, `clock_domain`,
    // `power_domain`, `reset_domain`). Indices built against the
    // version-1 schema are missing these columns and must be rebuilt.
    let manifest = SpecIndexManifest {
        schema_version: 2,
        indexed_at: Utc::now().to_rfc3339(),
        spec_ingest_manifest: spec_ingest_manifest_path.to_string_lossy().to_string(),
        spec_ingest_source_sha256,
        spec_md_sha256,
        counts,
    };
    let manifest_path = index_root.join("manifest.toml");
    manifest.save(&manifest_path)?;

    let embedder_manifest = EmbedderManifest {
        schema_version: 1,
        provider: embedder.provider().to_string(),
        base_url: String::new(),
        model: embedder.model_id().to_string(),
        dimension: embedder.dimension(),
        indexed_at: manifest.indexed_at.clone(),
        auth: None,
    };
    let embedder_path = index_root.join("embedder.toml");
    embedder_manifest.save(&embedder_path)?;

    Ok(SpecBuildOutcome {
        spec_chunks_rows,
        signal_table_rows,
        cross_spec_refs_rows,
        manifest_path,
        embedder_path,
    })
}

/// Build the `spec_chunks` table.
pub fn build_spec_chunks(
    opts: &SpecBuildOpts,
    embedder: &Arc<dyn EmbeddingClient>,
    index_root: &Path,
) -> Result<u64, SpecBuildError> {
    let ingest_root = opts.project_root.join(".sim-flow").join("spec-ingest");
    let manifest_path = ingest_root.join("manifest.toml");
    if !manifest_path.is_file() {
        return Err(SpecBuildError::MissingIngestManifest(manifest_path));
    }

    let mut pending: Vec<PendingSpecChunk> = Vec::new();
    let primary_chunks = ingest_root.join("primary").join("chunks");
    if primary_chunks.is_dir() {
        for path in walk_files(&primary_chunks, &["md"])? {
            if let Some(chunk) = read_chunk_md(&path, "primary")? {
                pending.push(chunk);
            }
        }
    }
    let peers_dir = ingest_root.join("peers");
    if peers_dir.is_dir() {
        let mut peer_dirs: Vec<_> = std::fs::read_dir(&peers_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        peer_dirs.sort_by_key(|e| e.file_name());
        for peer in peer_dirs {
            let id = peer.file_name().to_string_lossy().to_string();
            let cdir = peer.path().join("chunks");
            if cdir.is_dir() {
                for path in walk_files(&cdir, &["md"])? {
                    if let Some(chunk) = read_chunk_md(&path, &id)? {
                        pending.push(chunk);
                    }
                }
            }
        }
    }

    // Always embed all rows in v1 (the corpus is small; incremental
    // rebuild lands when needed).
    let _ = opts.force;
    let vectors = if pending.is_empty() {
        Vec::new()
    } else {
        let texts: Vec<&str> = pending.iter().map(|c| c.text.as_str()).collect();
        run_async(embedder.embed(&texts))?
    };

    let schema = spec_chunks_schema(embedder.dimension());
    let n = pending.len() as u64;
    if pending.is_empty() {
        write_empty_table(index_root, "spec_chunks.lance", schema)?;
        return Ok(0);
    }
    let batch = build_spec_chunks_batch(schema.clone(), &pending, &vectors, embedder.dimension())?;
    atomic_write_table(index_root, "spec_chunks.lance", schema, batch)?;
    Ok(n)
}

/// Build the `signal_table_rows` table.
pub fn build_signal_table_rows(
    opts: &SpecBuildOpts,
    index_root: &Path,
) -> Result<u64, SpecBuildError> {
    let mut rows: Vec<PendingSignalRow> = Vec::new();
    let ingest_root = opts.project_root.join(".sim-flow").join("spec-ingest");
    if ingest_root.is_dir() {
        for path in walk_tables_signals(&ingest_root)? {
            let body = std::fs::read_to_string(&path)?;
            let parsed: SourceSignalTableFile = toml::from_str(&body)
                .map_err(|e| SpecBuildError::Toml(format!("{}: {e}", path.display())))?;
            for row in parsed.rows {
                let source_id = parsed.source_id.clone().unwrap_or_else(|| "primary".into());
                let chunk_id = parsed.chunk_id.clone().unwrap_or_default();
                let breadcrumb = parsed.breadcrumb.clone();
                let stage = breadcrumb.last().cloned().unwrap_or_default();
                rows.push(PendingSignalRow {
                    source_kind: "source-spec".into(),
                    source_id,
                    chunk_id,
                    stage,
                    breadcrumb,
                    signal_name: row.signal_name,
                    direction: row.direction,
                    peer: row.peer,
                    description: row.description,
                });
            }
        }
    }

    let spec_md_path = opts.project_root.join("docs").join("spec.md");
    if spec_md_path.is_file() {
        let body = std::fs::read_to_string(&spec_md_path)?;
        let parsed = parse_spec_md(&body)?;
        rows.extend(spec_md_signal_rows(&parsed));
    }

    // Fill in `row_id` per §3.6.
    let materialized: Vec<MaterializedSignalRow> =
        rows.into_iter().map(MaterializedSignalRow::from).collect();

    let schema = signal_table_rows_schema();
    let n = materialized.len() as u64;
    if materialized.is_empty() {
        write_empty_table(index_root, "signal_table_rows.lance", schema)?;
        return Ok(0);
    }
    let batch = build_signal_rows_batch(schema.clone(), &materialized)?;
    atomic_write_table(index_root, "signal_table_rows.lance", schema, batch)?;
    Ok(n)
}

/// Build the `cross_spec_refs` table.
pub fn build_cross_spec_refs(
    opts: &SpecBuildOpts,
    index_root: &Path,
) -> Result<u64, SpecBuildError> {
    let mut rows: Vec<PendingCrossRef> = Vec::new();
    let ingest_root = opts.project_root.join(".sim-flow").join("spec-ingest");
    if ingest_root.is_dir() {
        for path in walk_references_toml(&ingest_root)? {
            let body = std::fs::read_to_string(&path)?;
            let parsed: ReferencesFile = toml::from_str(&body)
                .map_err(|e| SpecBuildError::Toml(format!("{}: {e}", path.display())))?;
            for r in parsed.references {
                rows.push(PendingCrossRef {
                    source_chunk_id: r.source_chunk_id,
                    peer_id: r.peer_id,
                    peer_chunk_id: r.peer_chunk_id.unwrap_or_default(),
                    reference_text: r.reference_text,
                    referenced_breadcrumbs: r.referenced_breadcrumbs.unwrap_or_default(),
                });
            }
        }
    }

    let schema = cross_spec_refs_schema();
    let n = rows.len() as u64;
    if rows.is_empty() {
        write_empty_table(index_root, "cross_spec_refs.lance", schema)?;
        return Ok(0);
    }
    let batch = build_cross_refs_batch(schema.clone(), &rows)?;
    atomic_write_table(index_root, "cross_spec_refs.lance", schema, batch)?;
    Ok(n)
}

/// Walk `<root>/**/tables/signals/*.toml`.
fn walk_tables_signals(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, out)?;
            } else if ft.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("toml")
                && path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    == Some("signals")
            {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(root, &mut out)?;
    Ok(out)
}

/// Walk `<root>/**/references.toml`.
fn walk_references_toml(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, out)?;
            } else if ft.is_file()
                && path.file_name().and_then(|s| s.to_str()) == Some("references.toml")
            {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(root, &mut out)?;
    Ok(out)
}

fn walk_files(root: &Path, wanted: &[&str]) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn visit(dir: &Path, wanted: &[&str], out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                visit(&path, wanted, out)?;
            } else if ft.is_file()
                && let Some(ext) = path.extension().and_then(|s| s.to_str())
                && wanted.iter().any(|w| ext.eq_ignore_ascii_case(w))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    visit(root, wanted, &mut out)?;
    Ok(out)
}

/// Parse a `chunks/NNN-<slug>.md` file: optional YAML front matter
/// then body. Returns `None` when the file has no front matter (e.g.
/// a stray sibling file that isn't a chunk).
fn read_chunk_md(path: &Path, source_id: &str) -> Result<Option<PendingSpecChunk>, SpecBuildError> {
    let raw = std::fs::read_to_string(path)?;
    let (front_matter, body) = split_front_matter(&raw);
    let Some(fm) = front_matter else {
        return Ok(None);
    };
    let fm: ChunkFrontMatter = serde_yaml_compat_parse(&fm).map_err(SpecBuildError::Toml)?;
    let text_sha256 = sha256_hex(&body);
    let id = if fm.chunk_id.is_empty() {
        format!(
            "{source_id}:{}",
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        )
    } else {
        fm.chunk_id.clone()
    };
    // Phase 9 milestone 9.13: surface the role / domain tags emit.rs
    // writes (Chapter 7 §7.8). Defaults match the schema:
    //   - `spec_md_role`: `"unknown"` when missing or empty.
    //   - `layer`: `"unknown"` when missing or empty.
    //   - `acronyms_referenced`: empty list when absent.
    //   - `*_domain`: `None` (null in lance) when missing or empty.
    let spec_md_role = if fm.spec_md_role.trim().is_empty() {
        "unknown".to_string()
    } else {
        fm.spec_md_role
    };
    let layer = if fm.layer.trim().is_empty() {
        "unknown".to_string()
    } else {
        fm.layer
    };
    let acronyms_referenced = fm.acronyms_referenced;
    let clock_domain = nonempty_string(&fm.clock_domain);
    let power_domain = nonempty_string(&fm.power_domain);
    let reset_domain = nonempty_string(&fm.reset_domain);

    Ok(Some(PendingSpecChunk {
        id,
        source_id: source_id.to_string(),
        breadcrumb: fm.breadcrumb,
        section_heading: fm.section_heading,
        source_page_start: fm.source_page_start.unwrap_or(0),
        source_page_end: fm.source_page_end.unwrap_or(0),
        kind: if fm.kind.is_empty() {
            "prose".into()
        } else {
            fm.kind
        },
        text: body,
        text_sha256,
        contained_signal_tables: fm.contained_signal_tables,
        contained_figures: fm.contained_figures,
        spec_md_role,
        layer,
        acronyms_referenced,
        clock_domain,
        power_domain,
        reset_domain,
    }))
}

/// Promote an empty or whitespace-only string to `None`; otherwise
/// return `Some` with the trimmed contents.
fn nonempty_string(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Lightweight YAML-ish parser used for chunk front matter. The
/// emit-stage writes a strict subset (key + scalar string, lists of
/// strings) so we don't need a full YAML parser dep. Returns the
/// fields we care about; unknown keys are tolerated.
fn serde_yaml_compat_parse(input: &str) -> Result<ChunkFrontMatter, String> {
    let mut out = ChunkFrontMatter::default();
    let mut current_list: Option<String> = None;
    for raw_line in input.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            if let Some(key) = current_list.as_deref() {
                let item = strip_quotes(rest.trim()).to_string();
                match key {
                    "breadcrumb" => out.breadcrumb.push(item),
                    "contained_signal_tables" => out.contained_signal_tables.push(item),
                    "contained_figures" => out.contained_figures.push(item),
                    "acronyms_referenced" => out.acronyms_referenced.push(item),
                    _ => {}
                }
                continue;
            } else {
                return Err(format!("bullet `{rest}` with no active list key"));
            }
        }
        current_list = None;
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("expected `key: value`, got `{line}`"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "chunk_id" => out.chunk_id = strip_quotes(value).to_string(),
            "section_heading" => out.section_heading = strip_quotes(value).to_string(),
            "kind" => out.kind = strip_quotes(value).to_string(),
            "source_page_start" => out.source_page_start = value.parse().ok(),
            "source_page_end" => out.source_page_end = value.parse().ok(),
            "breadcrumb" => {
                current_list = Some("breadcrumb".into());
                if !value.is_empty() {
                    // Inline form: `breadcrumb: ["A", "B"]` is also
                    // supported; parse a flat JSON-array-ish.
                    out.breadcrumb = split_inline_array(value);
                    current_list = None;
                }
            }
            "contained_signal_tables" => {
                current_list = Some("contained_signal_tables".into());
                if !value.is_empty() {
                    out.contained_signal_tables = split_inline_array(value);
                    current_list = None;
                }
            }
            "contained_figures" => {
                current_list = Some("contained_figures".into());
                if !value.is_empty() {
                    out.contained_figures = split_inline_array(value);
                    current_list = None;
                }
            }
            // Phase 9 milestone 9.13: role / domain front-matter.
            "spec_md_role" => out.spec_md_role = strip_quotes(value).to_string(),
            "layer" => out.layer = strip_quotes(value).to_string(),
            "acronyms_referenced" => {
                current_list = Some("acronyms_referenced".into());
                if !value.is_empty() {
                    out.acronyms_referenced = split_inline_array(value);
                    current_list = None;
                }
            }
            "clock_domain" => out.clock_domain = strip_quotes(value).to_string(),
            "power_domain" => out.power_domain = strip_quotes(value).to_string(),
            "reset_domain" => out.reset_domain = strip_quotes(value).to_string(),
            _ => {} // tolerate unknown fields
        }
    }
    Ok(out)
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    s.trim_start_matches('"').trim_end_matches('"')
}

fn split_inline_array(value: &str) -> Vec<String> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.trim().is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(|s| strip_quotes(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a chunk file into `(Option<front_matter>, body)`. We treat
/// the front matter as the YAML block delimited by leading `---` and
/// a closing `---`. Files without a leading `---` return `(None, body)`.
fn split_front_matter(input: &str) -> (Option<String>, String) {
    let trimmed = input.trim_start_matches('\u{feff}');
    if let Some(rest) = trimmed.strip_prefix("---") {
        let rest = rest.trim_start_matches('\n');
        if let Some(end) = rest.find("\n---") {
            let fm = rest[..end].to_string();
            let body = rest[end + 4..].trim_start_matches('\n').to_string();
            return (Some(fm), body);
        }
    }
    (None, trimmed.to_string())
}

#[derive(Debug, Default, Deserialize)]
struct SourceSignalTableFile {
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default)]
    breadcrumb: Vec<String>,
    #[serde(default)]
    rows: Vec<SourceSignalRow>,
}

#[derive(Debug, Default, Deserialize)]
struct SourceSignalRow {
    #[serde(default)]
    signal_name: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    peer: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Default, Deserialize)]
struct ReferencesFile {
    #[serde(default)]
    references: Vec<RawReference>,
}

#[derive(Debug, Default, Deserialize)]
struct RawReference {
    #[serde(default)]
    source_chunk_id: String,
    #[serde(default)]
    peer_id: String,
    #[serde(default)]
    peer_chunk_id: Option<String>,
    #[serde(default)]
    reference_text: String,
    #[serde(default)]
    referenced_breadcrumbs: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct PendingSignalRow {
    source_kind: String,
    source_id: String,
    chunk_id: String,
    stage: String,
    breadcrumb: Vec<String>,
    signal_name: String,
    direction: String,
    peer: String,
    description: String,
}

#[derive(Debug, Clone)]
struct MaterializedSignalRow {
    row_id: String,
    source_kind: String,
    source_id: String,
    chunk_id: String,
    stage: String,
    breadcrumb: Vec<String>,
    signal_name: String,
    direction: String,
    peer: String,
    description: String,
}

impl From<PendingSignalRow> for MaterializedSignalRow {
    fn from(r: PendingSignalRow) -> Self {
        let key = format!(
            "{}|{}|{}|{}",
            r.source_kind, r.source_id, r.stage, r.signal_name
        );
        let row_id = sha256_hex(&key);
        MaterializedSignalRow {
            row_id,
            source_kind: r.source_kind,
            source_id: r.source_id,
            chunk_id: r.chunk_id,
            stage: r.stage,
            breadcrumb: r.breadcrumb,
            signal_name: r.signal_name,
            direction: r.direction,
            peer: r.peer,
            description: r.description,
        }
    }
}

/// Extract `signal_table_rows` rows from each block's signal table.
/// Blocks are stored flat in `SpecMd::blocks`, with `parent` and
/// `sub_blocks` recording the hierarchy. We reconstruct a breadcrumb
/// by walking the `parent` chain (terminated by the literal
/// `(none -- top-level)` sentinel the parser emits for roots).
fn spec_md_signal_rows(spec: &SpecMd) -> Vec<PendingSignalRow> {
    let by_name: std::collections::HashMap<&str, &Block> =
        spec.blocks.iter().map(|b| (b.name.as_str(), b)).collect();
    let mut out = Vec::new();
    for block in &spec.blocks {
        let breadcrumb = breadcrumb_for_block(block, &by_name);
        for row in &block.signals {
            out.push(PendingSignalRow {
                source_kind: "spec-md".into(),
                source_id: "spec.md".into(),
                chunk_id: format!("spec.md#{}", breadcrumb.join("/")),
                stage: block.name.clone(),
                breadcrumb: breadcrumb.clone(),
                signal_name: row.name.clone(),
                direction: row.direction.clone(),
                peer: row.peer.clone(),
                description: row.description.clone(),
            });
        }
    }
    out
}

fn breadcrumb_for_block(
    block: &Block,
    by_name: &std::collections::HashMap<&str, &Block>,
) -> Vec<String> {
    let mut crumb = vec![block.name.clone()];
    let mut current_parent = block.parent.clone();
    let mut guard = 0;
    while !current_parent.is_empty() && current_parent != "(none -- top-level)" && guard < 64 {
        if let Some(parent) = by_name.get(current_parent.as_str()) {
            crumb.push(parent.name.clone());
            current_parent = parent.parent.clone();
        } else {
            break;
        }
        guard += 1;
    }
    crumb.reverse();
    crumb
}

#[derive(Debug, Clone)]
struct PendingCrossRef {
    source_chunk_id: String,
    peer_id: String,
    peer_chunk_id: String,
    reference_text: String,
    referenced_breadcrumbs: Vec<String>,
}

fn build_spec_chunks_batch(
    schema: Arc<arrow_schema::Schema>,
    rows: &[PendingSpecChunk],
    vectors: &[Vec<f32>],
    dimension: usize,
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let n = rows.len();
    let ids = StringArray::from_iter_values(rows.iter().map(|r| r.id.as_str()));
    let source_ids = StringArray::from_iter_values(rows.iter().map(|r| r.source_id.as_str()));
    let breadcrumbs = build_list_array(rows.iter().map(|r| r.breadcrumb.as_slice()))?;
    let headings = StringArray::from_iter_values(rows.iter().map(|r| r.section_heading.as_str()));
    let page_start = UInt32Array::from_iter_values(rows.iter().map(|r| r.source_page_start));
    let page_end = UInt32Array::from_iter_values(rows.iter().map(|r| r.source_page_end));
    let kinds = StringArray::from_iter_values(rows.iter().map(|r| r.kind.as_str()));
    let texts = StringArray::from_iter_values(rows.iter().map(|r| r.text.as_str()));
    let shas = StringArray::from_iter_values(rows.iter().map(|r| r.text_sha256.as_str()));

    let mut flat = Vec::with_capacity(n * dimension);
    for v in vectors {
        if v.len() != dimension {
            return Err(arrow_schema::ArrowError::SchemaError(format!(
                "spec_chunks vector dim {} != schema dim {dimension}",
                v.len()
            )));
        }
        flat.extend_from_slice(v);
    }
    let values = Arc::new(Float32Array::from(flat));
    let field = Arc::new(arrow_schema::Field::new(
        "item",
        arrow_schema::DataType::Float32,
        true,
    ));
    let vector_array = FixedSizeListArray::try_new(field, dimension as i32, values, None)?;

    let signal_tables =
        build_list_array(rows.iter().map(|r| r.contained_signal_tables.as_slice()))?;
    let figures = build_list_array(rows.iter().map(|r| r.contained_figures.as_slice()))?;

    // Phase 9 milestone 9.13: role / domain columns.
    let spec_md_roles = StringArray::from_iter_values(rows.iter().map(|r| r.spec_md_role.as_str()));
    let layers = StringArray::from_iter_values(rows.iter().map(|r| r.layer.as_str()));
    let acronyms = build_list_array(rows.iter().map(|r| r.acronyms_referenced.as_slice()))?;
    // Nullable strings: an `Option<String>::None` becomes a SQL NULL.
    let clock_domains = StringArray::from_iter(rows.iter().map(|r| r.clock_domain.clone()));
    let power_domains = StringArray::from_iter(rows.iter().map(|r| r.power_domain.clone()));
    let reset_domains = StringArray::from_iter(rows.iter().map(|r| r.reset_domain.clone()));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(source_ids),
            Arc::new(breadcrumbs),
            Arc::new(headings),
            Arc::new(page_start),
            Arc::new(page_end),
            Arc::new(kinds),
            Arc::new(texts),
            Arc::new(shas),
            Arc::new(vector_array),
            Arc::new(signal_tables),
            Arc::new(figures),
            Arc::new(spec_md_roles),
            Arc::new(layers),
            Arc::new(acronyms),
            Arc::new(clock_domains),
            Arc::new(power_domains),
            Arc::new(reset_domains),
        ],
    )
}

fn build_signal_rows_batch(
    schema: Arc<arrow_schema::Schema>,
    rows: &[MaterializedSignalRow],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let row_ids = StringArray::from_iter_values(rows.iter().map(|r| r.row_id.as_str()));
    let source_kinds = StringArray::from_iter_values(rows.iter().map(|r| r.source_kind.as_str()));
    let source_ids = StringArray::from_iter_values(rows.iter().map(|r| r.source_id.as_str()));
    let chunk_ids = StringArray::from_iter_values(rows.iter().map(|r| r.chunk_id.as_str()));
    let stages = StringArray::from_iter_values(rows.iter().map(|r| r.stage.as_str()));
    let breadcrumbs = build_list_array(rows.iter().map(|r| r.breadcrumb.as_slice()))?;
    let signal_names = StringArray::from_iter_values(rows.iter().map(|r| r.signal_name.as_str()));
    let directions = StringArray::from_iter_values(rows.iter().map(|r| r.direction.as_str()));
    let peers = StringArray::from_iter_values(rows.iter().map(|r| r.peer.as_str()));
    let descriptions = StringArray::from_iter_values(rows.iter().map(|r| r.description.as_str()));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(row_ids),
            Arc::new(source_kinds),
            Arc::new(source_ids),
            Arc::new(chunk_ids),
            Arc::new(stages),
            Arc::new(breadcrumbs),
            Arc::new(signal_names),
            Arc::new(directions),
            Arc::new(peers),
            Arc::new(descriptions),
        ],
    )
}

fn build_cross_refs_batch(
    schema: Arc<arrow_schema::Schema>,
    rows: &[PendingCrossRef],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let owned_ids: Vec<String> = rows
        .iter()
        .map(|r| {
            sha256_hex(&format!(
                "{}|{}|{}",
                r.source_chunk_id, r.peer_id, r.reference_text
            ))
        })
        .collect();
    let ref_ids = StringArray::from_iter_values(owned_ids.iter().map(|s| s.as_str()));

    let source_chunk_ids =
        StringArray::from_iter_values(rows.iter().map(|r| r.source_chunk_id.as_str()));
    let peer_ids = StringArray::from_iter_values(rows.iter().map(|r| r.peer_id.as_str()));
    let peer_chunk_ids =
        StringArray::from_iter_values(rows.iter().map(|r| r.peer_chunk_id.as_str()));
    let reference_texts =
        StringArray::from_iter_values(rows.iter().map(|r| r.reference_text.as_str()));
    let breadcrumbs = build_list_array(rows.iter().map(|r| r.referenced_breadcrumbs.as_slice()))?;

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ref_ids),
            Arc::new(source_chunk_ids),
            Arc::new(peer_ids),
            Arc::new(peer_chunk_ids),
            Arc::new(reference_texts),
            Arc::new(breadcrumbs),
        ],
    )
}

/// Build a `List<Utf8>` Arrow array from a per-row slice of strings.
fn build_list_array<'a>(
    rows: impl Iterator<Item = &'a [String]>,
) -> Result<ListArray, arrow_schema::ArrowError> {
    let mut builder = ListBuilder::new(StringBuilder::new());
    for row in rows {
        for item in row {
            builder.values().append_value(item);
        }
        builder.append(true);
    }
    Ok(builder.finish())
}

fn atomic_write_table(
    index_root: &Path,
    table_dir_name: &str,
    schema: Arc<arrow_schema::Schema>,
    batch: RecordBatch,
) -> Result<(), SpecBuildError> {
    // lancedb appends `.lance` to the table name on disk. We pass a
    // synthetic `<stem>_tmp` to `create_table`, then atomically
    // rename the resulting `<stem>_tmp.lance/` to the final
    // `<stem>.lance/`.
    let stem = table_dir_name.strip_suffix(".lance").ok_or_else(|| {
        SpecBuildError::Toml(format!("table dir must end with .lance: {table_dir_name}"))
    })?;
    let tmp_table_name = format!("{stem}_tmp");
    let final_dir = index_root.join(table_dir_name);
    let tmp_dir = index_root.join(format!("{tmp_table_name}.lance"));
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }

    let conn_uri = index_root.to_string_lossy().to_string();
    run_async(async move {
        use arrow_array::RecordBatchReader;
        let conn = lancedb::connect(&conn_uri).execute().await?;
        let batches = vec![Ok(batch)];
        let reader: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(batches.into_iter(), schema));
        conn.create_table(&tmp_table_name, reader).execute().await?;
        Ok::<(), lancedb::Error>(())
    })?;

    std::fs::rename(&tmp_dir, &final_dir)?;
    Ok(())
}

fn write_empty_table(
    index_root: &Path,
    table_dir_name: &str,
    schema: Arc<arrow_schema::Schema>,
) -> Result<(), SpecBuildError> {
    // Build an empty batch matching the schema by constructing
    // zero-row arrays for each field. Easiest path: use the schema's
    // fields and an empty `RecordBatch::new_empty`.
    let batch = RecordBatch::new_empty(schema.clone());
    atomic_write_table(index_root, table_dir_name, schema, batch)
}

fn read_ingest_source_sha(manifest_path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(manifest_path).ok()?;
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("source_sha256") {
            let v = rest.trim().trim_start_matches('=').trim();
            let v = v.trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_front_matter_basic() {
        let raw = "---\nchunk_id: \"abc\"\nkind: prose\n---\nbody text\n";
        let (fm, body) = split_front_matter(raw);
        assert!(fm.is_some(), "got {fm:?}");
        assert!(fm.unwrap().contains("chunk_id"));
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn yaml_compat_handles_block_list() {
        let fm = "chunk_id: \"abc\"\nbreadcrumb:\n- \"A\"\n- \"B\"\nkind: prose\n";
        let parsed = serde_yaml_compat_parse(fm).expect("parse");
        assert_eq!(parsed.chunk_id, "abc");
        assert_eq!(parsed.breadcrumb, vec!["A", "B"]);
        assert_eq!(parsed.kind, "prose");
    }

    #[test]
    fn yaml_compat_handles_inline_array() {
        let fm = "breadcrumb: [\"X\", \"Y\"]\n";
        let parsed = serde_yaml_compat_parse(fm).expect("parse");
        assert_eq!(parsed.breadcrumb, vec!["X", "Y"]);
    }

    #[test]
    fn yaml_compat_handles_role_and_layer_keys() {
        // Phase 9 milestone 9.13: the parser must pick up the new
        // chunk front-matter keys emit.rs writes.
        let fm = "\
            chunk_id: \"abc\"\n\
            spec_md_role: \"block:Instruction Fetch (IF)\"\n\
            layer: \"micro\"\n\
            acronyms_referenced: [\"IF\", \"PC\"]\n\
            clock_domain: \"core_clk\"\n\
            power_domain: \"core_pd\"\n\
            reset_domain: \"core_rst\"\n";
        let parsed = serde_yaml_compat_parse(fm).expect("parse");
        assert_eq!(parsed.spec_md_role, "block:Instruction Fetch (IF)");
        assert_eq!(parsed.layer, "micro");
        assert_eq!(parsed.acronyms_referenced, vec!["IF", "PC"]);
        assert_eq!(parsed.clock_domain, "core_clk");
        assert_eq!(parsed.power_domain, "core_pd");
        assert_eq!(parsed.reset_domain, "core_rst");
    }

    #[test]
    fn yaml_compat_handles_block_list_for_acronyms() {
        // Bullet-style lists are the canonical emit form too.
        let fm = "\
            chunk_id: \"abc\"\n\
            acronyms_referenced:\n- \"IF\"\n- \"PC\"\n- \"PD\"\n";
        let parsed = serde_yaml_compat_parse(fm).expect("parse");
        assert_eq!(parsed.acronyms_referenced, vec!["IF", "PC", "PD"]);
    }

    #[test]
    fn read_chunk_md_defaults_role_and_domains() {
        // A chunk file without the new keys must populate the
        // PendingSpecChunk with the documented defaults
        // ("unknown" role + layer, empty acronyms, no domains).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("000-intro.md");
        std::fs::write(
            &path,
            "---\nchunk_id: \"abc\"\nbreadcrumb: [\"A\"]\nkind: prose\n---\nbody text\n",
        )
        .unwrap();
        let chunk = read_chunk_md(&path, "primary")
            .expect("read")
            .expect("front matter present");
        assert_eq!(chunk.spec_md_role, "unknown");
        assert_eq!(chunk.layer, "unknown");
        assert!(chunk.acronyms_referenced.is_empty());
        assert!(chunk.clock_domain.is_none());
        assert!(chunk.power_domain.is_none());
        assert!(chunk.reset_domain.is_none());
    }

    #[test]
    fn read_chunk_md_surfaces_role_and_domain_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("000-block.md");
        std::fs::write(
            &path,
            "---\n\
             chunk_id: \"abc\"\n\
             spec_md_role: \"block:IF\"\n\
             layer: \"micro\"\n\
             acronyms_referenced: [\"IF\", \"PC\"]\n\
             clock_domain: \"core_clk\"\n\
             ---\n\
             body\n",
        )
        .unwrap();
        let chunk = read_chunk_md(&path, "primary").expect("read").expect("fm");
        assert_eq!(chunk.spec_md_role, "block:IF");
        assert_eq!(chunk.layer, "micro");
        assert_eq!(chunk.acronyms_referenced, vec!["IF", "PC"]);
        assert_eq!(chunk.clock_domain.as_deref(), Some("core_clk"));
        // Unset domains remain None even when role + layer are set.
        assert!(chunk.power_domain.is_none());
        assert!(chunk.reset_domain.is_none());
    }
}
