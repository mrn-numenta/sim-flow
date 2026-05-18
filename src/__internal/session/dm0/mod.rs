//! DM0 (Specification) flow integration.
//!
//! Phase 6 owns this module. DM0 produces `docs/spec.md` either by
//! auto-populating from an ingested source-spec corpus
//! ([`auto_populate`]) or by driving an interactive Q&A loop on top
//! of the `ask_user` tool ([`qa_loop`]). The DM0 gate-check
//! ([`gate`]) validates the artifact against the Phase 1 schema.
//!
//! Public surface is the [`run_dm0_work`] entry point plus the
//! per-section helpers each submodule exposes; the implementation
//! is split across three streams so they can land in parallel
//! without overlapping in this `mod.rs`.

pub mod auto_populate;
pub mod gate;
pub mod prompts;
pub mod qa_loop;

use std::path::Path;

use crate::Result;

/// DM0 operating mode, derived from `.sim-flow/spec-ingest/manifest.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dm0Mode {
    /// `source_kind` is `pdf | markdown | text`. The auto-populate
    /// step seeds `SpecMd` from the ingest corpus; the agent
    /// completes prose sections and resolves TBDs.
    SourceDriven,
    /// `source_kind = "none"`. The agent drives an interactive Q&A
    /// loop to fill every REQUIRED field.
    NoSource,
}

/// Read the ingest manifest at
/// `<project>/.sim-flow/spec-ingest/manifest.toml` and infer the
/// DM0 mode. Returns `Err` if the manifest is missing or malformed
/// — callers decide whether to fall back to `NoSource` or to surface
/// a diagnostic.
#[allow(dead_code)]
pub fn detect_mode(_project_dir: &Path) -> Result<Dm0Mode> {
    todo!("Phase 6 milestone 6.2 — parse manifest.toml and branch on source_kind")
}

/// Outcome of a DM0 work session. Reported back to the orchestrator
/// for advancement / gate dispatch.
#[derive(Debug, Clone, Default)]
pub struct Dm0Outcome {
    pub mode: Option<Dm0Mode>,
    pub fields_filled: usize,
    pub tbds_recorded: usize,
}

/// Top-level entry point invoked by the orchestrator when the DM0
/// work session starts. Branches on [`detect_mode`] and either runs
/// the auto-populate path ([`auto_populate::run`]) or the Q&A loop
/// ([`qa_loop::drive_qa_loop`]). The orchestrator drives the LLM
/// turn loop separately; this function returns once the
/// auto-populate / Q&A bootstrapping is done.
#[allow(dead_code)]
pub fn run_dm0_work(_project_dir: &Path) -> Result<Dm0Outcome> {
    todo!("Phase 6 milestone 6.1 — wire auto_populate + qa_loop from detect_mode")
}
