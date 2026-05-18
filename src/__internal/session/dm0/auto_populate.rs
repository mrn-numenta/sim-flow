//! Source-driven auto-populate for DM0.
//!
//! Reads the ingest corpus at
//! `<project>/.sim-flow/spec-ingest/` and seeds a [`SpecMd`] struct
//! with metadata, parameters, encodings, errors, FSMs, blocks,
//! figures, anchors, and TBDs. The agent's LLM turn picks up from
//! the populated draft and fills the prose subsections.
//!
//! Each `populate_*` function is idempotent: calling it on an
//! already-populated `SpecMd` is a no-op (or strictly appends new
//! rows). The whole module is owned by Phase 6 Stream A.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::__internal::session::spec_md::types::{
    QuantitativeRow, SourceDocument, SourceDocumentRole, SpecMd,
};
use crate::{Error, Result};

/// Aggregate report returned by [`run`]. Counts let the gate decide
/// whether the agent's downstream prompt has anything left to do.
#[derive(Debug, Clone, Default)]
pub struct AutoPopulateReport {
    pub blocks: usize,
    pub parameters: usize,
    pub encodings: usize,
    pub errors: usize,
    pub fsms: usize,
    pub figures: usize,
    pub anchors: usize,
    pub open_questions: usize,
}

/// Run every `populate_*` step in order and return an aggregate
/// report. Called from [`super::run_dm0_work`] when
/// [`super::detect_mode`] returns [`super::Dm0Mode::SourceDriven`].
#[allow(dead_code)]
pub fn run(_project_dir: &Path, _spec: &mut SpecMd) -> Result<AutoPopulateReport> {
    todo!("Phase 6 milestones 6.3–6.6 — orchestrate the populate_* helpers")
}

// ---------------------------------------------------------------------------
// 6.3 — Metadata + Assumptions
// ---------------------------------------------------------------------------

