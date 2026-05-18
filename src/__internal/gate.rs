//! Gate validation primitives.
//!
//! Each step's gate is a sequence of [`GateCheck`]s. The orchestrator
//! evaluates every check and collects failures so the user sees the full
//! list, not just the first blocker.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Result;

mod evaluators;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum GateCheck {
    /// The given path (relative to the project dir) must exist and be
    /// non-empty.
    FileExists { path: PathBuf, description: String },
    /// The given file must contain a match for the regex.
    FileMatches {
        path: PathBuf,
        pattern: String,
        description: String,
    },
    /// Run `cmd` in the project dir; success is exit 0.
    Shell {
        cmd: String,
        args: Vec<String>,
        description: String,
    },
    /// The critique file at `path` must not contain any `BLOCKER:`
    /// or `UNRESOLVED:` lines.
    CritiqueClean { path: PathBuf, description: String },
    /// The experiments index at `.sim-flow/experiments.db` must contain
    /// at least one row in the `runs` table. Used by DM4 to confirm that
    /// tracking captured a simulation run before the analysis gate
    /// passes.
    ExperimentsRecorded { description: String },
    /// At least one of the listed paths must exist and be non-empty.
    /// Each path may be a file (matched directly) or a directory
    /// (treated as "any non-empty `*.md` inside, excluding
    /// `.gitkeep` / `README.md`"). Used to accept either of two
    /// canonical artifact layouts -- e.g. the generated spec can
    /// land as a single `docs/spec.md` (small designs) or as a
    /// directory of section files under `docs/spec/` (large
    /// designs paginated like the input spec).
    AnyExists {
        paths: Vec<PathBuf>,
        description: String,
    },
    /// Like `FileMatches`, but the pattern needs to match in at
    /// least ONE of the listed paths. Each path may be a file or
    /// a directory of `*.md` (same expansion rule as `AnyExists`).
    /// Used so DM0's frequency / tech-node regexes still pass when
    /// the spec is paginated and the matching string lives in a
    /// section file rather than the top-level `docs/spec.md`.
    AnyMatches {
        paths: Vec<PathBuf>,
        pattern: String,
        description: String,
    },
    /// Structured-spec.md check: parse `spec_md_path`, run the
    /// Phase 1 validator, verify the Quantitative-row regexes, and
    /// resolve every source-anchor against `manifest_path` (when
    /// present). Used by DM0 in place of the old regex-based gate
    /// dispatch. The implementation lives in
    /// [`crate::__internal::session::dm0::gate::check_dm0_gate`];
    /// the evaluator simply converts its `Dm0GateOutcome` into the
    /// orchestrator's `GateReport` so the existing emission paths
    /// keep working.
    SpecMdStructured {
        spec_md_path: PathBuf,
        /// `Some` when the project has an ingest manifest;
        /// `None` for no-source-spec projects (anchor resolution
        /// is then skipped).
        manifest_path: Option<PathBuf>,
        description: String,
    },
    /// Every milestone file under `dir` matching one of
    /// `<file_prefix>NN-*.md` (for any `<file_prefix>` in
    /// `file_prefixes`) must be resolved.
    ///
    /// Two resolution modes, matching `MilestoneWalkConfig`:
    /// - When `placeholder_marker` is `None` (execution-step gate
    ///   for DM2d / DM3b / DM3c / DM4b): every `- [ ]` row must be
    ///   resolved. By default `- [x]` (done) AND `- [-]` (deferred)
    ///   both pass; set `forbid_deferred = true` on steps where
    ///   skipping a row would silently drop work that downstream
    ///   steps depend on (DM2d / DM3c / DM4b -- the model-impl,
    ///   test-impl, perf-impl gates) so `- [-]` ALSO counts as
    ///   pending. The check still permits `- [-]` on DM3b
    ///   (testbench skeletons can legitimately defer integration
    ///   shims) where `forbid_deferred = false`.
    /// - When `placeholder_marker` is `Some(s)` (planning-detail
    ///   gate for DM2cd / DM3ad / DM4ad): no milestone body may
    ///   contain `s`. The detail step replaces the outline's stubs
    ///   with full task lists; this gate fails until every stub has
    ///   been replaced, ignoring `- [ ]` row counts (since the
    ///   detail step's whole purpose is to LAND `- [ ]` rows for
    ///   the downstream execution step to walk).
    MilestonesAllResolved {
        dir: PathBuf,
        file_prefixes: Vec<String>,
        placeholder_marker: Option<String>,
        description: String,
        /// When `true`, `- [-]` rows count as pending in the
        /// no-placeholder execution-mode branch. Used by DM2d /
        /// DM3c / DM4b to enforce that deferred items are actually
        /// implemented, not optimistically skipped. Has no effect
        /// in placeholder-marker mode.
        forbid_deferred: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct GateFailure {
    pub description: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GateReport {
    pub failures: Vec<GateFailure>,
}

impl GateReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

pub fn evaluate(project_dir: &Path, checks: &[GateCheck]) -> Result<GateReport> {
    let mut report = GateReport::default();
    for check in checks {
        if let Some(failure) = evaluators::evaluate_one(project_dir, check)? {
            report.failures.push(failure);
        }
    }
    Ok(report)
}
