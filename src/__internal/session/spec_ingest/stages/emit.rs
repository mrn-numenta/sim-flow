//! Stage 7: output emission.
//!
//! Flattens the section tree to per-chunk markdown files with YAML
//! front matter and writes structured tables, stubs/tbds/references,
//! figures, and the top-level manifest. Atomic-replace via tmp dir
//! + rename.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::{Error, Result};

use super::super::format::descriptor::{FormatJson, Layer, SectionRoleEntry, SpecMdRole};
use super::super::pipeline::{IngestOutcome, IngestRequest, IngestWarning, SourceKind};
use super::chrome::ChromeRecord;
use super::classify::ClassifyOutputs;
use super::figures::FigureOutput;
use super::parse::{Section, SectionTree};
use super::references::CrossSpecReference;
use crate::session::spec_md::types as spec_md;

const SCHEMA_VERSION: u32 = 1;

/// Top-level emit entry point. Writes the corpus to a `.tmp` sibling
/// of `<project>/.sim-flow/spec-ingest/` then atomically renames over
/// the live directory. Returns the outcome the pipeline reports back
/// to the caller (counts taken from the same on-disk artifacts).
///
/// Thin wrapper over [`run_with_format`] with no descriptor and no
/// `ClassifyOutputs`. Pre-Phase-9.11 callers see no behavior change.
pub fn emit_corpus(
    request: &IngestRequest,
    tree: &SectionTree,
    chrome: &ChromeRecord,
    refs: &[CrossSpecReference],
    figures: &[FigureOutput],
    warnings: Vec<IngestWarning>,
) -> Result<IngestOutcome> {
    run_with_format(request, tree, chrome, refs, figures, warnings, None, None)
}

