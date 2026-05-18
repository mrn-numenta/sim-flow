//! DM0 gate-check.
//!
//! Replaces the regex-based gate dispatch with a parser-driven
//! check: parse `docs/spec.md` via Phase 1, run
//! `SpecMd::validate()`, verify quantitative-section regexes,
//! resolve every source-anchor against the ingest manifest, and
//! (in automated mode) verify Auto-decisions were populated.
//!
//! Owned by Phase 6 Stream C.

use std::path::Path;

use crate::Result;

/// Aggregate outcome of a DM0 gate-check. The `failures` vector
/// mirrors `gate::GateFailure` but is computed against the new
/// structured parser instead of regex matchers. The orchestrator
/// converts this into the existing `gate::GateReport` for protocol
/// emission.
#[derive(Debug, Clone, Default)]
pub struct Dm0GateOutcome {
    pub failures: Vec<Dm0GateFailure>,
}

impl Dm0GateOutcome {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Dm0GateFailure {
    /// Short human-readable label (e.g. `"missing-clock-frequency"`).
    pub code: String,
    /// One-sentence description of what failed and where.
    pub message: String,
}

/// Evaluate the DM0 gate against the project's current `spec.md`
/// and ingest manifest. The manifest is optional — for no-source
/// projects the manifest path may not exist; in that case
/// anchor-resolution checks are skipped but the structural / regex
/// checks still run.
#[allow(dead_code)]
pub fn check_dm0_gate(
    _spec_md_path: &Path,
    _manifest_path: Option<&Path>,
) -> Result<Dm0GateOutcome> {
    todo!("Phase 6 milestone 6.11 — parse, validate, resolve anchors, check auto-decisions")
}