/// Fill `SpecMd.metadata.source_documents` from the ingest manifest's
/// `source_path` (primary entry) plus every `[[peers]]` block. Other
/// metadata fields (design_name, version, authors, status, dates)
/// are NOT touched — those live in source-spec prose the agent must
/// extract, or come from user dictation in no-source mode.
///
/// Idempotent: re-running on a populated `source_documents` appends
/// nothing new (matched by `role + path`).
pub fn populate_metadata(manifest_path: &Path, spec: &mut SpecMd) -> Result<()> {
    let body = fs::read_to_string(manifest_path).map_err(|source| Error::Io {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    let raw: RawManifest = toml::from_str(&body).map_err(|source| Error::TomlParse {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    if !raw.source_path.is_empty() {
        let entry = SourceDocument {
            role: SourceDocumentRole::Primary,
            peer_id: None,
            path: raw.source_path.clone(),
        };
        push_unique_source_document(&mut spec.metadata.source_documents, entry);
    }
    for peer in &raw.peers {
        let entry = SourceDocument {
            role: SourceDocumentRole::Peer,
            peer_id: Some(peer.id.clone()),
            path: peer.source_path.clone(),
        };
        push_unique_source_document(&mut spec.metadata.source_documents, entry);
    }
    Ok(())
}

fn push_unique_source_document(out: &mut Vec<SourceDocument>, entry: SourceDocument) {
    let already = out
        .iter()
        .any(|d| d.role == entry.role && d.path == entry.path && d.peer_id == entry.peer_id);
    if !already {
        out.push(entry);
    }
}

/// Heuristic scan for clock-frequency-like and technology-node-like
/// facts in the corpus. Inspects:
///
/// - Every `<corpus>/tables/parameters/*.toml` row (name / comment /
///   default) for clock / frequency / tech-node keywords.
/// - Every `<corpus>/chunks/*.md` body for the same patterns when
///   parameter tables yielded nothing.
///
/// Appends a `QuantitativeRow` for each detected fact, deduped by
/// `(constraint, value)`. The anchor is the `primary:p<N>` page form
/// derived from the table or chunk source page range.
///
/// This is intentionally best-effort: the agent's LLM completion step
/// is responsible for verifying / refining these rows.
pub fn populate_assumptions(corpus_root: &Path, spec: &mut SpecMd) -> Result<()> {
    let clock_re = regex::Regex::new(r"(?i)(\d{1,4}(?:\.\d+)?)\s*(GHz|MHz|kHz)").unwrap();
    let tech_re = regex::Regex::new(r"(?i)\b(\d{1,3}\s*nm)\b").unwrap();

    let mut clock_hit: Option<(String, String)> = None; // (value, anchor)
    let mut tech_hit: Option<(String, String)> = None;

    // Scan parameter tables first.
    let params_dir = corpus_root.join("tables").join("parameters");
    for path in list_toml_files(&params_dir) {
        let body = match fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(table) = toml::from_str::<RawParameterTable>(&body) else {
            continue;
        };
        let anchor = page_anchor("primary", &table.source_page_range);
        for row in &table.rows {
            let haystack = format!("{} {} {}", row.name, row.default, row.comment);
            if clock_hit.is_none()
                && let Some(m) = clock_re.captures(&haystack)
            {
                clock_hit = Some((format!("{} {}", &m[1], &m[2]), anchor.clone()));
            }
            if tech_hit.is_none()
                && let Some(m) = tech_re.captures(&haystack)
            {
                tech_hit = Some((m[1].to_string(), anchor.clone()));
            }
        }
    }

    // Fall back to chunk bodies.
    if clock_hit.is_none() || tech_hit.is_none() {
        let chunks_dir = corpus_root.join("chunks");
        for path in list_md_files(&chunks_dir) {
            let body = match fs::read_to_string(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let frontmatter = parse_chunk_frontmatter(&body).unwrap_or_default();
            let anchor = if let Some((start, end)) = frontmatter.source_page_range {
                page_anchor("primary", &[start, end])
            } else {
                "primary".to_string()
            };
            if clock_hit.is_none()
                && let Some(m) = clock_re.captures(&body)
            {
                clock_hit = Some((format!("{} {}", &m[1], &m[2]), anchor.clone()));
            }
            if tech_hit.is_none()
                && let Some(m) = tech_re.captures(&body)
            {
                tech_hit = Some((m[1].to_string(), anchor.clone()));
            }
            if clock_hit.is_some() && tech_hit.is_some() {
                break;
            }
        }
    }

    if let Some((value, anchor)) = tech_hit {
        push_unique_quant_row(
            &mut spec.assumptions.quantitative,
            QuantitativeRow {
                constraint: "Technology node".into(),
                value,
                source_anchor: anchor,
            },
        );
    }
    if let Some((value, anchor)) = clock_hit {
        push_unique_quant_row(
            &mut spec.assumptions.quantitative,
            QuantitativeRow {
                constraint: "Clock frequency".into(),
                value,
                source_anchor: anchor,
            },
        );
    }
    Ok(())
}

fn push_unique_quant_row(out: &mut Vec<QuantitativeRow>, row: QuantitativeRow) {
    let already = out
        .iter()
        .any(|r| r.constraint.eq_ignore_ascii_case(&row.constraint));
    if !already {
        out.push(row);
    }
}

#[allow(dead_code)]
pub fn populate_parameters(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.4 — read primary/tables/parameters/*.toml → SpecMd.parameters")
}

#[allow(dead_code)]
pub fn populate_encodings(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.4 — read primary/tables/encodings/*.toml → SpecMd.encodings")
}

#[allow(dead_code)]
pub fn populate_errors(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.4 — read primary/tables/errors/*.toml → SpecMd.errors")
}

#[allow(dead_code)]
pub fn populate_fsms(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.4 — read primary/tables/state_machines/*.toml → SpecMd.fsms")
}

#[allow(dead_code)]
pub fn populate_blocks(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.5 — one block per primary/tables/signals/NNN-<stage>.toml")
}

#[allow(dead_code)]
pub fn populate_figures(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.6 — one FigureEntry per figures/page-NNN.png")
}

#[allow(dead_code)]
pub fn populate_anchors(_spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.6 — walk populated sections and build Source-Spec Anchors index")
}

#[allow(dead_code)]
pub fn populate_open_questions_from_tbds(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<usize> {
    todo!("Phase 6 milestone 6.6 — turn primary/tbds.toml entries into OpenQuestions")
}

// ---------------------------------------------------------------------------
// Path / parsing helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn manifest_path(project_dir: &Path) -> PathBuf {
    super::manifest_path(project_dir)
}

#[allow(dead_code)]
fn corpus_root(project_dir: &Path) -> PathBuf {
    project_dir
        .join(".sim-flow")
        .join("spec-ingest")
        .join("primary")
}

fn list_toml_files(dir: &Path) -> Vec<PathBuf> {
    list_files_with_extension(dir, "toml")
}

fn list_md_files(dir: &Path) -> Vec<PathBuf> {
    list_files_with_extension(dir, "md")
}

fn list_files_with_extension(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Build a `<source>:p<N>` or `<source>:p<N>-<M>` anchor from a
/// 2-element page-range array. Returns just `<source>` if the range
/// is invalid (zero / inverted).
fn page_anchor(source: &str, page_range: &[u32]) -> String {
    if page_range.len() < 2 {
        return source.to_string();
    }
    let start = page_range[0];
    let end = page_range[1];
    if start == 0 {
        return source.to_string();
    }
    if start == end {
        format!("{source}:p{start}")
    } else if end > start {
        format!("{source}:p{start}-{end}")
    } else {
        format!("{source}:p{start}")
    }
}

#[derive(Debug, Default)]
struct ChunkFrontmatter {
    source_page_range: Option<(u32, u32)>,
}

fn parse_chunk_frontmatter(body: &str) -> Option<ChunkFrontmatter> {
    let stripped = body.strip_prefix("---\n")?;
    let end = stripped.find("\n---\n")?;
    let fm = &stripped[..end];
    let mut out = ChunkFrontmatter::default();
    for line in fm.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("source_page_range:") {
            // Expected shape: `[start, end]`.
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
                if parts.len() == 2
                    && let (Ok(a), Ok(b)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                {
                    out.source_page_range = Some((a, b));
                }
            }
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Raw on-disk shapes — match Phase 2's emit stage byte-for-byte.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct RawManifest {
    #[serde(default)]
    source_path: String,
    #[serde(default)]
    peers: Vec<RawManifestPeer>,
}

#[derive(Debug, Deserialize)]
struct RawManifestPeer {
    id: String,
    source_path: String,
}

#[derive(Debug, Deserialize)]
struct RawParameterTable {
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawParameterRow>,
}

#[derive(Debug, Deserialize)]
struct RawParameterRow {
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    kind: Option<String>,
    #[serde(default)]
    default: String,
    #[serde(default)]
    comment: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn make_project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn metadata_pulls_primary_and_peers() {
        let tmp = make_project();
        let mp = manifest_path(tmp.path());
        write(
            &mp,
            "schema_version = 1\n\
             source_kind = \"pdf\"\n\
             source_path = \"docs/main.pdf\"\n\
             \n\
             [[peers]]\n\
             id = \"tm\"\n\
             source_path = \"docs/tm.pdf\"\n\
             source_sha256 = \"\"\n\
             reason = \"\"\n\
             \n\
             [[peers]]\n\
             id = \"snn\"\n\
             source_path = \"docs/snn.pdf\"\n\
             source_sha256 = \"\"\n\
             reason = \"\"\n",
        );
        let mut spec = SpecMd::default();
        populate_metadata(&mp, &mut spec).unwrap();
        assert_eq!(spec.metadata.source_documents.len(), 3);
        assert_eq!(
            spec.metadata.source_documents[0].role,
            SourceDocumentRole::Primary
        );
        assert_eq!(spec.metadata.source_documents[0].path, "docs/main.pdf");
        assert_eq!(
            spec.metadata.source_documents[1].role,
            SourceDocumentRole::Peer
        );
        assert_eq!(
            spec.metadata.source_documents[1].peer_id.as_deref(),
            Some("tm")
        );
        // Re-running is a no-op.
        populate_metadata(&mp, &mut spec).unwrap();
        assert_eq!(spec.metadata.source_documents.len(), 3);
    }

    #[test]
    fn metadata_handles_no_source_manifest() {
        let tmp = make_project();
        let mp = manifest_path(tmp.path());
        write(
            &mp,
            "schema_version = 1\nsource_kind = \"none\"\nsource_path = \"\"\n",
        );
        let mut spec = SpecMd::default();
        populate_metadata(&mp, &mut spec).unwrap();
        assert!(spec.metadata.source_documents.is_empty());
    }

    #[test]
    fn assumptions_extract_clock_and_tech_from_parameter_table() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus
            .join("tables")
            .join("parameters")
            .join("000-clock.toml");
        write(
            &p,
            "schema_version = 1\n\
             table_kind = \"parameter_table\"\n\
             source_chunk_id = \"abc\"\n\
             source_page_range = [3, 3]\n\
             group = \"clocks\"\n\
             \n\
             [[rows]]\n\
             name = \"core_freq\"\n\
             default = \"1 GHz\"\n\
             comment = \"target core clock at 7nm\"\n",
        );
        let mut spec = SpecMd::default();
        populate_assumptions(&corpus, &mut spec).unwrap();
        let q = &spec.assumptions.quantitative;
        assert!(
            q.iter()
                .any(|r| r.constraint == "Clock frequency" && r.value == "1 GHz"),
            "rows: {q:?}"
        );
        assert!(
            q.iter()
                .any(|r| r.constraint == "Technology node" && r.value == "7nm"),
            "rows: {q:?}"
        );
        // Idempotency.
        populate_assumptions(&corpus, &mut spec).unwrap();
        let q = &spec.assumptions.quantitative;
        assert_eq!(
            q.iter()
                .filter(|r| r.constraint == "Clock frequency")
                .count(),
            1
        );
    }

    #[test]
    fn assumptions_fall_back_to_chunk_body() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus.join("chunks").join("000-overview.md");
        write(
            &p,
            "---\n\
             chunk_id: \"abc\"\n\
             breadcrumb: []\n\
             section_heading: \"Overview\"\n\
             source_page_range: [3, 4]\n\
             kind: \"prose\"\n\
             contained_signal_tables: []\n\
             contained_figures: []\n\
             contained_table_refs: []\n\
             tbd_count: 0\n\
             ---\n\
             \n\
             The design runs at 500 MHz on a 14nm node.\n",
        );
        let mut spec = SpecMd::default();
        populate_assumptions(&corpus, &mut spec).unwrap();
        let q = &spec.assumptions.quantitative;
        assert!(
            q.iter()
                .any(|r| r.constraint == "Clock frequency" && r.value == "500 MHz")
        );
        assert!(
            q.iter()
                .any(|r| r.constraint == "Technology node" && r.value == "14nm")
        );
        // The anchor should reflect the chunk's page range.
        let row = q
            .iter()
            .find(|r| r.constraint == "Clock frequency")
            .unwrap();
        assert_eq!(row.source_anchor, "primary:p3-4");
    }
}