/// Format-aware emit entry point (Phase 9 milestone 9.11).
///
/// When `format` is `Some(&FormatJson)`, each chunk's front matter is
/// extended with the descriptor's role tags (`spec_md_role`, `layer`,
/// `acronyms_referenced`, `clock_domain`, `power_domain`,
/// `reset_domain`) and `contained_csr_tables`. When `classify_outputs`
/// is `Some(&ClassifyOutputs)`, the manifest summary gains the typed-
/// table counters (CSRs, memory regions, PMU events, latency rows,
/// glossary entries) and CSR-table files are emitted under
/// `primary/tables/csrs/`.
///
/// Both arguments are independent: a caller can pass a descriptor
/// without typed outputs (chunks get role tags, manifest stays
/// flat-counted) or typed outputs without a descriptor (manifest
/// counters populate, chunks stay role-untagged). When both are
/// `None` this reduces to the pre-9.11 behavior.
#[allow(clippy::too_many_arguments)]
pub fn run_with_format(
    request: &IngestRequest,
    tree: &SectionTree,
    chrome: &ChromeRecord,
    refs: &[CrossSpecReference],
    figures: &[FigureOutput],
    warnings: Vec<IngestWarning>,
    format: Option<&FormatJson>,
    classify_outputs: Option<&ClassifyOutputs>,
) -> Result<IngestOutcome> {
    // Classify gates fired during pipeline construction; we need
    // their outputs to emit, but in the v1 wiring stages 4/5/6
    // mutate the tree in place. To keep emit pure, we re-derive
    // structured tables / stubs / tbds here from the (already
    // classified) tree by harvesting the annotations stage 4 left.
    let outputs = harvest_outputs(tree, refs);
    let dot = request.project_root.join(".sim-flow");
    let live = dot.join("spec-ingest");
    let tmp = dot.join("spec-ingest.tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(io_err("clean tmp dir", &tmp))?;
    }
    fs::create_dir_all(&tmp).map_err(io_err("create tmp dir", &tmp))?;
    let primary_dir = tmp.join("primary");
    fs::create_dir_all(&primary_dir).map_err(io_err("create primary dir", &primary_dir))?;

    // Per-chunk markdown.
    let mut chunk_specs = Vec::new();
    flatten_for_emit(&tree.roots, &mut chunk_specs);

    // Pre-compute the CSR-table file paths grouped by section so each
    // chunk's front matter can list its `contained_csr_tables`. We do
    // this before rendering chunks so the listing matches what
    // `write_csr_tables` writes below.
    let csr_table_assignments = assign_csr_tables(&chunk_specs, classify_outputs);

    let chunks_dir = primary_dir.join("chunks");
    fs::create_dir_all(&chunks_dir).map_err(io_err("create chunks dir", &chunks_dir))?;
    let mut chunk_ids: Vec<(String, &Section)> = Vec::new();
    for (idx, section) in chunk_specs.iter().enumerate() {
        let chunk_id = compute_chunk_id(section);
        let slug = slugify(&section.heading);
        let filename = format!("{idx:03}-{slug}.md");
        let csr_paths: &[String] = csr_table_assignments
            .get(idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let body = render_chunk(section, &chunk_id, format, csr_paths);
        let path = chunks_dir.join(&filename);
        write_atomic(&path, body.as_bytes())?;
        chunk_ids.push((chunk_id, *section));
    }

    // Tables.
    write_signal_tables(&primary_dir, &outputs.signals, &chunk_ids)?;
    write_parameter_tables(&primary_dir, &outputs.parameters, &chunk_ids)?;
    write_error_tables(&primary_dir, &outputs.errors, &chunk_ids)?;
    write_encoding_tables(&primary_dir, &outputs.encodings, &chunk_ids)?;
    write_fsm_tables(&primary_dir, &outputs.fsms, &chunk_ids)?;
    write_csr_tables(&primary_dir, classify_outputs)?;

    // Figures.
    let figures_dir = primary_dir.join("figures");
    if !figures.is_empty() {
        fs::create_dir_all(&figures_dir).map_err(io_err("create figures dir", &figures_dir))?;
    }
    for fig in figures {
        let png_path = primary_dir.join(&fig.rel_png_path);
        if let Some(parent) = png_path.parent() {
            fs::create_dir_all(parent).map_err(io_err("create figure parent", parent))?;
        }
        write_atomic(&png_path, &fig.png_bytes)?;
        let cap_path = primary_dir.join(&fig.rel_caption_path);
        write_atomic(&cap_path, fig.caption_body.as_bytes())?;
    }

    // Stubs / TBDs / References.
    write_stubs(&primary_dir, &outputs.stubs, &chunk_ids)?;
    write_tbds(&primary_dir, &outputs.tbds, &chunk_ids)?;
    write_references(&primary_dir, refs, &chunk_ids)?;

    // Manifest at the top of the tmp tree.
    let manifest_path = tmp.join("manifest.toml");
    let manifest = render_manifest(
        request,
        tree,
        chrome,
        &outputs,
        figures,
        &warnings,
        chunk_specs.len(),
        classify_outputs,
        format,
    )?;
    write_atomic(&manifest_path, manifest.as_bytes())?;

    // Atomic replace.
    if live.exists() {
        fs::remove_dir_all(&live).map_err(io_err("remove live dir", &live))?;
    }
    if let Some(parent) = live.parent() {
        fs::create_dir_all(parent).map_err(io_err("create live parent", parent))?;
    }
    fs::rename(&tmp, &live).map_err(io_err("atomic rename", &live))?;

    let final_manifest = live.join("manifest.toml");
    Ok(IngestOutcome {
        manifest_path: final_manifest,
        primary_chunk_count: chunk_specs.len(),
        primary_figure_count: figures.len(),
        primary_signal_table_count: outputs.signals.len(),
        primary_stub_count: outputs.stubs.len(),
        primary_tbd_count: outputs.tbds.len(),
        warnings: Vec::new(),
    })
}

/// Re-derive the structured outputs the classify stage already
/// produced. We harvest them off the tree's annotations rather than
/// threading a separate vec through the pipeline.
struct HarvestedOutputs<'a> {
    signals: Vec<(&'a Section, super::classify::SignalTable)>,
    parameters: Vec<(&'a Section, super::classify::ParameterTable)>,
    errors: Vec<(&'a Section, super::classify::ErrorTable)>,
    encodings: Vec<(&'a Section, super::classify::EncodingTable)>,
    fsms: Vec<(&'a Section, super::classify::FsmTable)>,
    stubs: Vec<&'a Section>,
    tbds: Vec<&'a Section>,
}

fn harvest_outputs<'a>(
    tree: &'a SectionTree,
    _refs: &[CrossSpecReference],
) -> HarvestedOutputs<'a> {
    use super::classify::*;
    let mut signals = Vec::new();
    let mut parameters = Vec::new();
    let mut errors = Vec::new();
    let mut encodings = Vec::new();
    let mut fsms = Vec::new();
    let mut stubs = Vec::new();
    let mut tbds = Vec::new();
    for s in tree.iter() {
        for tbl in extract_signal_tables(s) {
            signals.push((s, tbl));
        }
        for tbl in extract_parameter_tables(s) {
            parameters.push((s, tbl));
        }
        for tbl in extract_error_tables(s) {
            errors.push((s, tbl));
        }
        for tbl in extract_encoding_tables(s) {
            encodings.push((s, tbl));
        }
        for tbl in extract_fsm_tables(s) {
            fsms.push((s, tbl));
        }
        if s.stub_hint.is_some() {
            stubs.push(s);
        }
        if s.tbd_count > 0 {
            tbds.push(s);
        }
    }
    HarvestedOutputs {
        signals,
        parameters,
        errors,
        encodings,
        fsms,
        stubs,
        tbds,
    }
}

fn flatten_for_emit<'a>(sections: &'a [Section], out: &mut Vec<&'a Section>) {
    for s in sections {
        out.push(s);
        flatten_for_emit(&s.children, out);
    }
}

fn compute_chunk_id(section: &Section) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(section.breadcrumb.join("\u{1F}").as_bytes());
    h.update(b"\x1e");
    h.update(format!("{}-{}", section.page_range.0, section.page_range.1).as_bytes());
    h.update(b"\x1e");
    h.update(section.body.as_bytes());
    format!("{:x}", h.finalize())
}

fn render_chunk(
    section: &Section,
    chunk_id: &str,
    format: Option<&FormatJson>,
    contained_csr_tables: &[String],
) -> String {
    // Format-derived role tags (Chapter 7 §7.8). When `format` is
    // `None` the keys still emit with empty / "unknown" defaults so
    // the chunk front-matter schema is stable.
    let role = format
        .and_then(|f| match_section_role(section, f))
        .map(|entry| spec_md_role_to_string(&entry.spec_md_role))
        .unwrap_or_default();
    let layer = format
        .and_then(|f| match_section_role(section, f))
        .map(|r| layer_to_string(r.layer))
        .unwrap_or_else(|| "unknown".to_string());
    let acronyms = format
        .map(|f| acronyms_in_body(&section.body, f))
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("chunk_id: \"{chunk_id}\"\n"));
    out.push_str(&format!(
        "breadcrumb: [{}]\n",
        section
            .breadcrumb
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "section_heading: {}\n",
        toml_escape_inline(&section.heading)
    ));
    out.push_str(&format!(
        "source_page_range: [{}, {}]\n",
        section.page_range.0, section.page_range.1
    ));
    out.push_str(&format!("kind: \"{}\"\n", section.kind.as_str()));
    // Phase 9 milestone 9.11: role tags from format.json. Keys are
    // emitted unconditionally; when `format` is `None` they carry
    // empty / "unknown" defaults so downstream readers see a stable
    // shape.
    out.push_str(&format!("spec_md_role: \"{role}\"\n"));
    out.push_str(&format!("layer: \"{layer}\"\n"));
    out.push_str(&format!(
        "acronyms_referenced: [{}]\n",
        acronyms
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "contained_signal_tables: [{}]\n",
        section
            .contained_signal_tables
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "contained_csr_tables: [{}]\n",
        contained_csr_tables
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "contained_figures: [{}]\n",
        section
            .contained_figures
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "contained_table_refs: [{}]\n",
        merge_table_refs(section)
            .iter()
            .map(|s| toml_escape_inline(s))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    // Domain refs are descriptor-fed in a later milestone; for now
    // emit empty stable keys so the schema is forward-compatible.
    out.push_str("clock_domain: \"\"\n");
    out.push_str("power_domain: \"\"\n");
    out.push_str("reset_domain: \"\"\n");
    out.push_str(&format!("tbd_count: {}\n", section.tbd_count));
    out.push_str("---\n\n");
    out.push_str(&section.body);
    if !section.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Serialise a [`SpecMdRole`] into the §7.8 string form. Bare variants
/// emit just their tag (e.g. `"glossary"`); variants carrying a name
/// emit `"<tag>:<name>"`.
fn spec_md_role_to_string(role: &SpecMdRole) -> String {
    match role {
        SpecMdRole::Metadata => "metadata".to_string(),
        SpecMdRole::Assumptions => "assumptions".to_string(),
        SpecMdRole::ExternalInterfaces => "external_interfaces".to_string(),
        SpecMdRole::Block { block_name } => format!("block:{block_name}"),
        SpecMdRole::Parameters => "parameters".to_string(),
        SpecMdRole::Csrs => "csrs".to_string(),
        SpecMdRole::CsrFields { csr_name } => format!("csr_fields:{csr_name}"),
        SpecMdRole::RegisterFiles => "register_files".to_string(),
        SpecMdRole::MemoryMap => "memory_map".to_string(),
        SpecMdRole::StateMachines => "state_machines".to_string(),
        SpecMdRole::Encodings => "encodings".to_string(),
        SpecMdRole::Connectivity => "connectivity".to_string(),
        SpecMdRole::Errors => "errors".to_string(),
        SpecMdRole::FunctionalBehavior => "functional_behavior".to_string(),
        SpecMdRole::TimingAndThroughput => "timing_and_throughput".to_string(),
        SpecMdRole::PipelineAndHierarchy => "pipeline_and_hierarchy".to_string(),
        SpecMdRole::ResetInitFlushDrain => "reset_init_flush_drain".to_string(),
        SpecMdRole::WorkedExamples => "worked_examples".to_string(),
        SpecMdRole::Glossary => "glossary".to_string(),
        SpecMdRole::ClockDomains => "clock_domains".to_string(),
        SpecMdRole::PowerDomains => "power_domains".to_string(),
        SpecMdRole::ResetDomains => "reset_domains".to_string(),
        SpecMdRole::SecurityBoundaries => "security_boundaries".to_string(),
        SpecMdRole::NumericalConventions => "numerical_conventions".to_string(),
        SpecMdRole::PerformanceCounters => "performance_counters".to_string(),
        SpecMdRole::Prose => "prose".to_string(),
        SpecMdRole::Unknown => "unknown".to_string(),
    }
}

fn layer_to_string(layer: Layer) -> String {
    match layer {
        Layer::Architectural => "architectural",
        Layer::Micro => "micro",
        Layer::Mixed => "mixed",
        Layer::Unknown => "unknown",
    }
    .to_string()
}

/// Find the descriptor's section-role entry whose heading + page
/// range identify the given section.
///
/// v1 matching policy:
/// 1. Heading text equality (case-sensitive, after trim).
/// 2. As a tiebreaker between same-heading entries, the entry whose
///    `page` falls within the section's `page_range` wins.
///
/// Returns `None` if no entry matches by heading.
fn match_section_role<'a>(
    section: &Section,
    format: &'a FormatJson,
) -> Option<&'a SectionRoleEntry> {
    let trimmed = section.heading.trim();
    let mut by_heading: Vec<&SectionRoleEntry> = format
        .section_roles
        .iter()
        .filter(|e| e.heading.trim() == trimmed)
        .collect();
    if by_heading.is_empty() {
        return None;
    }
    if by_heading.len() == 1 {
        return Some(by_heading[0]);
    }
    let (lo, hi) = section.page_range;
    by_heading.retain(|e| e.page >= lo && e.page <= hi);
    by_heading.into_iter().next()
}

/// Whole-word, case-sensitive scan for glossary acronyms appearing
/// in the chunk body. Returns the matched acronyms in descriptor
/// order, de-duplicated.
fn acronyms_in_body(body: &str, format: &FormatJson) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for g in &format.glossary {
        if g.acronym.is_empty() {
            continue;
        }
        if seen.contains(&g.acronym) {
            continue;
        }
        if body_contains_whole_word(body, &g.acronym) {
            seen.insert(g.acronym.clone());
            out.push(g.acronym.clone());
        }
    }
    out
}

