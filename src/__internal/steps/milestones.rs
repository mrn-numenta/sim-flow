//! Milestone walk machinery.
//!
//! Steps whose work / critique sessions iterate over a directory of
//! per-milestone markdown files (DM2cd / DM2d / DM3a/b/c / DM4a/b)
//! share this walker + state engine. Two modes live behind the
//! [`MilestoneWalkConfig::placeholder_marker`] switch:
//!
//! - **Execution mode** (`placeholder_marker = None`): milestones are
//!   checkbox-driven. `- [ ]` rows mean "task pending", `- [x]` means
//!   "agent worked on it", `- [-]` means "deferred" (and counts as
//!   pending iff `forbid_deferred` is `true`).
//!
//! - **Planning-detail mode** (`placeholder_marker = Some(s)`): the
//!   milestone files are stubs the upstream outline step wrote. A
//!   milestone is pending iff `s` is still in the body; the agent
//!   resolves it by replacing the stub with a real task list.
//!
//! Sub-modules group the implementation:
//!   - this file holds shared types + the [`MilestoneManager`] trait
//!   - [`walk`] -- find / enumerate / look-up by name
//!   - [`state`] -- per-milestone state checks + reset
//!   - [`tick`] -- auto-tick checkbox rows when artifacts resolve

pub mod state;
pub mod tick;
pub mod walk;

pub use state::milestone_is_resolved;
pub use tick::tick_resolved_milestone_tasks;
pub use walk::{enumerate_pending_milestones, find_current_milestone, find_milestone_by_name};

/// Config for steps whose work / critique sessions iterate over a
/// directory of milestone files instead of running once over the
/// step as a whole. See `StepDescriptor::milestone_walk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilestoneWalkConfig {
    /// Project-relative directory holding the milestone files.
    /// Must end in `/`. Example: `"docs/test-plan/"`.
    pub dir: &'static str,
    /// Filename prefixes (before the `NN-...` suffix) that identify
    /// milestone files in `dir`. Most steps use one prefix
    /// (`"tb-milestone-"` for DM3b, `"test-milestone-"` for DM3c);
    /// DM3a-detail walks BOTH `tb-milestone-` and `test-milestone-`
    /// in one step because each tb / test category is small enough
    /// to share a critique loop. The walker treats every matching
    /// file as a milestone, walking lexicographically across all
    /// prefixes (so `tb-milestone-01-*.md` comes before
    /// `test-milestone-01-*.md` if both prefixes are present).
    pub file_prefixes: &'static [&'static str],
    /// Project-relative path of the plan-index file that lives
    /// alongside the milestone files. Always inlined in the
    /// session inputs so the agent can see the TOC, traceability,
    /// etc. Example: `"docs/test-plan/test-plan.md"`.
    pub index_file: &'static str,
    /// Optional placeholder-mode marker. When `Some(s)`, a milestone
    /// is "pending" iff its body contains `s`, "resolved" iff it
    /// doesn't. Used by the planning detail steps (DM2cd / DM3ad /
    /// DM4ad), which walk milestone-NN-*.md stub files written by
    /// the outline step (DM2c / DM3a / DM4a) and replace the stub
    /// with a full task list. Without this mode the gate would see
    /// fresh `- [ ]` rows (the planned tasks for the downstream
    /// execution step) and never advance.
    ///
    /// When `None`, the walker uses the default execution-step
    /// semantics: pending iff the file has at least one `- [ ]`
    /// row, resolved iff every row is `- [x]` / `- [-]`.
    pub placeholder_marker: Option<&'static str>,
    /// When `true`, `- [-]` (deferred) rows count as pending for
    /// `find_current_milestone` -- the walker keeps targeting the
    /// milestone until every deferred row is converted to `- [x]`.
    /// Pairs with the matching `forbid_deferred` flag on the gate
    /// check so milestone-walk dispatch and gate evaluation agree.
    /// Used by DM2d / DM3c / DM4b. Has no effect under
    /// `placeholder_marker` mode (planning-detail steps don't have
    /// `[-]` rows).
    pub forbid_deferred: bool,
}

/// Walk + reset interface for steps whose work / critique sessions
/// iterate over a directory of milestone files.
///
/// Co-locating the two methods is the load-bearing detail: the walker
/// reads per-milestone progress state (checkbox marks or placeholder
/// markers) to pick the next target, and `sim-flow reset <step>` has
/// to clear EXACTLY the same state so post-reset on-disk shape
/// equals never-run shape. Before the trait existed, the file sweep
/// in `clear_step_collateral_forward` only deleted things in the
/// step's `work_artifacts` list -- but the milestone checkbox state
/// lives in upstream-owned files (DM2c owns `docs/impl-plan/*.md`;
/// DM2d only flips boxes inside them), so a `reset DM2d` left every
/// `- [x]` from the prior run intact and the dashboard reported 100%
/// completion for a step with no source code on disk.
///
/// One impl per progress shape: [`MilestoneWalkConfig`] covers both
/// the checkbox-driven mode (DM2d / DM3b / DM3c / DM4b) and the
/// placeholder-marker mode (DM2cd / DM3ad / DM4ad) via a single
/// branch on `placeholder_marker`. Future shapes (e.g. JSON-state
/// driven) get their own impl without touching either call site.
pub trait MilestoneManager: std::fmt::Debug + Send + Sync {
    /// Project-relative directory holding the milestone files.
    /// Always ends in `/`.
    fn dir(&self) -> &str;

