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

use crate::__internal::session::spec_ingest::format::{
    FormatJson, Layer as FormatLayer, SpecMdRole, TableKind, TableTarget,
};
use crate::__internal::session::spec_md::types::{
    AnchorIndexEntry, Block, BlockSignalRow, ClockDomain, Csr, CsrField, Encoding, EncodingValue,
    ErrorEntry, FigureEntry, FsmState, FsmTransition, GlossaryEntry, Layer as SpecLayer,
    NumericalConvention, OpenQuestion, Parameter, PmuEvent, PowerDomain, PrivilegeLevel,
    QuantitativeRow, ResetDomain, SignalRole, SourceDocument, SourceDocumentRole, SpecMd,
    StateMachine,
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
    // ---- Phase 9 milestone 9.12 additions ----
    pub csrs: usize,
    pub glossary: usize,
    pub clock_domains: usize,
    pub power_domains: usize,
    pub reset_domains: usize,
    pub security_boundaries: usize,
    pub numerical_conventions: usize,
    pub performance_counters: usize,
}

/// Run every `populate_*` step in order and return an aggregate
/// report. Called from [`super::run_dm0_work`] when
/// [`super::detect_mode`] returns [`super::Dm0Mode::SourceDriven`].
///
/// Order is load-bearing: `populate_anchors` walks already-populated
/// sections, so it runs after every other populate step that emits a
/// `source_anchor`. TBDs run last because they only consume the
/// ingest corpus (no spec-state dependency) and are the least likely
/// to surface useful information until everything else is in place.
///
/// Thin wrapper over [`run_with_format`] with no descriptor — the
/// heuristic-only path (Phase 6 behaviour) for callers that haven't
/// loaded a `format.json` yet.
pub fn run(project_dir: &Path, spec: &mut SpecMd) -> Result<AutoPopulateReport> {
    run_with_format(project_dir, spec, None)
}