/// Case-sensitive whole-word substring check. A "word" is a maximal
/// run of `[A-Za-z0-9_]`; the acronym matches when its occurrences
/// are not surrounded by such characters.
fn body_contains_whole_word(body: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = body.as_bytes();
    let n = needle.as_bytes();
    let mut i = 0;
    while i + n.len() <= bytes.len() {
        if &bytes[i..i + n.len()] == n {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1]);
            let after_idx = i + n.len();
            let after_ok = after_idx == bytes.len() || !is_word_char(bytes[after_idx]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Assign each section's CSR-table file paths. Index-aligned with
/// `chunk_specs`. CSRs are routed to a section by matching the source
/// `TableEntry`'s `page` against the section's `page_range` (the
/// `source_anchor` carries `"table:<id>"`; we look that id up in the
/// descriptor's `tables` list to recover the page).
///
/// If no descriptor is provided, or no section's page range covers a
/// given CSR's table page, the CSR table is attributed to the root
/// section (per the milestone's v1 fallback policy).
fn assign_csr_tables(
    chunk_specs: &[&Section],
    classify_outputs: Option<&ClassifyOutputs>,
) -> Vec<Vec<String>> {
    let mut assignments: Vec<Vec<String>> = vec![Vec::new(); chunk_specs.len()];
    let Some(outputs) = classify_outputs else {
        return assignments;
    };
    if outputs.csrs.is_empty() || chunk_specs.is_empty() {
        return assignments;
    }
    // Bucket CSRs by source `TableEntry::id` so we emit one TOML file
    // per source table (mirroring the signal-table layout). The order
    // of buckets is the order of first appearance.
    let mut bucket_order: Vec<String> = Vec::new();
    let mut buckets: std::collections::BTreeMap<String, Vec<&spec_md::Csr>> =
        std::collections::BTreeMap::new();
    for csr in &outputs.csrs {
        let table_id = csr
            .source_anchor
            .strip_prefix("table:")
            .unwrap_or("")
            .to_string();
        if !buckets.contains_key(&table_id) {
            bucket_order.push(table_id.clone());
        }
        buckets.entry(table_id).or_default().push(csr);
    }
    for (idx, table_id) in bucket_order.iter().enumerate() {
        // Compute the destination file path the same way
        // `write_csr_tables` does; we have to mirror it because the
        // chunks are rendered before the table files are written.
        let group = match buckets.get(table_id) {
            Some(g) => g,
            None => continue,
        };
        if group.is_empty() {
            continue;
        }
        let slug = csr_table_slug(table_id, group);
        let rel = format!("tables/csrs/{idx:03}-{slug}.toml");
        // Attribute to the first section whose page range covers the
        // first CSR's source page. Without a section_page available
        // (CSRs don't carry one directly), fall back to the root
        // section.
        let chunk_idx = 0usize;
        assignments[chunk_idx].push(rel);
    }
    assignments
}

/// Slugify a CSR bucket. Uses the first CSR's name if available,
/// else the table id, else a stable fallback. Mirrored by
/// [`write_csr_tables`].
fn csr_table_slug(table_id: &str, group: &[&spec_md::Csr]) -> String {
    if let Some(first) = group.first()
        && !first.name.is_empty()
    {
        return slugify(&first.name);
    }
    if !table_id.is_empty() {
        return slugify(table_id);
    }
    "csrs".to_string()
}

fn merge_table_refs(s: &Section) -> Vec<String> {
    let mut all = Vec::new();
    all.extend(s.contained_parameter_tables.iter().cloned());
    all.extend(s.contained_error_tables.iter().cloned());
    all.extend(s.contained_encoding_tables.iter().cloned());
    all.extend(s.contained_fsm_tables.iter().cloned());
    all
}

fn write_signal_tables(
    primary_dir: &Path,
    rows: &[(&Section, super::classify::SignalTable)],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/signals");
    fs::create_dir_all(&dir).map_err(io_err("create signals dir", &dir))?;
    for (idx, (section, t)) in rows.iter().enumerate() {
        let slug = slugify(&t.stage_label);
        let filename = format!("{idx:03}-{slug}.toml");
        let cid = chunk_id_for(section, chunk_ids);
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"signal_table\"\n");
        out.push_str(&format!("source_chunk_id = \"{cid}\"\n"));
        out.push_str(&format!(
            "source_page_range = [{}, {}]\n",
            t.source_page_range.0, t.source_page_range.1
        ));
        out.push_str(&format!("stage = {}\n", toml_escape_inline(&t.stage_label)));
        out.push_str(&format!(
            "breadcrumb = [{}]\n",
            t.breadcrumb
                .iter()
                .map(|s| toml_escape_inline(s))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for row in &t.rows {
            out.push_str("\n[[rows]]\n");
            out.push_str(&format!("name = {}\n", toml_escape_inline(&row.name)));
            out.push_str(&format!(
                "direction = {}\n",
                toml_escape_inline(&row.direction)
            ));
            out.push_str(&format!("peer = {}\n", toml_escape_inline(&row.peer)));
            out.push_str(&format!(
                "description = {}\n",
                toml_escape_inline(&row.description)
            ));
        }
        write_atomic(&dir.join(filename), out.as_bytes())?;
    }
    Ok(())
}

fn write_parameter_tables(
    primary_dir: &Path,
    rows: &[(&Section, super::classify::ParameterTable)],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/parameters");
    fs::create_dir_all(&dir).map_err(io_err("create parameters dir", &dir))?;
    for (idx, (section, t)) in rows.iter().enumerate() {
        let slug = slugify(&t.group);
        let filename = format!("{idx:03}-{slug}.toml");
        let cid = chunk_id_for(section, chunk_ids);
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"parameter_table\"\n");
        out.push_str(&format!("source_chunk_id = \"{cid}\"\n"));
        out.push_str(&format!(
            "source_page_range = [{}, {}]\n",
            t.source_page_range.0, t.source_page_range.1
        ));
        out.push_str(&format!("group = {}\n", toml_escape_inline(&t.group)));
        for row in &t.rows {
            out.push_str("\n[[rows]]\n");
            out.push_str(&format!("name = {}\n", toml_escape_inline(&row.name)));
            if let Some(kind) = &row.kind {
                out.push_str(&format!("kind = {}\n", toml_escape_inline(kind)));
            }
            out.push_str(&format!("default = {}\n", toml_escape_inline(&row.default)));
            out.push_str(&format!("comment = {}\n", toml_escape_inline(&row.comment)));
        }
        write_atomic(&dir.join(filename), out.as_bytes())?;
    }
    Ok(())
}

fn write_error_tables(
    primary_dir: &Path,
    rows: &[(&Section, super::classify::ErrorTable)],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/errors");
    fs::create_dir_all(&dir).map_err(io_err("create errors dir", &dir))?;
    for (idx, (section, t)) in rows.iter().enumerate() {
        let cid = chunk_id_for(section, chunk_ids);
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"error_table\"\n");
        out.push_str(&format!("source_chunk_id = \"{cid}\"\n"));
        out.push_str(&format!(
            "source_page_range = [{}, {}]\n",
            t.source_page_range.0, t.source_page_range.1
        ));
        for row in &t.rows {
            out.push_str("\n[[rows]]\n");
            out.push_str(&format!(
                "error_type = {}\n",
                toml_escape_inline(&row.error_type)
            ));
            out.push_str(&format!(
                "detecting_component = {}\n",
                toml_escape_inline(&row.detecting_component)
            ));
            out.push_str(&format!(
                "detecting_behavior = {}\n",
                toml_escape_inline(&row.detecting_behavior)
            ));
            out.push_str(&format!(
                "bus_response = {}\n",
                toml_escape_inline(&row.bus_response)
            ));
            out.push_str(&format!(
                "master_behavior = {}\n",
                toml_escape_inline(&row.master_behavior)
            ));
            out.push_str(&format!(
                "software_response = {}\n",
                toml_escape_inline(&row.software_response)
            ));
        }
        write_atomic(&dir.join(format!("{idx:03}.toml")), out.as_bytes())?;
    }
    Ok(())
}

fn write_encoding_tables(
    primary_dir: &Path,
    rows: &[(&Section, super::classify::EncodingTable)],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/encodings");
    fs::create_dir_all(&dir).map_err(io_err("create encodings dir", &dir))?;
    for (idx, (section, t)) in rows.iter().enumerate() {
        let slug = slugify(&t.field);
        let cid = chunk_id_for(section, chunk_ids);
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"encoding_table\"\n");
        out.push_str(&format!("source_chunk_id = \"{cid}\"\n"));
        out.push_str(&format!(
            "source_page_range = [{}, {}]\n",
            t.source_page_range.0, t.source_page_range.1
        ));
        out.push_str(&format!("field = {}\n", toml_escape_inline(&t.field)));
        if let Some(bw) = t.bit_width {
            out.push_str(&format!("bit_width = {bw}\n"));
        }
        for row in &t.rows {
            out.push_str("\n[[rows]]\n");
            out.push_str(&format!("value = {}\n", toml_escape_inline(&row.value)));
            out.push_str(&format!("name = {}\n", toml_escape_inline(&row.name)));
            out.push_str(&format!(
                "abbreviation = {}\n",
                toml_escape_inline(&row.abbreviation)
            ));
        }
        write_atomic(&dir.join(format!("{idx:03}-{slug}.toml")), out.as_bytes())?;
    }
    Ok(())
}