    /// Project-relative path of the plan-index file alongside the
    /// milestone files. Inlined into every session's inputs.
    fn index_file(&self) -> &str;

    /// True when `- [-]` (deferred) rows count as pending. Drives
    /// the walker to keep targeting milestones with deferrals until
    /// they're converted to `- [x]`. Default `false`.
    fn forbids_deferred(&self) -> bool {
        false
    }

    /// Find the milestone file the next session should target.
    /// See [`find_current_milestone`] for the contract; this method
    /// is the trait-facing entry point. The free-standing function
    /// stays as the implementation detail so existing callers keep
    /// compiling unchanged.
    fn walk(
        &self,
        project_dir: &std::path::Path,
        prior_critique_has_blockers: bool,
    ) -> CurrentMilestone;

    /// Clear all per-milestone progress so post-reset state is
    /// equivalent to a never-run step. Called from
    /// `clear_step_collateral_forward`. Returns the list of files
    /// modified. Errors propagate; the caller surfaces them as
    /// reset diagnostics.
    fn reset(&self, project_dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>>;
}

impl MilestoneManager for MilestoneWalkConfig {
    fn dir(&self) -> &str {
        self.dir
    }

    fn index_file(&self) -> &str {
        self.index_file
    }

    fn forbids_deferred(&self) -> bool {
        self.forbid_deferred
    }

    fn walk(
        &self,
        project_dir: &std::path::Path,
        prior_critique_has_blockers: bool,
    ) -> CurrentMilestone {
        find_current_milestone(project_dir, self, prior_critique_has_blockers)
    }

    fn reset(&self, project_dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        match self.placeholder_marker {
            // Checkbox mode (DM2d / DM3b / DM3c / DM4b): flip every
            // `- [x]` / `- [X]` / `- [-]` row back to `- [ ]`. The
            // milestone files themselves stay -- they're upstream's
            // (DM2c's) work_artifacts and contain the task TEXT,
            // which we don't want to regenerate.
            None => state::reset_checkbox_milestones(project_dir, self),
            // Placeholder mode (DM2cd / DM3ad / DM4ad): the milestone
            // files were stubs containing `marker`, replaced by real
            // task lists when this step ran. We don't have the
            // original stub content to restore, so an in-place
            // reset isn't possible. Skip silently: the only path
            // to a clean placeholder-mode reset is also resetting
            // the upstream outline step (DM2c / DM3a / DM4a), which
            // the downstream cascade in `clear_step_collateral_forward`
            // will sweep when the user issues `reset <outline-step>`.
            // Returning an error here would block the rest of the
            // reset (which DOES want to proceed for the other
            // collateral) -- worse outcome than the user having to
            // re-run reset against the upstream.
            Some(_) => Ok(Vec::new()),
        }
    }
}

/// Outcome of `find_current_milestone`. The orchestrator uses this
/// to decide which milestone file to scope the next work / critique
/// session to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentMilestone {
    /// Project-relative path of the milestone the next session
    /// should target (e.g. `docs/test-plan/tb-milestone-02-drivers.md`).
    File(String),
    /// Every milestone file under `dir` has all its `- [ ]` rows
    /// resolved (`- [x]` or `- [-]` deferred). The auto-driver
    /// uses this as the structural "all done" signal that, paired
    /// with critique-clean, advances the step gate.
    AllResolved,
    /// `dir` exists but contains no `<file_prefix>NN-*.md` files.
    /// Treat as a configuration error: the planning step (DM2c /
    /// DM3a / DM4a) should have produced at least one milestone
    /// file before this step ran.
    NoMilestonesPresent,
}

/// Return value of [`enumerate_pending_milestones`]. Distinguishes
/// "directory present" (with a possibly-empty list of pending
/// milestones) from "directory missing" (probable setup error)
/// so callers don't have to overload the empty-vec signal. See
/// [`find_milestone_by_name`] / [`find_current_milestone`] which
/// surface the same distinction via `CurrentMilestone::NoMilestonesPresent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingMilestones {
    /// The milestone directory exists; `pending` is the list of
    /// milestone-relative paths whose body still matches
    /// `walk.placeholder_marker`. Empty when every milestone has
    /// been resolved.
    Present { pending: Vec<String> },
    /// The milestone directory the walker is configured for does
    /// not exist on disk. The caller should treat this as a setup
    /// error rather than "nothing to do."
    DirectoryMissing,
}

impl PendingMilestones {
    /// Pending list when present, or an empty list when the
    /// directory is missing. Use this only when "missing" and
    /// "empty" are equivalent for the caller's purpose. Callers
    /// that care (e.g. the parallel dispatcher needing to flip to
    /// manual on a setup error) should match on the variant
    /// directly.
    pub fn into_vec(self) -> Vec<String> {
        match self {
            PendingMilestones::Present { pending } => pending,
            PendingMilestones::DirectoryMissing => Vec::new(),
        }
    }

    /// Number of pending milestones. Returns 0 when the directory
    /// is missing.
    pub fn len(&self) -> usize {
        match self {
            PendingMilestones::Present { pending } => pending.len(),
            PendingMilestones::DirectoryMissing => 0,
        }
    }

    /// `true` when there are no pending milestones, whether
    /// because the directory is missing or because every
    /// milestone is already resolved.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
