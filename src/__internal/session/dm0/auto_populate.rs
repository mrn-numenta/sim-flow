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

use std::path::Path;

use crate::__internal::session::spec_md::SpecMd;
use crate::Result;

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

#[allow(dead_code)]
pub fn populate_metadata(_manifest_path: &Path, _spec: &mut SpecMd) -> Result<()> {
    todo!("Phase 6 milestone 6.3 — fill SpecMd.metadata.source_documents from manifest peers")
}

#[allow(dead_code)]
pub fn populate_assumptions(_corpus_root: &Path, _spec: &mut SpecMd) -> Result<()> {
    todo!("Phase 6 milestone 6.3 — scan chunks for clock freq / tech node and append rows")
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