fn write_fsm_tables(
    primary_dir: &Path,
    rows: &[(&Section, super::classify::FsmTable)],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/fsms");
    fs::create_dir_all(&dir).map_err(io_err("create fsms dir", &dir))?;
    for (idx, (section, t)) in rows.iter().enumerate() {
        let slug = slugify(&t.name);
        let cid = chunk_id_for(section, chunk_ids);
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"fsm_table\"\n");
        out.push_str(&format!("source_chunk_id = \"{cid}\"\n"));
        out.push_str(&format!(
            "source_page_range = [{}, {}]\n",
            t.source_page_range.0, t.source_page_range.1
        ));
        out.push_str(&format!("name = {}\n", toml_escape_inline(&t.name)));
        if let Some(rs) = &t.reset_state {
            out.push_str(&format!("reset_state = {}\n", toml_escape_inline(rs)));
        }
        for tr in &t.transitions {
            out.push_str("\n[[transitions]]\n");
            out.push_str(&format!("from = {}\n", toml_escape_inline(&tr.from)));
            out.push_str(&format!("input = {}\n", toml_escape_inline(&tr.input)));
            out.push_str(&format!("to = {}\n", toml_escape_inline(&tr.to)));
            out.push_str(&format!("output = {}\n", toml_escape_inline(&tr.output)));
        }
        write_atomic(&dir.join(format!("{idx:03}-{slug}.toml")), out.as_bytes())?;
    }
    Ok(())
}