/// Format-aware variant of [`run`]. When `format` is `Some`, populate
/// steps that have a format-driven mode (blocks today, more in later
/// milestones) consume the descriptor instead of inferring from
/// filenames. When `format` is `None`, the behaviour is identical to
/// [`run`].
///
/// The Phase 9 milestone 9.12 additions (CSRs, glossary, domains,
/// conventions, PMU events, security boundaries) read shards under
/// `<corpus>/tables/<name>/` if the format-driven classify path
/// emitted them; each populate gracefully returns 0 when the
/// directory is absent so the legacy / markdown path still works.
pub fn run_with_format(
    project_dir: &Path,
    spec: &mut SpecMd,
    format: Option<&FormatJson>,
) -> Result<AutoPopulateReport> {
    let manifest = manifest_path(project_dir);
    let corpus = corpus_root(project_dir);
    populate_metadata(&manifest, spec)?;
    populate_assumptions(&corpus, spec)?;
    let parameters = populate_parameters(&corpus, spec)?;
    let encodings = populate_encodings(&corpus, spec)?;
    let errors = populate_errors(&corpus, spec)?;
    let fsms = populate_fsms(&corpus, spec)?;
    let blocks = populate_blocks_with_format(&corpus, spec, format)?;
    let figures = populate_figures(&corpus, spec)?;
    let csrs = populate_csrs(&corpus, spec)?;
    let glossary = populate_glossary(&corpus, spec, format)?;
    let clock_domains = populate_clock_domains(&corpus, spec)?;
    let power_domains = populate_power_domains(&corpus, spec)?;
    let reset_domains = populate_reset_domains(&corpus, spec)?;
    let security_boundaries = populate_security_boundaries(&corpus, spec)?;
    let numerical_conventions = populate_numerical_conventions(&corpus, spec)?;
    let performance_counters = populate_performance_counters(&corpus, spec)?;
    let anchors = populate_anchors(spec)?;
    let open_questions_tbds = populate_open_questions_from_tbds(&corpus, spec)?;
    let open_questions_unknown = populate_open_questions_from_unknown_tables(&corpus, spec)?;
    let open_questions = open_questions_tbds + open_questions_unknown;
    Ok(AutoPopulateReport {
        blocks,
        parameters,
        encodings,
        errors,
        fsms,
        figures,
        anchors,
        open_questions,
        csrs,
        glossary,
        clock_domains,
        power_domains,
        reset_domains,
        security_boundaries,
        numerical_conventions,
        performance_counters,
    })
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

// ---------------------------------------------------------------------------
// 6.4 — Parameters / Encodings / Errors / FSMs
// ---------------------------------------------------------------------------

/// Append one `Parameter` per row across every
/// `<corpus>/tables/parameters/*.toml` shard. Idempotent on
/// `(name, source_anchor)`.
pub fn populate_parameters(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("parameters");
    let mut appended = 0usize;
    for path in list_toml_files(&dir) {
        let body = read_required(&path)?;
        let table: RawParameterTable = parse_toml(&path, &body)?;
        let anchor = page_anchor("primary", &table.source_page_range);
        for row in table.rows {
            let p = Parameter {
                name: row.name.clone(),
                ty: row.kind.unwrap_or_default(),
                default: row.default.clone(),
                valid_range: String::new(),
                behavioral_impact: row.comment.clone(),
                source_anchor: anchor.clone(),
            };
            if !spec
                .parameters
                .iter()
                .any(|x| x.name == p.name && x.source_anchor == p.source_anchor)
            {
                spec.parameters.push(p);
                appended += 1;
            }
        }
    }
    Ok(appended)
}

/// Append one `Encoding` per
/// `<corpus>/tables/encodings/*.toml`. Each encoding's `values[]`
/// captures the rows of the shard. Idempotent on `(field,
/// source_anchor)`.
pub fn populate_encodings(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("encodings");
    let mut appended = 0usize;
    for path in list_toml_files(&dir) {
        let body = read_required(&path)?;
        let table: RawEncodingTable = parse_toml(&path, &body)?;
        let anchor = page_anchor("primary", &table.source_page_range);
        let already = spec
            .encodings
            .iter()
            .any(|e| e.field == table.field && e.source_anchor == anchor);
        if already {
            continue;
        }
        let values = table
            .rows
            .into_iter()
            .map(|r| EncodingValue {
                value: r.value,
                name: r.name,
                abbreviation: r.abbreviation,
            })
            .collect();
        spec.encodings.push(Encoding {
            field: table.field,
            bit_width: table.bit_width.map(|b| b.to_string()).unwrap_or_default(),
            source_anchor: anchor,
            values,
            reserved: String::new(),
        });
        appended += 1;
    }
    Ok(appended)
}

/// Append one `ErrorEntry` per row across every
/// `<corpus>/tables/errors/*.toml` shard. Idempotent on
/// `(error_type, source_anchor)`.
pub fn populate_errors(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("errors");
    let mut appended = 0usize;
    for path in list_toml_files(&dir) {
        let body = read_required(&path)?;
        let table: RawErrorTable = parse_toml(&path, &body)?;
        let anchor = page_anchor("primary", &table.source_page_range);
        for row in table.rows {
            let entry = ErrorEntry {
                error_type: row.error_type,
                detecting_component: row.detecting_component,
                detection_behavior: row.detecting_behavior,
                bus_response: row.bus_response,
                master_behavior: row.master_behavior,
                software_response: row.software_response,
                source_anchor: anchor.clone(),
            };
            if !spec
                .error_handling
                .iter()
                .any(|e| e.error_type == entry.error_type && e.source_anchor == entry.source_anchor)
            {
                spec.error_handling.push(entry);
                appended += 1;
            }
        }
    }
    Ok(appended)
}

/// Append one `StateMachine` per `<corpus>/tables/fsms/*.toml`.
///
/// The emit stage writes FSM shards under `tables/fsms/` (not
/// `tables/state_machines/`); the plan's wording is a typo. Each
/// shard contains a single FSM with its `name`, optional
/// `reset_state`, and a `[[transitions]]` list. The shard does not
/// declare per-state descriptions, so we derive the FSM's
/// `states[]` by unioning the `from` and `to` cells of the
/// transitions and leaving descriptions empty for the agent to fill.
///
/// Idempotent on `(name, source_anchor)`.
pub fn populate_fsms(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("fsms");
    let mut appended = 0usize;
    for path in list_toml_files(&dir) {
        let body = read_required(&path)?;
        let table: RawFsmTable = parse_toml(&path, &body)?;
        let anchor = page_anchor("primary", &table.source_page_range);
        let already = spec
            .state_machines
            .iter()
            .any(|s| s.name == table.name && s.source_anchor == anchor);
        if already {
            continue;
        }
        let transitions: Vec<FsmTransition> = table
            .transitions
            .iter()
            .map(|t| FsmTransition {
                from: t.from.clone(),
                input: t.input.clone(),
                to: t.to.clone(),
                output: t.output.clone(),
            })
            .collect();
        let mut state_names: Vec<String> = Vec::new();
        for t in &transitions {
            if !state_names.contains(&t.from) {
                state_names.push(t.from.clone());
            }
            if !state_names.contains(&t.to) {
                state_names.push(t.to.clone());
            }
        }
        let states: Vec<FsmState> = state_names
            .into_iter()
            .map(|n| FsmState {
                name: n,
                description: String::new(),
            })
            .collect();
        spec.state_machines.push(StateMachine {
            name: table.name,
            reset_state: table.reset_state.unwrap_or_default(),
            source_anchor: anchor,
            states,
            transitions,
        });
        appended += 1;
    }
    Ok(appended)
}

// ---------------------------------------------------------------------------
// 6.5 — Blocks
// ---------------------------------------------------------------------------

/// Emit one `Block` per `<corpus>/tables/signals/NNN-<stage>.toml`.
/// Heuristic / no-format mode: each block's `name` is the table's
/// `stage`, the parent is the top-level sentinel `(none --
/// top-level)` (the agent refines later after reading enough
/// source-spec context to infer hierarchy), and `signals[]`
/// populates from the shard's `[[rows]]`. The shard's source page
/// range becomes a single `primary:p<N>` anchor on the block.
///
/// Idempotent on `(name, source_anchor)`.
pub fn populate_blocks(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    populate_blocks_with_format(corpus_root, spec, None)
}

/// Format-aware variant. When `format` is `Some`, the block name is
/// taken from `format.json::tables[i].spec_md_target.block_name`
/// rather than the shard's heading-derived `stage`, and
/// `Block.layer` is set from the matching `section_role.layer` when
/// available. Per-signal `BlockSignalRow.role` is inferred from the
/// `column_map` canonical (`role`) when present, otherwise from
/// naming-suffix heuristics on the signal name.
///
/// When `format` is `None`, falls back to the heuristic behaviour
/// from [`populate_blocks`].
///
/// Shards in `<corpus>/tables/signals/` are still matched by file
/// listing order against descriptor entries whose `kind` is
/// `SignalTable` and whose `spec_md_target` is `BlockSignals`. If a
/// shard cannot be matched against a descriptor entry, it falls back
/// to the heuristic behaviour for that shard.
pub fn populate_blocks_with_format(
    corpus_root: &Path,
    spec: &mut SpecMd,
    format: Option<&FormatJson>,
) -> Result<usize> {
    let dir = corpus_root.join("tables").join("signals");
    let mut appended = 0usize;

    // Pre-build a list of (block_name, layer, role_canonical_present)
    // from the descriptor in declaration order. The classify.rs
    // format-driven path emits signal-table shards in the same order
    // as the descriptor's `TableEntry`s, so positional matching is
    // sufficient.
    let descriptor_blocks: Vec<DescriptorBlockHint> = match format {
        Some(fmt) => collect_block_hints(fmt),
        None => Vec::new(),
    };

    let shard_paths = list_toml_files(&dir);
    for (idx, path) in shard_paths.iter().enumerate() {
        let body = read_required(path)?;
        let table: RawSignalTable = parse_toml(path, &body)?;
        let anchor = page_anchor("primary", &table.source_page_range);
        let hint = descriptor_blocks.get(idx);
        // Resolve canonical block name + layer from the descriptor
        // when present; otherwise fall back to the shard's `stage`
        // and Unknown layer.
        let block_name = hint
            .map(|h| h.block_name.clone())
            .unwrap_or_else(|| table.stage.clone());
        let layer = hint.map(|h| h.layer).unwrap_or(SpecLayer::Unknown);
        let role_from_column = hint.is_some_and(|h| h.has_role_column);

        let already = spec
            .blocks
            .iter()
            .any(|b| b.name == block_name && b.source_anchors.iter().any(|a| a == &anchor));
        if already {
            continue;
        }
        let signals: Vec<BlockSignalRow> = table
            .rows
            .into_iter()
            .map(|r| {
                let role = if role_from_column {
                    // Descriptor declares a `role` column. The
                    // classify.rs path is responsible for projecting
                    // it into the row; here we use the heuristic
                    // suffix fallback because the legacy shard
                    // shape has no role column. When wired into the
                    // format-driven emit path this becomes
                    // `r.role` directly.
                    infer_role_from_name(&r.name)
                } else {
                    infer_role_from_name(&r.name)
                };
                BlockSignalRow {
                    name: r.name,
                    direction: r.direction,
                    peer: r.peer,
                    description: r.description,
                    role,
                }
            })
            .collect();
        spec.blocks.push(Block {
            name: block_name,
            role: String::new(),
            parent: "(none -- top-level)".into(),
            clock_domain: String::new(),
            power_domain: String::new(),
            reset_domain: String::new(),
            layer,
            parameterized_by: Vec::new(),
            signals,
            state: Vec::new(),
            behavior_summary: String::new(),
            source_anchors: vec![anchor],
            figures: Vec::new(),
            sub_blocks: Vec::new(),
        });
        appended += 1;
    }
    Ok(appended)
}

/// One descriptor entry resolved to the (block_name, layer,
/// role-column-present) tuple `populate_blocks_with_format` needs.
struct DescriptorBlockHint {
    block_name: String,
    layer: SpecLayer,
    has_role_column: bool,
}

fn collect_block_hints(format: &FormatJson) -> Vec<DescriptorBlockHint> {
    let mut out = Vec::new();
    for table in &format.tables {
        if table.kind != TableKind::SignalTable {
            continue;
        }
        let TableTarget::BlockSignals { block_name } = &table.spec_md_target else {
            continue;
        };
        let layer = layer_for_block(format, block_name);
        let has_role_column = table
            .column_map
            .iter()
            .any(|c| c.canonical.eq_ignore_ascii_case("role"));
        out.push(DescriptorBlockHint {
            block_name: block_name.clone(),
            layer,
            has_role_column,
        });
    }
    out
}

/// Look up the `section_roles` entry that owns this block (matching
/// `SpecMdRole::Block { block_name }`) and map its `Layer` into the
/// spec_md type. Returns `Unknown` when no matching role exists.
fn layer_for_block(format: &FormatJson, block_name: &str) -> SpecLayer {
    for role in &format.section_roles {
        if let SpecMdRole::Block { block_name: bn } = &role.spec_md_role
            && bn == block_name
        {
            return map_layer(role.layer);
        }
    }
    SpecLayer::Unknown
}

fn map_layer(layer: FormatLayer) -> SpecLayer {
    match layer {
        FormatLayer::Architectural => SpecLayer::Architectural,
        FormatLayer::Micro => SpecLayer::Micro,
        FormatLayer::Mixed => SpecLayer::Mixed,
        FormatLayer::Unknown => SpecLayer::Unknown,
    }
}

/// Heuristic: classify a signal as control / data / status from
/// common naming-suffix conventions. Anything else collapses to
/// `Unknown`. Matches the classify.rs naming-pattern table in
/// architecture §7.7.
fn infer_role_from_name(name: &str) -> SignalRole {
    let lower = name.to_ascii_lowercase();
    // Strip a leading direction prefix (`if_`, `id_`, etc.) before
    // checking the suffix.
    let tail = lower.rsplit_once('_').map(|(_, t)| t).unwrap_or(&lower);
    match tail {
        "en" | "enable" | "valid" | "ready" | "req" | "ack" | "go" => SignalRole::Control,
        "status" | "busy" | "done" | "err" | "error" => SignalRole::Status,
        _ => SignalRole::Unknown,
    }
}

// ---------------------------------------------------------------------------
// 6.6 — Figures / Anchors / Open Questions
// ---------------------------------------------------------------------------

/// Emit one `FigureEntry` per `<corpus>/figures/page-NNN.png`. Source
/// page is the parsed `NNN`; raster is the relative path
/// `figures/page-NNN.png`; caption is intentionally empty for the
/// agent or a future vision-captioning pass to fill.
///
/// Idempotent on `(name, raster)`.
pub fn populate_figures(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("figures");
    let mut entries: Vec<FigureEntry> = Vec::new();
    let Ok(read) = fs::read_dir(&dir) else {
        return Ok(0);
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        let stem = match p.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let Some(page) = parse_page_filename(stem) else {
            continue;
        };
        let raster = format!(
            "figures/{}",
            p.file_name().and_then(|s| s.to_str()).unwrap_or("")
        );
        entries.push(FigureEntry {
            name: format!("page-{page:03}"),
            source_page: page.to_string(),
            raster,
            role: String::new(),
            referenced_blocks: Vec::new(),
            caption: String::new(),
            elements: Vec::new(),
        });
    }
    entries.sort_by(|a, b| a.raster.cmp(&b.raster));
    let mut appended = 0usize;
    for e in entries {
        let already = spec
            .figures
            .iter()
            .any(|f| f.name == e.name && f.raster == e.raster);
        if !already {
            spec.figures.push(e);
            appended += 1;
        }
    }
    Ok(appended)
}

// ---------------------------------------------------------------------------
// 9.12 — CSRs / Glossary / Domains / Conventions / PMU events
// ---------------------------------------------------------------------------

/// Append one `Csr` per row across every
/// `<corpus>/tables/csrs/*.toml` shard, then stitch each
/// `<corpus>/tables/csr_fields/*.toml` row onto the matching `Csr`
/// by `csr_name` (the field shard's parent CSR).
///
/// Idempotent on `(csr.name, csr.source_anchor)` for the CSR header
/// rows and on `(csr_name, field.bits, field.name)` for the field
/// rows. Returns the count of newly-appended CSRs.
pub fn populate_csrs(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let csrs_dir = corpus_root.join("tables").join("csrs");
    let fields_dir = corpus_root.join("tables").join("csr_fields");
    let mut appended = 0usize;
    let mut any_seen = false;
    let mut any_parsed = false;

    for path in list_toml_files(&csrs_dir) {
        any_seen = true;
        let body = read_required(&path)?;
        let table: RawCsrTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        any_parsed = true;
        let anchor = page_anchor("primary", &table.source_page_range);
        for row in table.rows {
            let csr = Csr {
                address: row.address.clone(),
                name: row.name.clone(),
                access: row.access.clone(),
                reset_value: row.reset_value.clone(),
                required_privilege: row.required_privilege.clone(),
                description: row.description.clone(),
                fields: Vec::new(),
                source_anchor: anchor.clone(),
            };
            if !spec
                .csrs
                .iter()
                .any(|c| c.name == csr.name && c.source_anchor == csr.source_anchor)
            {
                spec.csrs.push(csr);
                appended += 1;
            }
        }
    }
    if any_seen && !any_parsed {
        eprintln!(
            "auto_populate: populate_csrs: {} exists but no rows parsed cleanly",
            csrs_dir.display()
        );
    }

    // Stitch field shards onto matching CSRs.
    let mut fields_seen = false;
    let mut fields_parsed = false;
    for path in list_toml_files(&fields_dir) {
        fields_seen = true;
        let body = read_required(&path)?;
        let table: RawCsrFieldTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        fields_parsed = true;
        let csr_name = table.csr_name.clone();
        if csr_name.is_empty() {
            continue;
        }
        // Locate parent CSR.
        let Some(csr) = spec.csrs.iter_mut().find(|c| c.name == csr_name) else {
            continue;
        };
        for row in table.rows {
            let already = csr
                .fields
                .iter()
                .any(|f| f.bits == row.bits && f.name == row.name);
            if !already {
                csr.fields.push(CsrField {
                    bits: row.bits,
                    name: row.name,
                    access: row.access,
                    description: row.description,
                });
            }
        }
    }
    if fields_seen && !fields_parsed {
        eprintln!(
            "auto_populate: populate_csrs: {} exists but no rows parsed cleanly",
            fields_dir.display()
        );
    }

    Ok(appended)
}

/// Append one `GlossaryEntry` per `format.json::glossary[]` entry
/// (when a descriptor is loaded) plus any rows found in
/// `<corpus>/glossary.toml` (when classify writes one). Each entry
/// is idempotent on `term`.
pub fn populate_glossary(
    corpus_root: &Path,
    spec: &mut SpecMd,
    format: Option<&FormatJson>,
) -> Result<usize> {
    let mut appended = 0usize;

    if let Some(fmt) = format {
        for entry in &fmt.glossary {
            let anchor = if entry.first_page == 0 {
                String::new()
            } else {
                format!("primary:p{}", entry.first_page)
            };
            let term = entry.acronym.clone();
            if term.is_empty() {
                continue;
            }
            if spec.glossary.iter().any(|g| g.term == term) {
                continue;
            }
            spec.glossary.push(GlossaryEntry {
                term,
                expansion: entry.expansion.clone(),
                scope: entry.scope.clone(),
                used_in_blocks: entry.used_in_blocks.clone(),
                source_anchor: anchor,
            });
            appended += 1;
        }
    }

    // Optional on-disk glossary shard. classify.rs may emit one in
    // the format-driven path; we read it gracefully when present.
    let glossary_path = corpus_root.join("glossary.toml");
    if glossary_path.is_file() {
        let body = read_required(&glossary_path)?;
        match parse_toml::<RawGlossaryTable>(&glossary_path, &body) {
            Ok(table) => {
                for row in table.rows {
                    let term = row.term.clone();
                    if term.is_empty() {
                        continue;
                    }
                    if spec.glossary.iter().any(|g| g.term == term) {
                        continue;
                    }
                    spec.glossary.push(GlossaryEntry {
                        term,
                        expansion: row.expansion,
                        scope: row.scope,
                        used_in_blocks: row.used_in_blocks,
                        source_anchor: row.source_anchor,
                    });
                    appended += 1;
                }
            }
            Err(_) => {
                eprintln!(
                    "auto_populate: populate_glossary: {} present but failed to parse",
                    glossary_path.display()
                );
            }
        }
    }

    Ok(appended)
}

/// Append one `ClockDomain` per row across every
/// `<corpus>/tables/clock_domains/*.toml` shard. Idempotent on
/// `name`. Returns 0 when the directory is absent (the common
/// v1 case).
pub fn populate_clock_domains(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("clock_domains");
    let mut appended = 0usize;
    let mut any_seen = false;
    let mut any_parsed = false;
    for path in list_toml_files(&dir) {
        any_seen = true;
        let body = read_required(&path)?;
        let table: RawClockDomainTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        any_parsed = true;
        for row in table.rows {
            if row.name.is_empty() {
                continue;
            }
            if spec.clock_domains.iter().any(|d| d.name == row.name) {
                continue;
            }
            spec.clock_domains.push(ClockDomain {
                name: row.name,
                frequency: row.frequency,
                source: row.source,
                description: row.description,
            });
            appended += 1;
        }
    }
    if any_seen && !any_parsed {
        eprintln!(
            "auto_populate: populate_clock_domains: {} exists but no rows parsed cleanly",
            dir.display()
        );
    }
    Ok(appended)
}

/// Append one `PowerDomain` per row across every
/// `<corpus>/tables/power_domains/*.toml` shard. Idempotent on
/// `name`. Returns 0 when the directory is absent.
pub fn populate_power_domains(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("power_domains");
    let mut appended = 0usize;
    let mut any_seen = false;
    let mut any_parsed = false;
    for path in list_toml_files(&dir) {
        any_seen = true;
        let body = read_required(&path)?;
        let table: RawPowerDomainTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        any_parsed = true;
        for row in table.rows {
            if row.name.is_empty() {
                continue;
            }
            if spec.power_domains.iter().any(|d| d.name == row.name) {
                continue;
            }
            spec.power_domains.push(PowerDomain {
                name: row.name,
                voltage: row.voltage,
                always_on: row.always_on,
                description: row.description,
            });
            appended += 1;
        }
    }
    if any_seen && !any_parsed {
        eprintln!(
            "auto_populate: populate_power_domains: {} exists but no rows parsed cleanly",
            dir.display()
        );
    }
    Ok(appended)
}

/// Append one `ResetDomain` per row across every
/// `<corpus>/tables/reset_domains/*.toml` shard. Idempotent on
/// `name`. Returns 0 when the directory is absent.
pub fn populate_reset_domains(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("reset_domains");
    let mut appended = 0usize;
    let mut any_seen = false;
    let mut any_parsed = false;
    for path in list_toml_files(&dir) {
        any_seen = true;
        let body = read_required(&path)?;
        let table: RawResetDomainTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        any_parsed = true;
        for row in table.rows {
            if row.name.is_empty() {
                continue;
            }
            if spec.reset_domains.iter().any(|d| d.name == row.name) {
                continue;
            }
            spec.reset_domains.push(ResetDomain {
                name: row.name,
                polarity: row.polarity,
                sync: row.sync,
                source: row.source,
                description: row.description,
            });
            appended += 1;
        }
    }
    if any_seen && !any_parsed {
        eprintln!(
            "auto_populate: populate_reset_domains: {} exists but no rows parsed cleanly",
            dir.display()
        );
    }
    Ok(appended)
}

/// Append one `PrivilegeLevel` per row across every
/// `<corpus>/tables/security_boundaries/*.toml` (or the synonym
/// `tables/privilege_levels/*.toml`) shard. Idempotent on `id`.
/// Returns 0 when both directories are absent.
pub fn populate_security_boundaries(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let mut appended = 0usize;
    for dirname in ["security_boundaries", "privilege_levels"] {
        let dir = corpus_root.join("tables").join(dirname);
        let mut any_seen = false;
        let mut any_parsed = false;
        for path in list_toml_files(&dir) {
            any_seen = true;
            let body = read_required(&path)?;
            let table: RawPrivilegeLevelTable = match parse_toml(&path, &body) {
                Ok(t) => t,
                Err(_) => continue,
            };
            any_parsed = true;
            for row in table.rows {
                if row.id.is_empty() {
                    continue;
                }
                if spec.security_boundaries.iter().any(|p| p.id == row.id) {
                    continue;
                }
                spec.security_boundaries.push(PrivilegeLevel {
                    id: row.id,
                    name: row.name,
                    description: row.description,
                    capabilities: row.capabilities,
                });
                appended += 1;
            }
        }
        if any_seen && !any_parsed {
            eprintln!(
                "auto_populate: populate_security_boundaries: {} exists but no rows parsed cleanly",
                dir.display()
            );
        }
    }
    Ok(appended)
}

/// Append one `NumericalConvention` per row across every
/// `<corpus>/tables/numerical_conventions/*.toml` shard. Idempotent
/// on `name`. Returns 0 when the directory is absent.
pub fn populate_numerical_conventions(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let dir = corpus_root.join("tables").join("numerical_conventions");
    let mut appended = 0usize;
    let mut any_seen = false;
    let mut any_parsed = false;
    for path in list_toml_files(&dir) {
        any_seen = true;
        let body = read_required(&path)?;
        let table: RawNumericalConventionTable = match parse_toml(&path, &body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        any_parsed = true;
        for row in table.rows {
            if row.name.is_empty() {
                continue;
            }
            if spec
                .numerical_conventions
                .iter()
                .any(|n| n.name == row.name)
            {
                continue;
            }
            spec.numerical_conventions.push(NumericalConvention {
                name: row.name,
                q_format_default: row.q_format_default,
                saturation_policy: row.saturation_policy,
                signed_default: row.signed_default,
                rounding_mode: row.rounding_mode,
                description: row.description,
            });
            appended += 1;
        }
    }
    if any_seen && !any_parsed {
        eprintln!(
            "auto_populate: populate_numerical_conventions: {} exists but no rows parsed cleanly",
            dir.display()
        );
    }
    Ok(appended)
}

/// Append one `PmuEvent` per row across every
/// `<corpus>/tables/pmu/*.toml` (or the synonym `tables/pmu_events/`)
/// shard. Idempotent on `id`. Returns 0 when both directories are
/// absent.
pub fn populate_performance_counters(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let mut appended = 0usize;
    for dirname in ["pmu", "pmu_events"] {
        let dir = corpus_root.join("tables").join(dirname);
        let mut any_seen = false;
        let mut any_parsed = false;
        for path in list_toml_files(&dir) {
            any_seen = true;
            let body = read_required(&path)?;
            let table: RawPmuEventTable = match parse_toml(&path, &body) {
                Ok(t) => t,
                Err(_) => continue,
            };
            any_parsed = true;
            for row in table.rows {
                if row.id.is_empty() {
                    continue;
                }
                if spec.performance_counters.iter().any(|e| e.id == row.id) {
                    continue;
                }
                spec.performance_counters.push(PmuEvent {
                    id: row.id,
                    name: row.name,
                    description: row.description,
                    csr_address: row.csr_address,
                });
                appended += 1;
            }
        }
        if any_seen && !any_parsed {
            eprintln!(
                "auto_populate: populate_performance_counters: {} exists but no rows parsed cleanly",
                dir.display()
            );
        }
    }
    Ok(appended)
}

/// Build the `Source-Spec Anchors` index by walking already-populated
/// sections and emitting one row per (section, anchor) pair. The
/// row's `chunk_id` / `page_range` are derived from the anchor's
/// textual form: `primary:p<N>` → `chunk_id = ""`, `page_range =
/// "N"`; `primary:p<N>-<M>` → `page_range = "N-M"`; chunk form →
/// `chunk_id` populated, `page_range` empty.
///
/// Idempotent on `(section_path, source, chunk_id, page_range)`.
pub fn populate_anchors(spec: &mut SpecMd) -> Result<usize> {
    use crate::__internal::session::spec_md::types::SourceSpecAnchor;

    let mut pending: Vec<AnchorIndexEntry> = Vec::new();

    let emit = |section_path: String, anchor_text: &str, pending: &mut Vec<AnchorIndexEntry>| {
        let Ok(parsed) = SourceSpecAnchor::parse(anchor_text) else {
            return;
        };
        let (source, chunk_id, page_range) = match &parsed {
            SourceSpecAnchor::Page { source, page } => {
                (source.clone(), String::new(), page.to_string())
            }
            SourceSpecAnchor::PageRange { source, start, end } => {
                (source.clone(), String::new(), format!("{start}-{end}"))
            }
            SourceSpecAnchor::Chunk { source, chunk } => {
                (source.clone(), format!("chunk-{chunk}"), String::new())
            }
        };
        pending.push(AnchorIndexEntry {
            section_path,
            source,
            chunk_id,
            page_range,
        });
    };

    // External interfaces.
    for iface in &spec.external_interfaces {
        for a in &iface.source_anchors {
            emit(
                format!("External Interfaces > {}", iface.name),
                a,
                &mut pending,
            );
        }
    }
    // Blocks.
    for block in &spec.blocks {
        for a in &block.source_anchors {
            emit(format!("Blocks > {}", block.name), a, &mut pending);
        }
    }
    // Parameters (one anchor per row).
    for p in &spec.parameters {
        if !p.source_anchor.is_empty() {
            emit(
                format!("Parameters > {}", p.name),
                &p.source_anchor,
                &mut pending,
            );
        }
    }
    // State machines.
    for s in &spec.state_machines {
        if !s.source_anchor.is_empty() {
            emit(
                format!("State Machines > {}", s.name),
                &s.source_anchor,
                &mut pending,
            );
        }
    }
    // Encodings.
    for e in &spec.encodings {
        if !e.source_anchor.is_empty() {
            emit(
                format!("Encodings > {}", e.field),
                &e.source_anchor,
                &mut pending,
            );
        }
    }
    // Memory map.
    for m in &spec.memory_map {
        if !m.source_anchor.is_empty() {
            emit(
                format!("Memory Map > {}", m.name),
                &m.source_anchor,
                &mut pending,
            );
        }
    }
    // Error handling.
    for e in &spec.error_handling {
        if !e.source_anchor.is_empty() {
            emit(
                format!("Error Handling > {}", e.error_type),
                &e.source_anchor,
                &mut pending,
            );
        }
    }
    // Assumptions / Quantitative rows.
    for q in &spec.assumptions.quantitative {
        if !q.source_anchor.is_empty() {
            emit(
                format!(
                    "Assumptions and Constraints > Quantitative > {}",
                    q.constraint
                ),
                &q.source_anchor,
                &mut pending,
            );
        }
    }

    let mut appended = 0usize;
    for entry in pending {
        let already = spec.source_spec_anchors.iter().any(|x| {
            x.section_path == entry.section_path
                && x.source == entry.source
                && x.chunk_id == entry.chunk_id
                && x.page_range == entry.page_range
        });
        if !already {
            spec.source_spec_anchors.push(entry);
            appended += 1;
        }
    }
    Ok(appended)
}

/// Turn each entry in `<corpus>/tbds.toml` into an `OpenQuestion`.
/// The question text carries the breadcrumb (so the agent has
/// enough context to resolve it) plus the TBD's surrounding line
/// and the page number.
///
/// Idempotent on the rendered question text.
/// Read `tables/unknown/*.toml` shards (written by classify when a
/// table's `(kind, target)` pair doesn't match any typed dispatch
/// arm) and surface one Open Question per shard so DM0 can ask the
/// user how to interpret the rows. Without this read, classify's
/// catch-all bucket is silently dropped — losing potentially useful
/// data the LLM critique pass left as `Unknown`.
///
/// Idempotent on the rendered question text.
pub fn populate_open_questions_from_unknown_tables(
    corpus_root: &Path,
    spec: &mut SpecMd,
) -> Result<usize> {
    let dir = corpus_root.join("tables").join("unknown");
    let mut appended = 0usize;
    for path in list_toml_files(&dir) {
        let body = read_required(&path)?;
        let raw: RawUnknownTable = parse_toml(&path, &body)?;
        let header_summary = if raw.header_row.is_empty() {
            String::new()
        } else {
            raw.header_row.join(" | ")
        };
        let text = match (raw.source_table_id.is_empty(), header_summary.is_empty()) {
            (true, true) => format!(
                "Unclassified table on primary:p{} ({} row(s)). How should this be modeled?",
                raw.source_page,
                raw.rows.len()
            ),
            (true, false) => format!(
                "Unclassified table on primary:p{} (columns: {header_summary}; {} row(s)). \
                 How should this be modeled?",
                raw.source_page,
                raw.rows.len()
            ),
            (false, true) => format!(
                "Unclassified table {} on primary:p{} ({} row(s)). How should this be modeled?",
                raw.source_table_id,
                raw.source_page,
                raw.rows.len()
            ),
            (false, false) => format!(
                "Unclassified table {} on primary:p{} (columns: {header_summary}; \
                 {} row(s)). How should this be modeled?",
                raw.source_table_id,
                raw.source_page,
                raw.rows.len()
            ),
        };
        let already = spec.open_questions.iter().any(|q| q.text == text);
        if !already {
            spec.open_questions.push(OpenQuestion { text });
            appended += 1;
        }
    }
    Ok(appended)
}

pub fn populate_open_questions_from_tbds(corpus_root: &Path, spec: &mut SpecMd) -> Result<usize> {
    let path = corpus_root.join("tbds.toml");
    if !path.is_file() {
        return Ok(0);
    }
    let body = read_required(&path)?;
    let raw: RawTbds = parse_toml(&path, &body)?;
    let mut appended = 0usize;
    for tbd in raw.tbds {
        let breadcrumb = if tbd.breadcrumb.is_empty() {
            String::new()
        } else {
            tbd.breadcrumb.join(" > ")
        };
        let text = match (breadcrumb.is_empty(), tbd.context.is_empty()) {
            (true, true) => format!("Unresolved TBD on primary:p{}.", tbd.source_page),
            (true, false) => format!(
                "Unresolved TBD on primary:p{}: {}",
                tbd.source_page, tbd.context
            ),
            (false, true) => format!(
                "Unresolved TBD in `{breadcrumb}` (primary:p{}).",
                tbd.source_page
            ),
            (false, false) => format!(
                "Unresolved TBD in `{breadcrumb}` (primary:p{}): {}",
                tbd.source_page, tbd.context
            ),
        };
        let already = spec.open_questions.iter().any(|q| q.text == text);
        if !already {
            spec.open_questions.push(OpenQuestion { text });
            appended += 1;
        }
    }
    Ok(appended)
}

// ---------------------------------------------------------------------------
// Path / parsing helpers
// ---------------------------------------------------------------------------

fn manifest_path(project_dir: &Path) -> PathBuf {
    super::manifest_path(project_dir)
}

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

fn read_required(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_toml<T: for<'de> Deserialize<'de>>(path: &Path, body: &str) -> Result<T> {
    toml::from_str(body).map_err(|source| Error::TomlParse {
        path: path.to_path_buf(),
        source,
    })
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

/// Parse a `page-NNN[-suffix]` figure filename stem and return the
/// page number. Accepts `page-013`, `page-13`, `page-013-foo`. The
/// suffix is anything after the page digits.
fn parse_page_filename(stem: &str) -> Option<u32> {
    let rest = stem.strip_prefix("page-")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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
struct RawSignalTable {
    stage: String,
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawSignalRow>,
}

#[derive(Debug, Deserialize)]
struct RawSignalRow {
    name: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    peer: String,
    #[serde(default)]
    description: String,
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
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    default: String,
    #[serde(default)]
    comment: String,
}

#[derive(Debug, Deserialize)]
struct RawErrorTable {
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawErrorRow>,
}

#[derive(Debug, Deserialize)]
struct RawErrorRow {
    #[serde(default)]
    error_type: String,
    #[serde(default)]
    detecting_component: String,
    #[serde(default)]
    detecting_behavior: String,
    #[serde(default)]
    bus_response: String,
    #[serde(default)]
    master_behavior: String,
    #[serde(default)]
    software_response: String,
}

#[derive(Debug, Deserialize)]
struct RawEncodingTable {
    field: String,
    #[serde(default)]
    bit_width: Option<u32>,
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawEncodingRow>,
}

#[derive(Debug, Deserialize)]
struct RawEncodingRow {
    value: String,
    name: String,
    #[serde(default)]
    abbreviation: String,
}

#[derive(Debug, Deserialize)]
struct RawFsmTable {
    name: String,
    #[serde(default)]
    reset_state: Option<String>,
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    transitions: Vec<RawFsmTransition>,
}

#[derive(Debug, Deserialize)]
struct RawFsmTransition {
    from: String,
    input: String,
    to: String,
    #[serde(default)]
    output: String,
}

#[derive(Debug, Deserialize)]
struct RawTbds {
    #[serde(default)]
    tbds: Vec<RawTbd>,
}

#[derive(Debug, Deserialize, Default)]
struct RawUnknownTable {
    #[serde(default)]
    source_table_id: String,
    #[serde(default)]
    source_page: u32,
    #[serde(default)]
    header_row: Vec<String>,
    #[serde(default)]
    rows: Vec<RawUnknownRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawUnknownRow {
    #[serde(default)]
    #[allow(dead_code)]
    cells: Vec<String>,
}

// ---- Phase 9 milestone 9.12 on-disk shapes ----

#[derive(Debug, Deserialize, Default)]
struct RawCsrTable {
    #[serde(default)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawCsrRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCsrRow {
    #[serde(default)]
    address: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    access: String,
    #[serde(default)]
    reset_value: String,
    #[serde(default)]
    required_privilege: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawCsrFieldTable {
    #[serde(default)]
    csr_name: String,
    // Kept for forward-compat — the parent CSR already carries the
    // anchor so we don't reuse this per-field.
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawCsrFieldRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCsrFieldRow {
    #[serde(default)]
    bits: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    access: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawGlossaryTable {
    #[serde(default)]
    rows: Vec<RawGlossaryRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawGlossaryRow {
    #[serde(default)]
    term: String,
    #[serde(default)]
    expansion: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    used_in_blocks: Vec<String>,
    #[serde(default)]
    source_anchor: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawClockDomainTable {
    // Domain types don't carry a source_anchor (Chapter 7 §7.7) so
    // this is accepted but unused.
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawClockDomainRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawClockDomainRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    frequency: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawPowerDomainTable {
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawPowerDomainRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawPowerDomainRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    voltage: String,
    #[serde(default)]
    always_on: bool,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawResetDomainTable {
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawResetDomainRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawResetDomainRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    polarity: String,
    #[serde(default)]
    sync: bool,
    #[serde(default)]
    source: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawPrivilegeLevelTable {
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawPrivilegeLevelRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawPrivilegeLevelRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawNumericalConventionTable {
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawNumericalConventionRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawNumericalConventionRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    q_format_default: String,
    #[serde(default)]
    saturation_policy: String,
    #[serde(default)]
    signed_default: String,
    #[serde(default)]
    rounding_mode: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawPmuEventTable {
    #[serde(default)]
    #[allow(dead_code)]
    source_page_range: Vec<u32>,
    #[serde(default)]
    rows: Vec<RawPmuEventRow>,
}

#[derive(Debug, Deserialize, Default)]
struct RawPmuEventRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    csr_address: String,
}

#[derive(Debug, Deserialize)]
struct RawTbd {
    #[serde(default)]
    breadcrumb: Vec<String>,
    #[serde(default)]
    source_page: u32,
    #[serde(default)]
    context: String,
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
    fn parameters_round_trip_per_shard() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus
            .join("tables")
            .join("parameters")
            .join("000-core.toml");
        write(
            &p,
            "schema_version = 1\n\
             table_kind = \"parameter_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [3, 3]\n\
             group = \"core\"\n\
             \n\
             [[rows]]\n\
             name = \"XLEN\"\n\
             kind = \"int\"\n\
             default = \"32\"\n\
             comment = \"register width\"\n\
             \n\
             [[rows]]\n\
             name = \"HAS_BPU\"\n\
             kind = \"bool\"\n\
             default = \"true\"\n\
             comment = \"branch prediction unit\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_parameters(&corpus, &mut spec).unwrap();
        assert_eq!(n, 2);
        assert_eq!(spec.parameters.len(), 2);
        assert_eq!(spec.parameters[0].name, "XLEN");
        assert_eq!(spec.parameters[0].ty, "int");
        assert_eq!(spec.parameters[0].default, "32");
        assert_eq!(spec.parameters[0].behavioral_impact, "register width");
        assert_eq!(spec.parameters[0].source_anchor, "primary:p3");
        // Idempotency.
        let n2 = populate_parameters(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
        assert_eq!(spec.parameters.len(), 2);
    }

    #[test]
    fn encodings_round_trip_per_shard() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus
            .join("tables")
            .join("encodings")
            .join("000-priv.toml");
        write(
            &p,
            "schema_version = 1\n\
             table_kind = \"encoding_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [5, 5]\n\
             field = \"Privilege Level\"\n\
             bit_width = 2\n\
             \n\
             [[rows]]\n\
             value = \"00\"\n\
             name = \"User/Application\"\n\
             abbreviation = \"U\"\n\
             \n\
             [[rows]]\n\
             value = \"11\"\n\
             name = \"Machine\"\n\
             abbreviation = \"M\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_encodings(&corpus, &mut spec).unwrap();
        assert_eq!(n, 1);
        assert_eq!(spec.encodings.len(), 1);
        assert_eq!(spec.encodings[0].field, "Privilege Level");
        assert_eq!(spec.encodings[0].bit_width, "2");
        assert_eq!(spec.encodings[0].source_anchor, "primary:p5");
        assert_eq!(spec.encodings[0].values.len(), 2);
        let n2 = populate_encodings(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn errors_round_trip_per_shard() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus.join("tables").join("errors").join("000.toml");
        write(
            &p,
            "schema_version = 1\n\
             table_kind = \"error_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [28, 28]\n\
             \n\
             [[rows]]\n\
             error_type = \"Bus error\"\n\
             detecting_component = \"NoC\"\n\
             detecting_behavior = \"Log Error\"\n\
             bus_response = \"Bus error\"\n\
             master_behavior = \"Abort\"\n\
             software_response = \"Interrupt\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_errors(&corpus, &mut spec).unwrap();
        assert_eq!(n, 1);
        assert_eq!(spec.error_handling[0].error_type, "Bus error");
        assert_eq!(spec.error_handling[0].detection_behavior, "Log Error");
        assert_eq!(spec.error_handling[0].source_anchor, "primary:p28");
        let n2 = populate_errors(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn fsms_round_trip_per_shard() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let p = corpus.join("tables").join("fsms").join("000-boot.toml");
        write(
            &p,
            "schema_version = 1\n\
             table_kind = \"fsm_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [8, 9]\n\
             name = \"Boot FSM\"\n\
             reset_state = \"IDLE\"\n\
             \n\
             [[transitions]]\n\
             from = \"IDLE\"\n\
             input = \"power_on\"\n\
             to = \"RESET_HOLD\"\n\
             output = \"assert nReset\"\n\
             \n\
             [[transitions]]\n\
             from = \"RESET_HOLD\"\n\
             input = \"stability_timer_done\"\n\
             to = \"RESET_RELEASE\"\n\
             output = \"begin reset deassertion\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_fsms(&corpus, &mut spec).unwrap();
        assert_eq!(n, 1);
        let fsm = &spec.state_machines[0];
        assert_eq!(fsm.name, "Boot FSM");
        assert_eq!(fsm.reset_state, "IDLE");
        assert_eq!(fsm.source_anchor, "primary:p8-9");
        assert_eq!(fsm.transitions.len(), 2);
        assert_eq!(fsm.states.len(), 3);
        let names: Vec<&str> = fsm.states.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"IDLE"));
        assert!(names.contains(&"RESET_HOLD"));
        assert!(names.contains(&"RESET_RELEASE"));
        let n2 = populate_fsms(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn figures_one_per_png_with_parsed_page() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(&corpus.join("figures").join("page-013.png"), "PNGDATA");
        write(&corpus.join("figures").join("page-007.png"), "PNGDATA");
        write(&corpus.join("figures").join("notes.txt"), "ignored");
        let mut spec = SpecMd::default();
        let n = populate_figures(&corpus, &mut spec).unwrap();
        assert_eq!(n, 2);
        assert_eq!(spec.figures.len(), 2);
        // Sorted lexicographically by raster path.
        assert_eq!(spec.figures[0].raster, "figures/page-007.png");
        assert_eq!(spec.figures[0].source_page, "7");
        assert_eq!(spec.figures[0].name, "page-007");
        assert_eq!(spec.figures[1].raster, "figures/page-013.png");
        assert_eq!(spec.figures[1].source_page, "13");
        // Captions empty by design.
        assert!(spec.figures[0].caption.is_empty());
        let n2 = populate_figures(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn anchors_built_from_populated_sections() {
        let mut spec = SpecMd::default();
        spec.blocks.push(Block {
            name: "Instruction Fetch (IF)".into(),
            parent: "(none -- top-level)".into(),
            source_anchors: vec!["primary:p12-13".into()],
            ..Default::default()
        });
        spec.parameters.push(Parameter {
            name: "XLEN".into(),
            ty: "int".into(),
            default: "32".into(),
            valid_range: String::new(),
            behavioral_impact: String::new(),
            source_anchor: "primary:p3".into(),
        });
        spec.assumptions.quantitative.push(QuantitativeRow {
            constraint: "Clock frequency".into(),
            value: "1 GHz".into(),
            source_anchor: "primary:p3".into(),
        });
        let n = populate_anchors(&mut spec).unwrap();
        assert_eq!(n, 3);
        let paths: Vec<&str> = spec
            .source_spec_anchors
            .iter()
            .map(|x| x.section_path.as_str())
            .collect();
        assert!(paths.contains(&"Blocks > Instruction Fetch (IF)"));
        assert!(paths.contains(&"Parameters > XLEN"));
        assert!(paths.contains(&"Assumptions and Constraints > Quantitative > Clock frequency"));
        let block_row = spec
            .source_spec_anchors
            .iter()
            .find(|r| r.section_path == "Blocks > Instruction Fetch (IF)")
            .unwrap();
        assert_eq!(block_row.source, "primary");
        assert_eq!(block_row.page_range, "12-13");
        assert_eq!(block_row.chunk_id, "");
        // Idempotency.
        let n2 = populate_anchors(&mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn open_questions_from_tbds_carries_breadcrumb() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tbds.toml"),
            "schema_version = 1\n\
             \n\
             [[tbds]]\n\
             chunk_id = \"c1\"\n\
             breadcrumb = [\"Pipeline\", \"IF\"]\n\
             source_page = 13\n\
             context = \"Reset value for `if_exception` not stated\"\n\
             \n\
             [[tbds]]\n\
             chunk_id = \"c2\"\n\
             breadcrumb = [\"BPU\"]\n\
             source_page = 9\n\
             context = \"BPU table size at default BPU_LOCAL_BITS=8 TBD\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_open_questions_from_tbds(&corpus, &mut spec).unwrap();
        assert_eq!(n, 2);
        assert!(spec.open_questions[0].text.contains("Pipeline > IF"));
        assert!(spec.open_questions[0].text.contains("primary:p13"));
        assert!(spec.open_questions[0].text.contains("if_exception"));
        // Idempotency.
        let n2 = populate_open_questions_from_tbds(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn open_questions_from_unknown_tables_surfaces_header_and_table_id() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tables").join("unknown").join("000-T38.toml"),
            "schema_version = 1\n\
             table_kind = \"unknown\"\n\
             source_table_id = \"T38\"\n\
             source_page = 45\n\
             header_row = [\"Mnemonic\", \"Opcode\", \"Description\"]\n\
             \n\
             [[rows]]\n\
             cells = [\"ADD\", \"0110011\", \"Add registers\"]\n\
             \n\
             [[rows]]\n\
             cells = [\"SUB\", \"0110011\", \"Subtract registers\"]\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_open_questions_from_unknown_tables(&corpus, &mut spec).unwrap();
        assert_eq!(n, 1);
        let q = &spec.open_questions[0].text;
        assert!(q.contains("T38"), "got `{q}`");
        assert!(q.contains("primary:p45"), "got `{q}`");
        assert!(q.contains("Mnemonic | Opcode | Description"), "got `{q}`");
        assert!(q.contains("2 row(s)"), "got `{q}`");
        // Idempotency.
        let n2 = populate_open_questions_from_unknown_tables(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn open_questions_from_unknown_tables_missing_dir_is_noop() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let mut spec = SpecMd::default();
        let n = populate_open_questions_from_unknown_tables(&corpus, &mut spec).unwrap();
        assert_eq!(n, 0);
        assert!(spec.open_questions.is_empty());
    }

    #[test]
    fn run_orchestrates_every_step_and_reports_counts() {
        let tmp = make_project();
        let project = tmp.path();
        // Minimal but representative ingest corpus.
        write(
            &manifest_path(project),
            "schema_version = 1\n\
             source_kind = \"pdf\"\n\
             source_path = \"docs/main.pdf\"\n",
        );
        let corpus = corpus_root(project);
        write(
            &corpus.join("tables").join("parameters").join("000.toml"),
            "schema_version = 1\n\
             table_kind = \"parameter_table\"\n\
             source_chunk_id = \"c\"\n\
             source_page_range = [3, 3]\n\
             group = \"x\"\n\
             \n\
             [[rows]]\n\
             name = \"XLEN\"\n\
             default = \"32\"\n\
             comment = \"register width at 1 GHz\"\n",
        );
        write(
            &corpus.join("tables").join("signals").join("000-if.toml"),
            "schema_version = 1\n\
             table_kind = \"signal_table\"\n\
             source_chunk_id = \"c\"\n\
             source_page_range = [12, 13]\n\
             stage = \"IF\"\n\
             breadcrumb = [\"P\", \"IF\"]\n\
             \n\
             [[rows]]\n\
             name = \"if_pc\"\n\
             direction = \"out\"\n\
             peer = \"PD\"\n\
             description = \"pc\"\n",
        );
        write(&corpus.join("figures").join("page-013.png"), "DATA");
        write(
            &corpus.join("tbds.toml"),
            "schema_version = 1\n\
             \n\
             [[tbds]]\n\
             chunk_id = \"c\"\n\
             breadcrumb = [\"IF\"]\n\
             source_page = 13\n\
             context = \"reset value TBD\"\n",
        );

        let mut spec = SpecMd::default();
        let report = run(project, &mut spec).unwrap();
        assert_eq!(report.parameters, 1);
        assert_eq!(report.blocks, 1);
        assert_eq!(report.figures, 1);
        assert_eq!(report.open_questions, 1);
        // anchors come from blocks + parameters + clock-quant row.
        assert!(report.anchors >= 3);
        assert_eq!(spec.metadata.source_documents.len(), 1);
        assert!(
            spec.assumptions
                .quantitative
                .iter()
                .any(|r| r.constraint == "Clock frequency")
        );
        // Re-running must not duplicate.
        let report2 = run(project, &mut spec).unwrap();
        assert_eq!(report2.parameters, 0);
        assert_eq!(report2.blocks, 0);
        assert_eq!(report2.figures, 0);
        assert_eq!(report2.open_questions, 0);
        assert_eq!(report2.anchors, 0);
    }

    #[test]
    fn open_questions_missing_file_is_zero() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let mut spec = SpecMd::default();
        let n = populate_open_questions_from_tbds(&corpus, &mut spec).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn blocks_one_per_signal_shard_with_top_level_parent() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let dir = corpus.join("tables").join("signals");
        let shards = [
            ("000-if.toml", "Instruction Fetch (IF)", 12u32, 13u32),
            ("001-pd.toml", "Pre-Decode (PD)", 14, 14),
            ("002-id.toml", "Instruction Decode (ID)", 15, 15),
            ("003-ex.toml", "Execute (EX)", 16, 16),
            ("004-mem.toml", "Memory (MEM)", 17, 17),
            ("005-wb.toml", "Write Back (WB)", 18, 18),
        ];
        for (filename, stage, p0, p1) in shards.iter() {
            let body = format!(
                "schema_version = 1\n\
                 table_kind = \"signal_table\"\n\
                 source_chunk_id = \"c-{filename}\"\n\
                 source_page_range = [{p0}, {p1}]\n\
                 stage = \"{stage}\"\n\
                 breadcrumb = [\"Pipeline\", \"{stage}\"]\n\
                 \n\
                 [[rows]]\n\
                 name = \"if_nxt_pc\"\n\
                 direction = \"out\"\n\
                 peer = \"Bus\"\n\
                 description = \"next pc\"\n"
            );
            write(&dir.join(filename), &body);
        }
        let mut spec = SpecMd::default();
        let n = populate_blocks(&corpus, &mut spec).unwrap();
        assert_eq!(n, 6);
        assert_eq!(spec.blocks.len(), 6);
        for b in &spec.blocks {
            assert_eq!(b.parent, "(none -- top-level)");
            assert_eq!(b.signals.len(), 1);
            assert_eq!(b.source_anchors.len(), 1);
        }
        assert_eq!(spec.blocks[0].name, "Instruction Fetch (IF)");
        assert_eq!(spec.blocks[0].source_anchors[0], "primary:p12-13");
        let n2 = populate_blocks(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
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

    // ---------------------------------------------------------------
    // Phase 9 milestone 9.12 tests
    // ---------------------------------------------------------------

    use crate::__internal::session::spec_ingest::format::{
        ColumnMapping, FontWeight, FormatJson, GlossaryEntry as FmtGlossaryEntry, GlossarySource,
        Layer as FmtLayer, SectionRoleEntry, SpecMdRole, TableEntry as FmtTableEntry, TableKind,
        TableTarget, ValidationBlock, WrapStrategy,
    };
    use chrono::{TimeZone, Utc};

    fn empty_format() -> FormatJson {
        FormatJson {
            schema_version: 1,
            model: "test".into(),
            prompt_version: "test".into(),
            source_sha256: "".into(),
            discovered_at: Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap(),
            section_roles: Vec::new(),
            tables: Vec::new(),
            figures: Vec::new(),
            glossary: Vec::new(),
            chrome: Vec::new(),
            validation: ValidationBlock::default(),
        }
    }

    #[test]
    fn populate_csrs_loads_csr_header_and_field_shards() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tables").join("csrs").join("000-mstatus.toml"),
            "schema_version = 1\n\
             table_kind = \"csr_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [42, 42]\n\
             \n\
             [[rows]]\n\
             address = \"0x300\"\n\
             name = \"mstatus\"\n\
             access = \"RW\"\n\
             reset_value = \"0x0\"\n\
             required_privilege = \"M\"\n\
             description = \"Machine status register\"\n",
        );
        write(
            &corpus
                .join("tables")
                .join("csr_fields")
                .join("000-mstatus.toml"),
            "schema_version = 1\n\
             table_kind = \"csr_field_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [42, 42]\n\
             csr_name = \"mstatus\"\n\
             \n\
             [[rows]]\n\
             bits = \"3\"\n\
             name = \"MIE\"\n\
             access = \"RW\"\n\
             description = \"Machine interrupt enable\"\n\
             \n\
             [[rows]]\n\
             bits = \"7\"\n\
             name = \"MPIE\"\n\
             access = \"RW\"\n\
             description = \"Previous machine interrupt enable\"\n",
        );

        let mut spec = SpecMd::default();
        let n = populate_csrs(&corpus, &mut spec).unwrap();
        assert_eq!(n, 1);
        assert_eq!(spec.csrs.len(), 1);
        let csr = &spec.csrs[0];
        assert_eq!(csr.name, "mstatus");
        assert_eq!(csr.address, "0x300");
        assert_eq!(csr.access, "RW");
        assert_eq!(csr.reset_value, "0x0");
        assert_eq!(csr.required_privilege, "M");
        assert_eq!(csr.source_anchor, "primary:p42");
        assert_eq!(csr.fields.len(), 2);
        assert_eq!(csr.fields[0].bits, "3");
        assert_eq!(csr.fields[0].name, "MIE");
        assert_eq!(csr.fields[1].name, "MPIE");

        // Idempotent.
        let n2 = populate_csrs(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
        assert_eq!(spec.csrs.len(), 1);
        assert_eq!(spec.csrs[0].fields.len(), 2);
    }

    #[test]
    fn populate_glossary_from_format_json() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let mut fmt = empty_format();
        fmt.glossary.push(FmtGlossaryEntry {
            acronym: "IF".into(),
            expansion: "Instruction Fetch".into(),
            first_page: 11,
            scope: "spec".into(),
            used_in_blocks: vec!["Instruction Fetch (IF)".into()],
            source: GlossarySource::ParenthesisedFirstMention,
        });
        fmt.glossary.push(FmtGlossaryEntry {
            acronym: "PD".into(),
            expansion: "Pre-Decode".into(),
            first_page: 13,
            scope: "spec".into(),
            used_in_blocks: vec![],
            source: GlossarySource::GlossarySection,
        });

        let mut spec = SpecMd::default();
        let n = populate_glossary(&corpus, &mut spec, Some(&fmt)).unwrap();
        assert_eq!(n, 2);
        assert_eq!(spec.glossary.len(), 2);
        assert_eq!(spec.glossary[0].term, "IF");
        assert_eq!(spec.glossary[0].expansion, "Instruction Fetch");
        assert_eq!(spec.glossary[0].source_anchor, "primary:p11");
        assert_eq!(
            spec.glossary[0].used_in_blocks,
            vec!["Instruction Fetch (IF)"]
        );
        assert_eq!(spec.glossary[1].term, "PD");
        assert_eq!(spec.glossary[1].source_anchor, "primary:p13");

        // Idempotent on `term`.
        let n2 = populate_glossary(&corpus, &mut spec, Some(&fmt)).unwrap();
        assert_eq!(n2, 0);
        assert_eq!(spec.glossary.len(), 2);

        // None-format path is a no-op when the on-disk shard is
        // absent.
        let mut spec2 = SpecMd::default();
        let n3 = populate_glossary(&corpus, &mut spec2, None).unwrap();
        assert_eq!(n3, 0);
    }

    #[test]
    fn populate_clock_domains_reads_table_shards() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tables").join("clock_domains").join("000.toml"),
            "schema_version = 1\n\
             source_page_range = [2, 2]\n\
             \n\
             [[rows]]\n\
             name = \"core_clk\"\n\
             frequency = \"1 GHz\"\n\
             source = \"PLL0\"\n\
             description = \"main CPU clock\"\n\
             \n\
             [[rows]]\n\
             name = \"bus_clk\"\n\
             frequency = \"500 MHz\"\n\
             source = \"PLL1\"\n\
             description = \"AHB / APB clock\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_clock_domains(&corpus, &mut spec).unwrap();
        assert_eq!(n, 2);
        assert_eq!(spec.clock_domains.len(), 2);
        assert_eq!(spec.clock_domains[0].name, "core_clk");
        assert_eq!(spec.clock_domains[0].frequency, "1 GHz");
        // Idempotent.
        let n2 = populate_clock_domains(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn populate_performance_counters_reads_pmu_shards() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tables").join("pmu").join("000.toml"),
            "schema_version = 1\n\
             source_page_range = [70, 70]\n\
             \n\
             [[rows]]\n\
             id = \"cycles\"\n\
             name = \"Cycle count\"\n\
             description = \"total cycles\"\n\
             csr_address = \"mcycle\"\n\
             \n\
             [[rows]]\n\
             id = \"icache_miss\"\n\
             name = \"I-cache miss\"\n\
             description = \"\"\n\
             csr_address = \"\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_performance_counters(&corpus, &mut spec).unwrap();
        assert_eq!(n, 2);
        assert_eq!(spec.performance_counters[0].id, "cycles");
        assert_eq!(spec.performance_counters[0].csr_address, "mcycle");
        let n2 = populate_performance_counters(&corpus, &mut spec).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn new_populate_dirs_absent_returns_zero() {
        // None of the new directories exist — every populate returns
        // 0 without error.
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        let mut spec = SpecMd::default();
        assert_eq!(populate_csrs(&corpus, &mut spec).unwrap(), 0);
        assert_eq!(populate_clock_domains(&corpus, &mut spec).unwrap(), 0);
        assert_eq!(populate_power_domains(&corpus, &mut spec).unwrap(), 0);
        assert_eq!(populate_reset_domains(&corpus, &mut spec).unwrap(), 0);
        assert_eq!(populate_security_boundaries(&corpus, &mut spec).unwrap(), 0);
        assert_eq!(
            populate_numerical_conventions(&corpus, &mut spec).unwrap(),
            0
        );
        assert_eq!(
            populate_performance_counters(&corpus, &mut spec).unwrap(),
            0
        );
        assert_eq!(populate_glossary(&corpus, &mut spec, None).unwrap(), 0);
    }

    #[test]
    fn populate_blocks_format_driven_uses_descriptor_block_name() {
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        // Shard's `stage` is a short heading-derived label; the
        // descriptor's `block_name` is the canonical long form.
        write(
            &corpus.join("tables").join("signals").join("000-if.toml"),
            "schema_version = 1\n\
             table_kind = \"signal_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [12, 13]\n\
             stage = \"IF\"\n\
             breadcrumb = [\"Pipeline\", \"IF\"]\n\
             \n\
             [[rows]]\n\
             name = \"if_valid\"\n\
             direction = \"out\"\n\
             peer = \"PD\"\n\
             description = \"valid flag\"\n\
             \n\
             [[rows]]\n\
             name = \"if_pc\"\n\
             direction = \"out\"\n\
             peer = \"PD\"\n\
             description = \"program counter\"\n",
        );

        let mut fmt = empty_format();
        fmt.section_roles.push(SectionRoleEntry {
            heading: "Instruction Fetch (IF)".into(),
            page: 12,
            line: 1,
            font_size: 14.0,
            font_weight: FontWeight::Bold,
            level: 2,
            spec_md_role: SpecMdRole::Block {
                block_name: "Instruction Fetch (IF)".into(),
            },
            layer: FmtLayer::Micro,
            rationale: String::new(),
        });
        fmt.tables.push(FmtTableEntry {
            id: "tbl_001".into(),
            page: 12,
            first_line: 5,
            row_count: 2,
            col_count: 4,
            kind: TableKind::SignalTable,
            spec_md_target: TableTarget::BlockSignals {
                block_name: "Instruction Fetch (IF)".into(),
            },
            column_map: vec![
                ColumnMapping {
                    source: "Signal".into(),
                    canonical: "name".into(),
                },
                ColumnMapping {
                    source: "Direction".into(),
                    canonical: "direction".into(),
                },
            ],
            wrap_strategy: WrapStrategy::SingleRow,
            rationale: String::new(),
        });

        let mut spec = SpecMd::default();
        let n = populate_blocks_with_format(&corpus, &mut spec, Some(&fmt)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(spec.blocks.len(), 1);
        let block = &spec.blocks[0];
        // Block name is the descriptor's canonical form, not the
        // shard's heading-derived `stage`.
        assert_eq!(block.name, "Instruction Fetch (IF)");
        assert_eq!(block.layer, SpecLayer::Micro);
        // Signal-role suffix heuristic kicked in for `if_valid`.
        let valid = block.signals.iter().find(|s| s.name == "if_valid").unwrap();
        assert_eq!(valid.role, SignalRole::Control);
        let pc = block.signals.iter().find(|s| s.name == "if_pc").unwrap();
        assert_eq!(pc.role, SignalRole::Unknown);
        // Idempotent under the format-driven path.
        let n2 = populate_blocks_with_format(&corpus, &mut spec, Some(&fmt)).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn populate_blocks_no_format_falls_back_to_stage_label() {
        // Smoke test: when `format` is None, populate_blocks behaves
        // exactly like Phase 6 (block name = shard `stage`, layer
        // Unknown, role inferred from name suffix only).
        let tmp = make_project();
        let corpus = corpus_root(tmp.path());
        write(
            &corpus.join("tables").join("signals").join("000-fetch.toml"),
            "schema_version = 1\n\
             table_kind = \"signal_table\"\n\
             source_chunk_id = \"c1\"\n\
             source_page_range = [3, 3]\n\
             stage = \"fetch\"\n\
             \n\
             [[rows]]\n\
             name = \"pc\"\n\
             direction = \"out\"\n\
             peer = \"decode\"\n\
             description = \"program counter\"\n",
        );
        let mut spec = SpecMd::default();
        let n = populate_blocks_with_format(&corpus, &mut spec, None).unwrap();
        assert_eq!(n, 1);
        assert_eq!(spec.blocks[0].name, "fetch");
        assert_eq!(spec.blocks[0].layer, SpecLayer::Unknown);
    }

    #[test]
    fn run_with_format_none_matches_run_smoke() {
        // Smoke test: run_with_format(None) reproduces run()'s
        // behaviour on a minimal source-driven corpus.
        let tmp = make_project();
        let project = tmp.path();
        write(
            &manifest_path(project),
            "schema_version = 1\n\
             source_kind = \"pdf\"\n\
             source_path = \"docs/main.pdf\"\n",
        );
        let corpus = corpus_root(project);
        write(
            &corpus.join("tables").join("signals").join("000-if.toml"),
            "schema_version = 1\n\
             table_kind = \"signal_table\"\n\
             source_chunk_id = \"c\"\n\
             source_page_range = [12, 12]\n\
             stage = \"IF\"\n\
             \n\
             [[rows]]\n\
             name = \"if_pc\"\n\
             direction = \"out\"\n\
             peer = \"PD\"\n\
             description = \"pc\"\n",
        );
        let mut a = SpecMd::default();
        let report_a = run(project, &mut a).unwrap();
        let mut b = SpecMd::default();
        let report_b = run_with_format(project, &mut b, None).unwrap();
        assert_eq!(report_a.blocks, report_b.blocks);
        assert_eq!(a.blocks.len(), b.blocks.len());
        assert_eq!(a.blocks[0].name, b.blocks[0].name);
        // None of the new sections populate without their dirs.
        assert_eq!(report_b.csrs, 0);
        assert_eq!(report_b.glossary, 0);
        assert_eq!(report_b.clock_domains, 0);
    }
}