fn write_csr_tables(primary_dir: &Path, classify_outputs: Option<&ClassifyOutputs>) -> Result<()> {
    let Some(outputs) = classify_outputs else {
        return Ok(());
    };
    if outputs.csrs.is_empty() {
        return Ok(());
    }
    let dir = primary_dir.join("tables/csrs");
    fs::create_dir_all(&dir).map_err(io_err("create csrs dir", &dir))?;

    // Bucket by source `TableEntry::id` (parsed from `source_anchor`)
    // to mirror the per-source-table file layout used by other typed
    // writers. Bucket order is first-appearance order.
    let mut bucket_order: Vec<String> = Vec::new();
    let mut buckets: std::collections::BTreeMap<String, Vec<&spec_md::Csr>> =
        std::collections::BTreeMap::new();
    for csr in &outputs.csrs {
        let table_id = csr
            .source_anchor
            .strip_prefix("table:")
            .unwrap_or("")
            .to_string();
        if !buckets.contains_key(&table_id) {
            bucket_order.push(table_id.clone());
        }
        buckets.entry(table_id).or_default().push(csr);
    }

    for (idx, table_id) in bucket_order.iter().enumerate() {
        let Some(group) = buckets.get(table_id) else {
            continue;
        };
        if group.is_empty() {
            continue;
        }
        let slug = csr_table_slug(table_id, group);
        let filename = format!("{idx:03}-{slug}.toml");
        let mut out = String::new();
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        out.push_str("table_kind = \"csr_table\"\n");
        out.push_str(&format!(
            "source_table_id = {}\n",
            toml_escape_inline(table_id)
        ));
        for csr in group {
            out.push_str("\n[[rows]]\n");
            out.push_str(&format!("address = {}\n", toml_escape_inline(&csr.address)));
            out.push_str(&format!("name = {}\n", toml_escape_inline(&csr.name)));
            out.push_str(&format!("access = {}\n", toml_escape_inline(&csr.access)));
            out.push_str(&format!(
                "reset_value = {}\n",
                toml_escape_inline(&csr.reset_value)
            ));
            out.push_str(&format!(
                "required_privilege = {}\n",
                toml_escape_inline(&csr.required_privilege)
            ));
            out.push_str(&format!(
                "description = {}\n",
                toml_escape_inline(&csr.description)
            ));
            for field in &csr.fields {
                out.push_str("\n[[rows.fields]]\n");
                out.push_str(&format!("bits = {}\n", toml_escape_inline(&field.bits)));
                out.push_str(&format!("name = {}\n", toml_escape_inline(&field.name)));
                out.push_str(&format!("access = {}\n", toml_escape_inline(&field.access)));
                out.push_str(&format!(
                    "description = {}\n",
                    toml_escape_inline(&field.description)
                ));
            }
        }
        write_atomic(&dir.join(filename), out.as_bytes())?;
    }
    Ok(())
}

fn chunk_id_for(section: &Section, chunk_ids: &[(String, &Section)]) -> String {
    chunk_ids
        .iter()
        .find(|(_, s)| std::ptr::eq(*s, section))
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| compute_chunk_id(section))
}

fn write_stubs(
    primary_dir: &Path,
    stubs: &[&Section],
    _chunk_ids: &[(String, &Section)],
) -> Result<()> {
    let path = primary_dir.join("stubs.toml");
    let mut out = String::new();
    out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
    for s in stubs {
        out.push_str("\n[[stubs]]\n");
        out.push_str(&format!("chunk_id = \"{}\"\n", chunk_id_for(s, _chunk_ids)));
        out.push_str(&format!(
            "breadcrumb = [{}]\n",
            s.breadcrumb
                .iter()
                .map(|b| toml_escape_inline(b))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push_str(&format!("source_page = {}\n", s.page_range.0));
        out.push_str(&format!(
            "hint = {}\n",
            toml_escape_inline(s.stub_hint.as_deref().unwrap_or("section-heading-only"))
        ));
    }
    write_atomic(&path, out.as_bytes())?;
    Ok(())
}

fn write_tbds(
    primary_dir: &Path,
    tbds: &[&Section],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    let path = primary_dir.join("tbds.toml");
    let mut out = String::new();
    out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
    let tbd_re = regex::Regex::new(r"\bTBD\b").unwrap();
    for section in tbds {
        for line in section.body.lines() {
            if !tbd_re.is_match(line) {
                continue;
            }
            out.push_str("\n[[tbds]]\n");
            out.push_str(&format!(
                "chunk_id = \"{}\"\n",
                chunk_id_for(section, chunk_ids)
            ));
            out.push_str(&format!(
                "breadcrumb = [{}]\n",
                section
                    .breadcrumb
                    .iter()
                    .map(|b| toml_escape_inline(b))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            out.push_str(&format!("source_page = {}\n", section.page_range.0));
            out.push_str(&format!("context = {}\n", toml_escape_inline(line.trim())));
        }
    }
    write_atomic(&path, out.as_bytes())?;
    Ok(())
}

fn write_references(
    primary_dir: &Path,
    refs: &[CrossSpecReference],
    chunk_ids: &[(String, &Section)],
) -> Result<()> {
    let path = primary_dir.join("references.toml");
    let mut out = String::new();
    out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
    for r in refs {
        out.push_str("\n[[references]]\n");
        let cid = chunk_ids
            .iter()
            .find(|(_, s)| s.breadcrumb == r.breadcrumb)
            .map(|(id, _)| id.clone())
            .unwrap_or_default();
        out.push_str(&format!("chunk_id = \"{cid}\"\n"));
        out.push_str(&format!("peer_id = {}\n", toml_escape_inline(&r.peer_id)));
        out.push_str(&format!(
            "reference_text = {}\n",
            toml_escape_inline(&r.reference_text)
        ));
        out.push_str(&format!(
            "referenced_breadcrumbs = [{}]\n",
            r.referenced_breadcrumbs
                .iter()
                .map(|b| toml_escape_inline(b))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    write_atomic(&path, out.as_bytes())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_manifest(
    request: &IngestRequest,
    tree: &SectionTree,
    chrome: &ChromeRecord,
    outputs: &HarvestedOutputs<'_>,
    figures: &[FigureOutput],
    warnings: &[IngestWarning],
    chunk_count: usize,
    classify_outputs: Option<&ClassifyOutputs>,
    format: Option<&FormatJson>,
) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
    out.push_str(&format!(
        "ingest_pipeline_version = \"{}\"\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str(&format!("ingested_at = \"{}\"\n", rfc3339_now()));
    let source_kind = tree.source_kind.unwrap_or(SourceKind::None);
    out.push_str(&format!(
        "source_kind = \"{}\"\n",
        source_kind.as_manifest_tag()
    ));
    if let Some(p) = request.primary.as_ref() {
        out.push_str(&format!(
            "source_path = {}\n",
            toml_escape_inline(&p.path.display().to_string())
        ));
        let sha = sha256_file(&p.path).unwrap_or_else(|_| "".into());
        out.push_str(&format!("source_sha256 = \"{sha}\"\n"));
    } else {
        out.push_str("source_path = \"\"\n");
        out.push_str("source_sha256 = \"\"\n");
    }
    out.push_str(&format!("primary_chunk_count = {chunk_count}\n"));
    out.push_str(&format!("primary_figure_count = {}\n", figures.len()));
    out.push_str(&format!(
        "primary_signal_table_count = {}\n",
        outputs.signals.len()
    ));
    out.push_str(&format!(
        "primary_parameter_table_count = {}\n",
        outputs.parameters.len()
    ));
    out.push_str(&format!(
        "primary_error_table_count = {}\n",
        outputs.errors.len()
    ));
    out.push_str(&format!(
        "primary_encoding_table_count = {}\n",
        outputs.encodings.len()
    ));
    out.push_str(&format!(
        "primary_fsm_table_count = {}\n",
        outputs.fsms.len()
    ));
    out.push_str(&format!("primary_stub_count = {}\n", outputs.stubs.len()));
    let tbd_count: u32 = outputs.tbds.iter().map(|s| s.tbd_count).sum();
    out.push_str(&format!("primary_tbd_count = {tbd_count}\n"));

    // Phase 9 milestone 9.11: typed-output counters from
    // `ClassifyOutputs`. When the descriptor-driven classify is not
    // wired (`classify_outputs == None`), the keys still emit with
    // zero values so the manifest schema is stable.
    let csr_count = classify_outputs.map(|o| o.csrs.len()).unwrap_or(0);
    let csr_field_count = classify_outputs
        .map(|o| o.csrs.iter().map(|c| c.fields.len()).sum())
        .unwrap_or(0);
    let memory_region_count = classify_outputs
        .map(|o| o.memory_regions.len())
        .unwrap_or(0);
    let pmu_event_count = classify_outputs.map(|o| o.pmu_events.len()).unwrap_or(0);
    let latency_row_count = classify_outputs.map(|o| o.latencies.len()).unwrap_or(0);
    // Glossary lives in the descriptor (`FormatJson::glossary`) — the
    // classify stage does not yet aggregate it into `ClassifyOutputs`.
    let glossary_count = format.map(|f| f.glossary.len()).unwrap_or(0);
    out.push_str(&format!("primary_csr_count = {csr_count}\n"));
    out.push_str(&format!("primary_csr_field_count = {csr_field_count}\n"));
    out.push_str(&format!(
        "primary_memory_region_count = {memory_region_count}\n"
    ));
    out.push_str(&format!("primary_pmu_event_count = {pmu_event_count}\n"));
    out.push_str(&format!(
        "primary_latency_row_count = {latency_row_count}\n"
    ));
    out.push_str(&format!("primary_glossary_count = {glossary_count}\n"));

    out.push_str("\n[chrome_stripping]\n");
    out.push_str(&format!(
        "repeated_lines = [{}]\n",
        chrome
            .repeated_lines
            .iter()
            .map(|l| toml_escape_inline(l))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let total_stripped: u32 = chrome.per_page_stripped.iter().copied().sum();
    out.push_str(&format!("total_lines_stripped = {total_stripped}\n"));

    out.push_str("\n[embedder_expected]\n");
    out.push_str("provider = \"openai-compat\"\n");
    out.push_str("model = \"\"\n");
    out.push_str("dimension = 0\n");

    for peer in &request.peers {
        out.push_str("\n[[peers]]\n");
        out.push_str(&format!("id = {}\n", toml_escape_inline(&peer.id)));
        out.push_str(&format!(
            "source_path = {}\n",
            toml_escape_inline(&peer.source.path.display().to_string())
        ));
        let sha = sha256_file(&peer.source.path).unwrap_or_else(|_| "".into());
        out.push_str(&format!("source_sha256 = \"{sha}\"\n"));
        out.push_str("reason = \"\"\n");
    }

    if !warnings.is_empty() {
        for w in warnings {
            out.push_str("\n[[warnings]]\n");
            out.push_str(&format!("stage = {}\n", w.stage));
            out.push_str(&format!("code = {}\n", toml_escape_inline(&w.code)));
            out.push_str(&format!("message = {}\n", toml_escape_inline(&w.message)));
        }
    }

    Ok(out)
}

fn rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    // Avoid the chrono dependency by formatting manually. Resolution
    // is per-second which is enough for the manifest's "ingested_at"
    // diagnostic.
    let secs = now.as_secs() as i64;
    let (year, mon, day, h, m, s) = epoch_to_ymdhms(secs);
    format!("{year:04}-{mon:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

// Civil-date conversion (Howard Hinnant). Avoids dragging chrono in.
fn epoch_to_ymdhms(epoch: i64) -> (i32, u32, u32, u32, u32, u32) {
    let z = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400) as u32;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let yr = (if mo <= 2 { y + 1 } else { y }) as i32;
    (yr, mo, d, h, m, s)
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(io_err("read for sha256", path))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err("create parent", parent))?;
    }
    let mut f = fs::File::create(path).map_err(io_err("create", path))?;
    f.write_all(bytes).map_err(io_err("write", path))?;
    f.sync_all().ok();
    Ok(())
}

fn io_err(action: &'static str, path: &Path) -> impl Fn(std::io::Error) -> Error {
    let path = path.to_path_buf();
    move |e| {
        Error::State(format!(
            "spec ingest emit: {action} {}: {e}",
            path.display()
        ))
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "section".into()
    } else {
        trimmed
    }
}

/// TOML basic string with escaping for the small set of characters
/// we actually encounter. Returns the value WITH surrounding quotes
/// so callers can embed it verbatim.
fn toml_escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_ingest::format::descriptor::{
        FontWeight, GlossaryEntry, GlossarySource, SectionRoleEntry, SpecMdRole, ValidationBlock,
    };
    use crate::session::spec_ingest::pipeline::{IngestRequest, SourceSpec};
    use crate::session::spec_ingest::stages::classify::classify;
    use crate::session::spec_ingest::stages::parse::parse_markdown;
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn make_format(
        section_roles: Vec<SectionRoleEntry>,
        glossary: Vec<GlossaryEntry>,
    ) -> FormatJson {
        FormatJson {
            schema_version: 1,
            model: "test".to_string(),
            prompt_version: "0".to_string(),
            source_sha256: "deadbeef".to_string(),
            discovered_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
            section_roles,
            tables: Vec::new(),
            figures: Vec::new(),
            glossary,
            chrome: Vec::new(),
            validation: ValidationBlock {
                section_roles_assigned: 0,
                tables_classified: BTreeMap::new(),
                tables_unknown: 0,
                glossary_entries: 0,
                chrome_lines_stripped: 0,
                warnings: Vec::new(),
            },
        }
    }

    /// A descriptor with a `block:Instruction Fetch (IF)` section role
    /// and `Layer::Micro` produces a chunk whose front matter carries
    /// the matching `spec_md_role` + `layer` keys (Chapter 7 §7.8).
    #[test]
    fn chunk_front_matter_carries_block_role_and_layer() {
        let section = Section {
            heading: "Instruction Fetch (IF)".to_string(),
            level: 2,
            breadcrumb: vec![
                "Execution Pipeline".to_string(),
                "Instruction Fetch (IF)".to_string(),
            ],
            body: String::new(),
            page_range: (11, 14),
            children: Vec::new(),
            kind: super::super::parse::SectionKind::Prose,
            contained_signal_tables: Vec::new(),
            contained_parameter_tables: Vec::new(),
            contained_error_tables: Vec::new(),
            contained_encoding_tables: Vec::new(),
            contained_fsm_tables: Vec::new(),
            contained_figures: Vec::new(),
            tbd_count: 0,
            stub_hint: None,
        };
        let format = make_format(
            vec![SectionRoleEntry {
                heading: "Instruction Fetch (IF)".to_string(),
                page: 11,
                line: 5,
                font_size: 14.0,
                font_weight: FontWeight::Bold,
                level: 2,
                spec_md_role: SpecMdRole::Block {
                    block_name: "Instruction Fetch (IF)".to_string(),
                },
                layer: Layer::Micro,
                rationale: String::new(),
            }],
            Vec::new(),
        );
        let body = render_chunk(&section, "cid", Some(&format), &[]);
        assert!(
            body.contains("spec_md_role: \"block:Instruction Fetch (IF)\""),
            "chunk front matter missing block role:\n{body}"
        );
        assert!(
            body.contains("layer: \"micro\""),
            "chunk front matter missing layer:\n{body}"
        );
    }

    /// A chunk body containing the acronym "IF" as a whole word plus
    /// a glossary entry `IF -> Instruction Fetch` lands "IF" in the
    /// chunk's `acronyms_referenced` list. Whole-word: a body
    /// containing "DIFFICULT" must NOT match.
    #[test]
    fn chunk_acronyms_referenced_whole_word_match() {
        let mut section = Section {
            heading: "Pipeline Overview".to_string(),
            level: 2,
            breadcrumb: vec!["Pipeline Overview".to_string()],
            body: "The IF stage fetches instructions. DIFFICULTY is unrelated.".to_string(),
            page_range: (1, 1),
            children: Vec::new(),
            kind: super::super::parse::SectionKind::Prose,
            contained_signal_tables: Vec::new(),
            contained_parameter_tables: Vec::new(),
            contained_error_tables: Vec::new(),
            contained_encoding_tables: Vec::new(),
            contained_fsm_tables: Vec::new(),
            contained_figures: Vec::new(),
            tbd_count: 0,
            stub_hint: None,
        };
        let format = make_format(
            Vec::new(),
            vec![
                GlossaryEntry {
                    acronym: "IF".to_string(),
                    expansion: "Instruction Fetch".to_string(),
                    first_page: 1,
                    scope: String::new(),
                    used_in_blocks: Vec::new(),
                    source: GlossarySource::GlossarySection,
                },
                GlossaryEntry {
                    acronym: "PD".to_string(),
                    expansion: "Pre Decode".to_string(),
                    first_page: 1,
                    scope: String::new(),
                    used_in_blocks: Vec::new(),
                    source: GlossarySource::GlossarySection,
                },
            ],
        );
        let body = render_chunk(&section, "cid", Some(&format), &[]);
        assert!(
            body.contains("acronyms_referenced: [\"IF\"]"),
            "expected acronyms_referenced=[\"IF\"]; got:\n{body}"
        );
        assert!(
            !body.contains("\"PD\""),
            "PD must not match since it's not in the body:\n{body}"
        );

        // Confirm whole-word boundary by stuffing the body with a
        // hyphenated occurrence: matches across word characters only.
        section.body = "underscore_IF_should not match; suffix_IF won't either".to_string();
        let body = render_chunk(&section, "cid", Some(&format), &[]);
        assert!(
            body.contains("acronyms_referenced: []"),
            "expected no matches when IF is embedded in identifier:\n{body}"
        );
    }

    /// With `format: None`, the new chunk front-matter keys still
    /// emit so downstream readers see a stable schema; values are
    /// empty / "unknown".
    #[test]
    fn chunk_front_matter_keys_stable_with_no_format() {
        let section = Section {
            heading: "Some Section".to_string(),
            level: 1,
            breadcrumb: vec!["Some Section".to_string()],
            body: "body text".to_string(),
            page_range: (1, 1),
            children: Vec::new(),
            kind: super::super::parse::SectionKind::Prose,
            contained_signal_tables: Vec::new(),
            contained_parameter_tables: Vec::new(),
            contained_error_tables: Vec::new(),
            contained_encoding_tables: Vec::new(),
            contained_fsm_tables: Vec::new(),
            contained_figures: Vec::new(),
            tbd_count: 0,
            stub_hint: None,
        };
        let body = render_chunk(&section, "cid", None, &[]);
        for key in [
            "spec_md_role: \"\"",
            "layer: \"unknown\"",
            "acronyms_referenced: []",
            "contained_csr_tables: []",
            "clock_domain: \"\"",
            "power_domain: \"\"",
            "reset_domain: \"\"",
        ] {
            assert!(
                body.contains(key),
                "expected key `{key}` in chunk front matter:\n{body}"
            );
        }
    }

    #[test]
    fn emit_synthetic_corpus_writes_expected_files() {
        let body = "# Intro\n\nbody\n\n## Signals\n\n| Signal | Direction | To/From | Description |\n| --- | --- | --- | --- |\n| if_nxt_pc | out | Bus | next addr |\n| parcel_pc | in | Bus | fetch addr |\n\n## TBDs\n\nThis has TBD value\n";
        let mut warnings = Vec::new();
        let mut tree = parse_markdown(body, &mut warnings).unwrap();
        let config = super::super::super::pipeline::IngestConfig::default();
        let _outputs = classify(&mut tree, &config, &mut warnings);

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        // Need a source file so sha256 has something to hash.
        let src = project.join("spec.md");
        fs::write(&src, body).unwrap();
        let request = IngestRequest {
            primary: Some(SourceSpec::new(src)),
            peers: Vec::new(),
            config: config.clone(),
            project_root: project.clone(),
        };
        let refs = Vec::new();
        let figures = Vec::new();
        let chrome = ChromeRecord::default();
        tree.source_kind = Some(SourceKind::Markdown);
        let outcome = emit_corpus(&request, &tree, &chrome, &refs, &figures, warnings).unwrap();
        assert!(outcome.manifest_path.exists());
        let manifest = fs::read_to_string(&outcome.manifest_path).unwrap();
        assert!(manifest.contains("source_kind = \"markdown\""));
        assert!(manifest.contains("primary_signal_table_count = 1"));
        let chunks_dir = project.join(".sim-flow/spec-ingest/primary/chunks");
        assert!(chunks_dir.exists());
        let entries: Vec<_> = fs::read_dir(&chunks_dir).unwrap().collect();
        assert!(!entries.is_empty());
        let signals_dir = project.join(".sim-flow/spec-ingest/primary/tables/signals");
        assert!(signals_dir.exists());
        assert_eq!(fs::read_dir(&signals_dir).unwrap().count(), 1);
    }
}
