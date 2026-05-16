//! Step registry.
//!
//! Every flow (DM, DS) registers its step descriptors here. Each descriptor
//! captures the step id, its ordered position in the flow, its prerequisite
//! step (if any), the slug used to look up instruction files, whether the
//! step is per-candidate, and its gate checks.

pub mod dm;
pub mod ds;
pub mod sv;

use crate::gate::GateCheck;
use crate::state::Flow;

/// Tools every step's orchestrator advertises to the LLM. Path-sandbox
/// keeps everything inside `project_dir`; the per-step gating that
/// used to live on `StepDescriptor` was cosmetic (file writes happen
/// through the artifact-write convention regardless of catalog) and
/// only encouraged tool-name hallucination, so we hand the agent the
/// full set everywhere.
pub const UNIVERSAL_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "write_file",
    "edit_file",
    "delete_file",
    "search",
    "run_cargo",
    "declare_fix",
    "declare_hypothesis",
    "log_bug",
    "resolve_bug",
    "record_run",
    // `api_*` tools are backed by a lazily-spawned rust-analyzer
    // subprocess (see `session::lsp`). Hybrid plan in
    // `docs/brainstorming/rust-analyzer-lsp-discovery.md`: keep
    // `fw:api/toc.md` for narrative scaffolding, replace deep
    // page reads with live LSP queries.
    "api_search",
    "api_hover",
    "api_impls",
    "api_references",
    "api_expand_macro",
];

#[derive(Debug, Clone)]
pub struct StepDescriptor {
    pub id: &'static str,
    pub flow: Flow,
    pub prerequisite: Option<&'static str>,
    pub instruction_slug: &'static str,
    pub per_candidate: bool,
    /// Comprehensive step gate evaluated at advance time. For
    /// non-milestone-walk steps the wind-down decision uses this
    /// too; milestone-walk steps that set `walk_gate_checks` use
    /// THAT instead during the walk and reserve this list for the
    /// final step-advance evaluation.
    pub gate_checks: Vec<GateCheck>,
    /// Per-milestone gate evaluated during the walk's wind-down
    /// decisions (and surfaced in the no-artifact pump's failure
    /// feedback). Empty means "use `gate_checks`" -- the previous
    /// behavior. Code-walking steps (DM2d, DM3b, DM3c, DM4b)
    /// override this with the cheap quality checks (`cargo fmt`,
    /// `clippy`, `build`, etc.) and reserve the expensive integration
    /// checks (`cargo test --test elaboration`, the cross-module
    /// `grep -r Symbol src` checks, `milestones_all_implemented`)
    /// for `gate_checks` so they only run at advance time. Until
    /// the last milestone lands, those checks would necessarily fail
    /// and rerunning to address them is wasted compute.
    pub walk_gate_checks: Vec<GateCheck>,
    /// Project-relative paths the work session is expected to
    /// produce or update. Used by `sim-flow describe` so hosts can
    /// tell the LLM exactly where to write artifacts. Directories
    /// end in `/` (e.g. `src/`).
    pub work_artifacts: &'static [&'static str],
    /// Project-relative paths from prior steps that this step's
    /// work and critique sessions read as input. Critique sessions
    /// also include this step's `work_artifacts` as their inputs;
    /// that is derived by the caller rather than stored.
    pub predecessor_inputs: &'static [&'static str],
    /// Project-relative path prefixes the WORK session is allowed to
    /// write. Trailing `/` matches any path under that directory; no
    /// trailing slash matches an exact file path. Enforced by
    /// `WriteFileTool`, `EditFileTool`, and the artifact-write
    /// extractor in the orchestrator. Critique sessions don't honor
    /// this list — they're independently restricted to a single
    /// `docs/critiques/{step_id}-critique.md` file.
    pub work_write_paths: &'static [&'static str],
    /// Phase pipeline for the work session, e.g. `["chat"]` for
    /// non-code steps or `["author", "build", "test"]` for code
    /// steps. The orchestrator runs validators between phases; see
    /// docs/architecture/ai-flow/08-orchestrator-tools.md.
    pub work_phases: &'static [&'static str],
    /// Phase pipeline for the critique session. Always `["chat"]`
    /// in M3.
    pub critique_phases: &'static [&'static str],
    /// Optional milestone-walk binding. When set, the step's work
    /// AND critique sessions are scoped by the orchestrator to a
    /// SINGLE milestone file at a time (the first
    /// `<dir><file_prefix>NN-*.md` with at least one open `- [ ]`
    /// row). The agent doesn't see other milestone files; the
    /// auto-driver iterates the work-then-critique cycle until
    /// every milestone in the directory has all rows resolved.
    /// This is the structural enforcement of the
    /// "one-milestone-at-a-time, critique-each-milestone-then-
    /// advance" workflow described in the prompt for DM2d / DM3b /
    /// DM3c / DM4b. The prompt-only version (relying on the agent
    /// to STOP) was unreliable -- agents often chained milestones
    /// in one work session.
    pub milestone_walk: Option<MilestoneWalkConfig>,
}

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

/// Allowed write-path prefixes for the given (step, kind). Work
/// sessions return the step's `work_write_paths`; critique sessions
/// allow the canonical critique filename in BOTH JSON form (the
/// shape the agent emits) and markdown form (the shape the
/// orchestrator renders post-write for human review). Path
/// enforcement uses prefix-match for entries ending in `/` and
/// exact-match otherwise.
pub fn allowed_write_paths(step: &StepDescriptor, kind: crate::client::SessionKind) -> Vec<String> {
    match kind {
        crate::client::SessionKind::Work => step
            .work_write_paths
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        crate::client::SessionKind::Critique => {
            vec![
                format!("docs/critiques/{}-critique.json", step.id),
                format!("docs/critiques/{}-critique.md", step.id),
            ]
        }
    }
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
            None => reset_checkbox_milestones(project_dir, self),
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

/// Find the milestone file the next session should target.
///
/// Modes (cross-cutting with `MilestoneWalkConfig::placeholder_marker`):
///
/// **Execution mode** (placeholder_marker = None) -- DM2d / DM3b /
/// DM3c / DM4b walk over milestone files filling in code:
/// - **Fresh / advance** (`prior_critique_has_blockers = false`):
///   return the FIRST milestone file with at least one open `- [ ]`
///   row.
/// - **Retry** (`prior_critique_has_blockers = true`): return the
///   HIGHEST-NUMBERED milestone file with at least one `- [x]` row
///   (i.e., the agent's most recent target). Even if the agent
///   prematurely flipped its rows to `- [x]`, the retry stays on
///   THE SAME milestone the critique fired on. Falls back to
///   first-pending if no `- [x]` rows exist anywhere.
///
/// **Planning-detail mode** (placeholder_marker = Some(s)) --
/// DM2cd / DM3ad / DM4ad walk over stub milestone files written by
/// the outline step and replace the stub with a real task list:
/// - **Fresh / advance**: return the FIRST milestone file whose
///   body still contains the placeholder.
/// - **Retry**: return the HIGHEST-NUMBERED milestone whose
///   placeholder has been removed (= the agent has detailed it; the
///   critique fired on that milestone). Falls back to
///   first-pending if no milestone has been detailed yet.
///
/// Filenames are matched case-sensitively against any
/// `<prefix><digits>...md` for `<prefix>` in `walk.file_prefixes`.
/// The lexicographic sort over the matching subset gives the
/// natural numeric order as long as callers pad NN to two digits
/// (`01`, `02`, ...), which the prompts and `plan-management.md`
/// enforce.
pub fn find_current_milestone(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
    prior_critique_has_blockers: bool,
) -> CurrentMilestone {
    let files = match list_milestone_files(project_dir, walk) {
        Some(f) if !f.is_empty() => f,
        _ => return CurrentMilestone::NoMilestonesPresent,
    };

    if prior_critique_has_blockers {
        // Retry: highest-numbered milestone the agent has touched.
        for (name, path) in files.iter().rev() {
            if milestone_is_touched(walk, path) {
                return CurrentMilestone::File(join_milestone_rel(walk, name));
            }
        }
        // Nothing touched yet: fall through to first-pending.
    }

    for (name, path) in &files {
        if milestone_is_pending(walk, path) {
            return CurrentMilestone::File(join_milestone_rel(walk, name));
        }
    }
    CurrentMilestone::AllResolved
}

/// Project-relative paths of every milestone file under `walk.dir`
/// that is currently pending, in walker order. Used by the parallel
/// plan-detail walk dispatcher to fan out Work sessions across all
/// pending stubs at once; the serial walker keeps using
/// [`find_current_milestone`] one-at-a-time.
///
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

/// Enumerate the pending milestones under `walk`'s configured
/// directory. Returns `DirectoryMissing` when the directory is
/// absent, otherwise `Present { pending }` -- with `pending`
/// possibly empty when every milestone is already resolved.
/// Mirrors `find_milestone_by_name`'s `NoMilestonesPresent` vs
/// "found" distinction so callers don't have to overload the
/// empty-vec signal to mean two structurally different states.
pub fn enumerate_pending_milestones(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> PendingMilestones {
    let Some(files) = list_milestone_files(project_dir, walk) else {
        return PendingMilestones::DirectoryMissing;
    };
    let pending = files
        .iter()
        .filter(|(_, path)| milestone_is_pending(walk, path))
        .map(|(name, _)| join_milestone_rel(walk, name))
        .collect();
    PendingMilestones::Present { pending }
}

/// Look up a specific milestone by its bare filename (e.g.
/// `"milestone-03-decode.md"`) or by its project-relative path (e.g.
/// `"docs/impl-plan/milestone-03-decode.md"`). Returns
/// `CurrentMilestone::File` for an exact match, regardless of whether
/// the milestone is pending, touched, or resolved -- the parallel
/// dispatcher uses this to scope a worker's session to a specific
/// stub it has already decided to operate on.
///
/// Returns `NoMilestonesPresent` when the directory is missing or
/// empty; `AllResolved` is never returned (it would conflate "found
/// but resolved" with "nothing matched the name").
pub fn find_milestone_by_name(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
    needle: &str,
) -> CurrentMilestone {
    let bare = std::path::Path::new(needle)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(needle);
    let Some(files) = list_milestone_files(project_dir, walk) else {
        return CurrentMilestone::NoMilestonesPresent;
    };
    if files.is_empty() {
        return CurrentMilestone::NoMilestonesPresent;
    }
    for (name, _) in &files {
        if name == bare {
            return CurrentMilestone::File(join_milestone_rel(walk, name));
        }
    }
    CurrentMilestone::NoMilestonesPresent
}

/// Walker-order list of every `<prefix><digits>...md` file in
/// `walk.dir`, used by [`find_current_milestone`],
/// [`enumerate_pending_milestones`], and [`find_milestone_by_name`].
/// Returns `None` if the directory cannot be read at all (missing
/// directory or permission error); `Some(empty)` if the directory
/// exists but no milestone files match.
fn list_milestone_files(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> Option<Vec<(String, std::path::PathBuf)>> {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let Some(prefix) = walk.file_prefixes.iter().find(|p| name.starts_with(**p)) else {
            continue;
        };
        let rest = &name[prefix.len()..];
        if !rest
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            continue;
        }
        files.push((name.to_string(), path));
    }
    // Sort by (numeric prefix, full filename) so a 10th milestone
    // doesn't lex-sort before the 9th. Previously
    // `a.0.cmp(&b.0)` was pure string comparison, which gives
    // `milestone-10 < milestone-9` whenever a project crosses the
    // single-digit boundary. The walker then picked
    // milestone-10 as "first pending" before milestone-9, running
    // them out of intended order. Falling back to the filename
    // string keeps the order deterministic when prefixes tie. See
    // orchestrator audit #12 (2026-05-16).
    files.sort_by(|a, b| {
        milestone_numeric_key(&a.0, walk)
            .cmp(&milestone_numeric_key(&b.0, walk))
            .then_with(|| a.0.cmp(&b.0))
    });
    Some(files)
}

/// Extract the numeric prefix run of `name` after stripping the
/// configured `walk.file_prefixes` match. Returns `u64::MAX` when
/// the file doesn't start with a recognized prefix + digits so
/// such entries sort last; callers should already have filtered
/// them out, but this keeps the sort total.
fn milestone_numeric_key(name: &str, walk: &MilestoneWalkConfig) -> u64 {
    let Some(prefix) = walk.file_prefixes.iter().find(|p| name.starts_with(**p)) else {
        return u64::MAX;
    };
    let rest = &name[prefix.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().unwrap_or(u64::MAX)
}

fn join_milestone_rel(walk: &MilestoneWalkConfig, name: &str) -> String {
    format!(
        "{}{}",
        if walk.dir.ends_with('/') {
            walk.dir.to_string()
        } else {
            format!("{}/", walk.dir)
        },
        name
    )
}

/// True iff the named milestone is fully resolved -- the parallel
/// plan-detail walk dispatcher uses this to decide whether its
/// pinned milestone session is done, since
/// [`find_current_milestone`] no longer gives a useful answer when
/// multiple workers are racing.
///
/// Placeholder-mode (DM2cd / DM3ad / DM4ad): resolved iff the
/// `<!-- detail-pending` marker is gone from the milestone body.
///
/// Execution-mode (DM2d / DM3b / DM3c / DM4b): resolved iff no `[ ]`
/// rows remain (and, when `walk.forbid_deferred`, no `[-]` rows
/// either). Defined as `!milestone_is_pending`; exposed so the
/// orchestrator's pinned-worker wind-down can ask the question
/// directly.
pub fn milestone_is_resolved(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    !milestone_is_pending(walk, path)
}

/// "Pending" means the agent still has work to do on this milestone.
/// In execution mode that's a `- [ ]` row -- and additionally `- [-]`
/// (deferred) when `walk.forbid_deferred` is true, so the walker
/// targets a milestone with deferrals until they're implemented as
/// `- [x]`. In planning-detail mode that's the placeholder marker
/// still present in the body.
fn milestone_is_pending(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    match walk.placeholder_marker {
        Some(marker) => milestone_body_contains(path, marker),
        None => {
            milestone_has_pending_row(path)
                || (walk.forbid_deferred && milestone_has_deferred_row(path))
        }
    }
}

/// "Touched" means the agent has done at least some work on this
/// milestone -- used as the retry-target picker so a critique
/// re-runs the milestone the agent JUST attempted, not whichever
/// one is next pending. In execution mode that's a `- [x]` row
/// (the agent ticked at least one task); in planning-detail mode
/// that's the placeholder having been removed (the agent replaced
/// the stub with real content).
fn milestone_is_touched(walk: &MilestoneWalkConfig, path: &std::path::Path) -> bool {
    match walk.placeholder_marker {
        Some(marker) => !milestone_body_contains(path, marker),
        None => milestone_has_checked_row(path),
    }
}

fn milestone_body_contains(path: &std::path::Path, needle: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(body) => body.contains(needle),
        Err(_) => false,
    }
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [ ]`.
fn milestone_has_pending_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines()
        .any(|line| line.trim_start().starts_with("- [ ]"))
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [-]`. Used (with
/// `forbid_deferred`) to keep the walker targeting milestones that
/// still have deferred rows, so DM2d / DM3c / DM4b force the agent
/// to actually implement them rather than letting `- [-]` masquerade
/// as resolved.
fn milestone_has_deferred_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines()
        .any(|line| line.trim_start().starts_with("- [-]"))
}

/// Reset every milestone file under `walk.dir` matching one of
/// `walk.file_prefixes`: flip every `- [x]` / `- [X]` / `- [-]` row
/// back to `- [ ]`. The task TEXT after the checkbox is preserved
/// verbatim, as are non-task lines (headings, prose, code fences).
/// Files outside the prefix set (e.g. `plan.md`,
/// `plan-management.md`) are left untouched. Returns the list of
/// files actually rewritten (skipped when already in their target
/// shape so we don't churn mtimes).
fn reset_checkbox_milestones(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut touched = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let Some(prefix) = walk.file_prefixes.iter().find(|p| name.starts_with(**p)) else {
            continue;
        };
        // Same digit-suffix gate as find_current_milestone so non-
        // milestone files (plan.md, plan-management.md) aren't
        // touched even if their basename happens to start with the
        // prefix.
        let rest = &name[prefix.len()..];
        if !rest
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let mut changed = false;
        let new_lines: Vec<String> = body
            .lines()
            .map(|line| {
                let trimmed_start = line.trim_start();
                let indent_len = line.len() - trimmed_start.len();
                if trimmed_start.starts_with("- [x]")
                    || trimmed_start.starts_with("- [X]")
                    || trimmed_start.starts_with("- [-]")
                {
                    // Replace just the 5-char `- [x]` prefix; keep
                    // the indentation and trailing text byte-exact.
                    changed = true;
                    let after = &trimmed_start[5..];
                    format!("{}- [ ]{}", &line[..indent_len], after)
                } else {
                    line.to_string()
                }
            })
            .collect();
        if !changed {
            continue;
        }
        let mut new_body = new_lines.join("\n");
        if body.ends_with('\n') && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        std::fs::write(&path, new_body)?;
        touched.push(path);
    }
    Ok(touched)
}

/// True iff the file at `path` contains at least one line whose
/// first non-whitespace tokens are `- [x]` (or `- [X]`). Used to
/// detect "the agent has worked on this milestone".
fn milestone_has_checked_row(path: &std::path::Path) -> bool {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]")
    })
}

/// Per-milestone task-row auto-tick. Walks the current milestone
/// file, finds every `- [ ]` row whose first backtick-quoted token
/// matches the `path[::Symbol[::Sub]]` pattern, verifies the file
/// exists (and the symbol grep-matches if a symbol was named), and
/// flips the row in place to `- [x]`. Returns the number of rows
/// flipped. Idempotent and a no-op when the step has no
/// `milestone_walk` config or the current milestone is `AllResolved`.
///
/// Conservative on purpose: a row whose backtick-quoted token does
/// NOT parse as `path::sym` (e.g. a prose row, or a row that names
/// only a directory) is left alone. The Critique still does the full
/// review; this just removes the agent's tick-the-checkbox turn from
/// the milestone loop.
pub fn tick_resolved_milestone_tasks(
    project_dir: &std::path::Path,
    step: &StepDescriptor,
) -> usize {
    let Some(walk) = step.milestone_walk else {
        return 0;
    };
    // Planning-detail walks (DM2cd / DM3ad / DM4ad,
    // `placeholder_marker = Some`) walk milestone STUBS and write
    // task lists describing what DM2d / DM3b / DM3c / DM4b will
    // later build. At the planning stage, "the named artifact
    // exists on disk" does NOT mean the task is done -- the task
    // is naming what WILL be produced, not what already is. Flip
    // here would silently mark planning tasks as completed, and
    // the critique would then re-flag the mismatch and loop until
    // the no-progress streak guard fires. Execution walks
    // (`placeholder_marker = None`) keep the auto-tick behavior --
    // there the rule "artifact exists -> task done" is correct.
    if walk.placeholder_marker.is_some() {
        return 0;
    }
    let CurrentMilestone::File(rel) = find_current_milestone(project_dir, &walk, true) else {
        return 0;
    };
    let path = project_dir.join(&rel);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let mut flipped = 0usize;
    // Iterate via split_inclusive so each segment retains its
    // original line ending (\n, \r\n, or the trailing slice with
    // no terminator). The prior `body.lines()` strip + `join("\n")`
    // re-emit silently converted CRLF files to LF -- breaks
    // milestone files edited on Windows or by editors that
    // preserve CRLF. See orchestrator audit #8 (2026-05-16).
    let new_body: String = body
        .split_inclusive('\n')
        .map(|line_with_terminator| {
            // Split off the trailing \r?\n so we mutate the
            // content, not the terminator.
            let (content, terminator) = split_line_terminator(line_with_terminator);
            let trimmed = content.trim_start();
            if !trimmed.starts_with("- [ ]") {
                return line_with_terminator.to_string();
            }
            let after = trimmed.trim_start_matches("- [ ]").trim_start();
            let Some(token) = first_backtick_token(after) else {
                return line_with_terminator.to_string();
            };
            if !task_artifact_resolved(project_dir, token) {
                return line_with_terminator.to_string();
            }
            flipped += 1;
            let replaced = content.replacen("- [ ]", "- [x]", 1);
            format!("{replaced}{terminator}")
        })
        .collect();
    if flipped > 0 && write_milestone_atomic(&path, &new_body).is_err() {
        return 0;
    }
    flipped
}

/// Split `s` into (content, terminator) where terminator is one of
/// "\r\n", "\n", or "" (no terminator on the final no-newline
/// segment returned by `split_inclusive`).
fn split_line_terminator(s: &str) -> (&str, &str) {
    if let Some(stripped) = s.strip_suffix("\r\n") {
        (stripped, "\r\n")
    } else if let Some(stripped) = s.strip_suffix('\n') {
        (stripped, "\n")
    } else {
        (s, "")
    }
}

/// Atomic write: tempfile -> sync -> rename -> parent fsync. Same
/// shape as state::write_atomic; inlined here to avoid coupling
/// the steps module to state. Without atomicity a crash mid-write
/// leaves a milestone file truncated and the auto loop can't
/// resume cleanly. See orchestrator audit #8 (2026-05-16).
fn write_milestone_atomic(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Pull the FIRST backtick-quoted token from `s`. Returns the inner
/// string (without the surrounding backticks) or `None` if no
/// well-formed token is found.
fn first_backtick_token(s: &str) -> Option<&str> {
    let start = s.find('`')?;
    let rest = &s[start + 1..];
    let end = rest.find('`')?;
    Some(&rest[..end])
}

/// True if `token` parses as `path[::Symbol[::Sub]]` AND the path
/// exists under `project_dir`, AND -- if a symbol was named -- the
/// LAST `::`-separated segment grep-matches as a word boundary in the
/// file. The grep is conservative: it accepts the symbol name in any
/// position (definition, comment, doc string) because tightening to
/// `\bfn name\b` / `\bstruct name\b` would miss legitimate variants
/// (associated methods, trait impls, type aliases) and produce
/// false-negatives the agent would then have to correct anyway. False
/// positives are recoverable: the Critique does the full review.
fn task_artifact_resolved(project_dir: &std::path::Path, token: &str) -> bool {
    let mut parts = token.splitn(2, "::");
    let path_str = match parts.next() {
        Some(p) if !p.is_empty() => p,
        _ => return false,
    };
    let abs = project_dir.join(path_str);
    if !abs.exists() {
        return false;
    }
    let Some(symbol_chain) = parts.next() else {
        return true;
    };
    let last_symbol = symbol_chain.rsplit("::").next().unwrap_or(symbol_chain);
    if last_symbol.is_empty() {
        return false;
    }
    let body = match std::fs::read_to_string(&abs) {
        Ok(b) => b,
        Err(_) => return false,
    };
    body.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == last_symbol)
}

/// True iff `path` (project-relative) is covered by one of the
/// `allowed` prefixes. `/` accepts both `\\` and `/` separators on
/// the input side so artifact paths that came in via the fenced
/// extractor on Windows still resolve correctly.
pub fn is_path_allowed_for_writes(allowed: &[String], path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    allowed.iter().any(|prefix| {
        if let Some(stripped) = prefix.strip_suffix('/') {
            normalized == stripped || normalized.starts_with(prefix)
        } else {
            normalized == *prefix
        }
    })
}

#[derive(Debug, Clone, Default)]
pub struct StepRegistry {
    steps: Vec<StepDescriptor>,
}

impl StepRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, step: StepDescriptor) {
        self.steps.push(step);
    }

    pub fn steps(&self) -> &[StepDescriptor] {
        &self.steps
    }

    pub fn get(&self, id: &str) -> Option<&StepDescriptor> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn order_for(&self, flow: Flow) -> Vec<&'static str> {
        self.steps
            .iter()
            .filter(|s| s.flow == flow)
            .map(|s| s.id)
            .collect()
    }
}

/// Build the registry for a given flow, containing only that flow's steps.
/// DM5 is intentionally omitted (TBD; see doc 02).
pub fn registry_for(flow: Flow) -> StepRegistry {
    let mut reg = StepRegistry::new();
    match flow {
        Flow::DirectModeling => dm::register(&mut reg),
        Flow::DesignStudy => ds::register(&mut reg),
        Flow::SystemVerilogConvert => sv::register(&mut reg),
    }
    reg
}

#[cfg(test)]
mod write_path_tests {
    use super::*;

    fn allowed(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn directory_prefix_matches_files_under_it() {
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs/spec.md"));
        assert!(is_path_allowed_for_writes(&a, "docs/critiques/DM0.md"));
    }

    #[test]
    fn directory_prefix_matches_bare_dir_form() {
        // The agent occasionally targets the directory itself
        // (e.g. `list_dir`-style references). For write paths this
        // is a no-op write target, but accepting `docs` against
        // allowlist `docs/` keeps the matcher symmetric and avoids
        // surprising rejections of round-tripped paths.
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs"));
    }

    #[test]
    fn directory_prefix_does_not_match_sibling_with_same_name_prefix() {
        // Regression: prefix `src/` must NOT match `src-backup/foo.rs`
        // -- the trailing slash in the prefix is load-bearing for
        // exactly this reason.
        let a = allowed(&["src/"]);
        assert!(!is_path_allowed_for_writes(&a, "src-backup/foo.rs"));
        assert!(!is_path_allowed_for_writes(&a, "srcsomething"));
    }

    #[test]
    fn exact_file_entry_matches_only_that_file() {
        let a = allowed(&["Cargo.toml"]);
        assert!(is_path_allowed_for_writes(&a, "Cargo.toml"));
        assert!(!is_path_allowed_for_writes(&a, "Cargo.toml.bak"));
        assert!(!is_path_allowed_for_writes(&a, "subdir/Cargo.toml"));
    }

    #[test]
    fn empty_allowlist_rejects_everything() {
        let a: Vec<String> = Vec::new();
        assert!(!is_path_allowed_for_writes(&a, "docs/spec.md"));
        assert!(!is_path_allowed_for_writes(&a, "anything"));
    }

    #[test]
    fn windows_style_separators_normalize_to_forward_slash() {
        // Artifact paths that come back through fenced blocks on
        // Windows shells use backslash separators. The allowlist is
        // declared in forward-slash form, so the matcher normalizes
        // the input before comparing.
        let a = allowed(&["docs/"]);
        assert!(is_path_allowed_for_writes(&a, "docs\\spec.md"));
    }

    #[test]
    fn multiple_entries_match_independently() {
        let a = allowed(&["src/", "tests/", "Cargo.toml"]);
        assert!(is_path_allowed_for_writes(&a, "src/lib.rs"));
        assert!(is_path_allowed_for_writes(&a, "tests/it.rs"));
        assert!(is_path_allowed_for_writes(&a, "Cargo.toml"));
        assert!(!is_path_allowed_for_writes(&a, "docs/spec.md"));
    }

    fn write_milestone(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn dm3b_walk() -> MilestoneWalkConfig {
        MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
            forbid_deferred: false,
        }
    }

    #[test]
    fn find_current_milestone_picks_first_pending_when_not_retrying() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "tb-milestone-01-payloads.md", "- [x] task one\n");
        write_milestone(
            &dir,
            "tb-milestone-02-drivers.md",
            "- [ ] task two\n- [x] task three\n",
        );
        write_milestone(&dir, "tb-milestone-03-monitors.md", "- [ ] task four\n");
        let result = find_current_milestone(tmp.path(), &dm3b_walk(), false);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/test-plan/tb-milestone-02-drivers.md".into())
        );
    }

    #[test]
    fn find_current_milestone_retry_targets_highest_with_checked_row() {
        // Retry: the agent worked on milestone 02 (it has at least
        // one `- [x]`), the critique flagged BLOCKERs about it. Even
        // though milestone 02's rows are also all `- [ ]` again --
        // OR even when the agent prematurely flipped them all to
        // `- [x]` -- the retry must target milestone 02 (the most
        // recent one touched), not jump ahead to milestone 03.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-payloads.md",
            "- [x] all done\n- [x] also done\n",
        );
        // Milestone 02: agent flipped boxes prematurely. All `[x]`
        // but the critique flagged BLOCKERs.
        write_milestone(
            &dir,
            "tb-milestone-02-drivers.md",
            "- [x] flipped early\n- [x] also flipped\n",
        );
        // Milestone 03 not started.
        write_milestone(&dir, "tb-milestone-03-monitors.md", "- [ ] task A\n");
        let result = find_current_milestone(tmp.path(), &dm3b_walk(), true);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/test-plan/tb-milestone-02-drivers.md".into()),
            "retry must target the highest-numbered milestone the agent touched, not advance"
        );
    }

    #[test]
    fn find_current_milestone_returns_all_resolved_when_done() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "tb-milestone-01-payloads.md", "- [x] done\n");
        write_milestone(&dir, "tb-milestone-02-drivers.md", "- [x] done\n");
        let result = find_current_milestone(tmp.path(), &dm3b_walk(), false);
        assert_eq!(result, CurrentMilestone::AllResolved);
    }

    #[test]
    fn find_current_milestone_returns_no_milestones_when_dir_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs/test-plan")).unwrap();
        let result = find_current_milestone(tmp.path(), &dm3b_walk(), false);
        assert_eq!(result, CurrentMilestone::NoMilestonesPresent);
    }

    #[test]
    fn find_current_milestone_ignores_non_milestone_files_in_dir() {
        // The dir also holds `test-plan.md` (index), `coverage.md`,
        // and the OTHER prefix's files (`test-milestone-*.md` for
        // DM3c). The walker must filter by file_prefix and require
        // a digit-prefixed remainder so only `tb-milestone-NN-*.md`
        // counts.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "test-plan.md", "# index\n");
        write_milestone(&dir, "coverage.md", "# cov\n");
        write_milestone(&dir, "test-milestone-01-smoke.md", "- [ ] DM3c task\n");
        write_milestone(&dir, "tb-milestone-01-payloads.md", "- [ ] DM3b task\n");
        let result = find_current_milestone(tmp.path(), &dm3b_walk(), false);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/test-plan/tb-milestone-01-payloads.md".into()),
            "tb-milestone walker must ignore index, coverage, AND the test-milestone-* files"
        );
    }

    #[test]
    fn find_current_milestone_walks_lexicographic_order_with_letter_splits() {
        // Category splits use letter suffixes (02a, 02b). They must
        // walk in lexicographic order between the parent number and
        // the next number.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "test-milestone-01-smoke.md", "- [x] done\n");
        write_milestone(
            &dir,
            "test-milestone-02a-edge-arithmetic.md",
            "- [x] done\n",
        );
        write_milestone(
            &dir,
            "test-milestone-02b-edge-flow.md",
            "- [ ] still pending\n",
        );
        write_milestone(&dir, "test-milestone-03-stress.md", "- [ ] later\n");
        let walk = MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
            forbid_deferred: false,
        };
        let result = find_current_milestone(tmp.path(), &walk, false);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/test-plan/test-milestone-02b-edge-flow.md".into()),
            "split files must walk lexicographically before the next category"
        );
    }

    fn dm3b_step_with_walk() -> StepDescriptor {
        StepDescriptor {
            id: "DM3b",
            flow: Flow::DirectModeling,
            prerequisite: None,
            instruction_slug: "dm3b-testbench-impl",
            per_candidate: false,
            gate_checks: Vec::new(),
            walk_gate_checks: Vec::new(),
            work_artifacts: &["tests/"],
            predecessor_inputs: &[],
            work_write_paths: &["tests/"],
            work_phases: &["chat"],
            critique_phases: &["chat"],
            milestone_walk: Some(dm3b_walk()),
        }
    }

    #[test]
    fn auto_tick_flips_pending_rows_when_path_only_artifact_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(tmp.path().join("tests/testbench")).unwrap();
        std::fs::write(tmp.path().join("tests/testbench/mod.rs"), "// stub\n").unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-scoreboard.md",
            "- [ ] `tests/testbench/mod.rs` -- module root\n",
        );
        let step = dm3b_step_with_walk();
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 1);
        let updated = std::fs::read_to_string(dir.join("tb-milestone-01-scoreboard.md")).unwrap();
        assert!(updated.contains("- [x] `tests/testbench/mod.rs`"));
    }

    #[test]
    fn auto_tick_requires_symbol_match_when_path_carries_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(tmp.path().join("tests/testbench")).unwrap();
        std::fs::write(
            tmp.path().join("tests/testbench/scoreboard.rs"),
            "pub struct RgbPipelineScoreboard;\n",
        )
        .unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-scoreboard.md",
            "- [ ] `tests/testbench/scoreboard.rs::RgbPipelineScoreboard`\n\
             - [ ] `tests/testbench/scoreboard.rs::Missing`\n",
        );
        let step = dm3b_step_with_walk();
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 1, "only the row whose symbol exists should flip");
        let updated = std::fs::read_to_string(dir.join("tb-milestone-01-scoreboard.md")).unwrap();
        assert!(updated.contains("- [x] `tests/testbench/scoreboard.rs::RgbPipelineScoreboard`"));
        assert!(updated.contains("- [ ] `tests/testbench/scoreboard.rs::Missing`"));
    }

    #[test]
    fn auto_tick_leaves_prose_rows_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-scoreboard.md",
            "- [ ] explain the scoreboard pattern in `docs/testbench.md`\n\
             - [ ] add a paragraph about latency alignment\n",
        );
        let step = dm3b_step_with_walk();
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 0);
    }

    #[test]
    fn auto_tick_finds_method_by_last_symbol_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(tmp.path().join("tests/testbench")).unwrap();
        std::fs::write(
            tmp.path().join("tests/testbench/scoreboard.rs"),
            "impl RgbPipelineScoreboard { fn on_input(&mut self) {} }\n",
        )
        .unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-scoreboard.md",
            "- [ ] `tests/testbench/scoreboard.rs::RgbPipelineScoreboard::on_input`\n",
        );
        let step = dm3b_step_with_walk();
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 1);
    }

    #[test]
    fn auto_tick_skips_planning_detail_walks() {
        // Planning-detail steps (placeholder_marker = Some) walk
        // milestone stubs and write task lists that describe what
        // EXECUTION steps will later build. The agent emits all
        // rows as `- [ ]` at this stage. The orchestrator must not
        // auto-tick them just because a row's first backtick token
        // happens to be an existing file path -- doing so would
        // silently mark planning tasks as completed and the next
        // critique would re-flag the mismatch in a perpetual loop.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        // The "resolved" path: this file exists and would be the
        // first backtick token on the task row. For an EXECUTION
        // walk this is exactly the trigger that flips the row.
        std::fs::write(tmp.path().join("docs/test-plan/test-plan.md"), "# index\n").unwrap();
        write_milestone(
            &dir,
            "test-milestone-05-coverage.md",
            "- [ ] Update `docs/test-plan/test-plan.md`'s `## Coverage` section\n",
        );
        let step = StepDescriptor {
            id: "DM3ad",
            flow: Flow::DirectModeling,
            prerequisite: None,
            instruction_slug: "dm3ad-test-plan-detail",
            per_candidate: false,
            gate_checks: Vec::new(),
            walk_gate_checks: Vec::new(),
            work_artifacts: &[],
            predecessor_inputs: &[],
            work_write_paths: &[],
            work_phases: &["chat"],
            critique_phases: &["chat"],
            milestone_walk: Some(MilestoneWalkConfig {
                dir: "docs/test-plan/",
                file_prefixes: &["test-milestone-"],
                index_file: "docs/test-plan/test-plan.md",
                placeholder_marker: Some("<!-- detail-pending"),
                forbid_deferred: false,
            }),
        };
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 0, "planning-detail walks must not auto-tick rows");
        let body = std::fs::read_to_string(dir.join("test-milestone-05-coverage.md")).unwrap();
        assert!(
            body.contains("- [ ]"),
            "row must remain unchecked at planning stage; got:\n{body}"
        );
    }

    #[test]
    fn auto_tick_is_no_op_when_step_has_no_milestone_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let step = StepDescriptor {
            id: "DM2a",
            flow: Flow::DirectModeling,
            prerequisite: None,
            instruction_slug: "dm2a-decomposition",
            per_candidate: false,
            gate_checks: Vec::new(),
            walk_gate_checks: Vec::new(),
            work_artifacts: &[],
            predecessor_inputs: &[],
            work_write_paths: &[],
            work_phases: &["chat"],
            critique_phases: &["chat"],
            milestone_walk: None,
        };
        let flipped = tick_resolved_milestone_tasks(tmp.path(), &step);
        assert_eq!(flipped, 0);
    }

    fn placeholder_walk() -> MilestoneWalkConfig {
        MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        }
    }

    #[test]
    fn find_current_milestone_placeholder_mode_picks_first_with_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        // milestone-01 already detailed (placeholder gone, real
        // tasks landed); milestone-02 still a stub.
        write_milestone(
            &dir,
            "milestone-01-payloads.md",
            "# Milestone 01\n## Tasks\n- [ ] real task\n",
        );
        write_milestone(
            &dir,
            "milestone-02-skeletons.md",
            "# Milestone 02\n## Tasks\n<!-- detail-pending\n",
        );
        let walk = placeholder_walk();
        let result = find_current_milestone(tmp.path(), &walk, false);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/impl-plan/milestone-02-skeletons.md".into()),
            "should target the first stub still carrying the placeholder"
        );
    }

    #[test]
    fn find_current_milestone_placeholder_retry_picks_highest_detailed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "milestone-01-payloads.md",
            "# Milestone 01\n## Tasks\n- [ ] real task\n",
        );
        write_milestone(
            &dir,
            "milestone-02-skeletons.md",
            "# Milestone 02\n## Tasks\n- [ ] another\n",
        );
        write_milestone(
            &dir,
            "milestone-03-stages.md",
            "# Milestone 03\n## Tasks\n<!-- detail-pending\n",
        );
        let walk = placeholder_walk();
        // Retry mode: critique fired on the milestone JUST detailed.
        // The highest-numbered detailed milestone is 02, so we
        // re-target it.
        let result = find_current_milestone(tmp.path(), &walk, true);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/impl-plan/milestone-02-skeletons.md".into()),
            "retry should target highest-numbered milestone whose placeholder is gone"
        );
    }

    #[test]
    fn find_current_milestone_placeholder_all_resolved_when_no_marker_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "milestone-01-payloads.md",
            "# Milestone 01\n## Tasks\n- [ ] task\n",
        );
        write_milestone(
            &dir,
            "milestone-02-skeletons.md",
            "# Milestone 02\n## Tasks\n- [ ] task\n",
        );
        let walk = placeholder_walk();
        let result = find_current_milestone(tmp.path(), &walk, false);
        assert_eq!(result, CurrentMilestone::AllResolved);
    }

    #[test]
    fn find_current_milestone_walks_multiple_prefixes() {
        // DM3ad walks tb-milestone-* and test-milestone-* in the
        // same directory; lexicographic order means tb-* comes
        // first, then test-*. The first stub with placeholder is
        // tb-milestone-02, since tb-milestone-01 is already
        // detailed.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/test-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "tb-milestone-01-payloads.md",
            "# tb-01\n## Tasks\n- [ ] real\n",
        );
        write_milestone(
            &dir,
            "tb-milestone-02-drivers.md",
            "# tb-02\n## Tasks\n<!-- detail-pending\n",
        );
        write_milestone(
            &dir,
            "test-milestone-01-smoke.md",
            "# test-01\n## Tasks\n<!-- detail-pending\n",
        );
        let walk = MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-", "test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: Some("<!-- detail-pending"),
            forbid_deferred: false,
        };
        let result = find_current_milestone(tmp.path(), &walk, false);
        assert_eq!(
            result,
            CurrentMilestone::File("docs/test-plan/tb-milestone-02-drivers.md".into()),
            "lexicographic order across both prefixes; tb-* sorts before test-*"
        );
    }

    #[test]
    fn enumerate_pending_milestones_placeholder_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "milestone-01-payloads.md",
            "# 01\n## Tasks\n- [ ] real\n",
        );
        write_milestone(
            &dir,
            "milestone-02-skeletons.md",
            "# 02\n<!-- detail-pending\n",
        );
        write_milestone(
            &dir,
            "milestone-03-decode.md",
            "# 03\n<!-- detail-pending\n",
        );
        write_milestone(
            &dir,
            "milestone-04-execute.md",
            "# 04\n## Tasks\n- [ ] real\n",
        );
        let walk = placeholder_walk();
        let pending = enumerate_pending_milestones(tmp.path(), &walk);
        match pending {
            PendingMilestones::Present { pending } => assert_eq!(
                pending,
                vec![
                    "docs/impl-plan/milestone-02-skeletons.md".to_string(),
                    "docs/impl-plan/milestone-03-decode.md".to_string(),
                ],
                "placeholder-mode enumerates stubs in walker order"
            ),
            PendingMilestones::DirectoryMissing => panic!("directory exists; expected Present"),
        }
    }

    #[test]
    fn enumerate_pending_milestones_present_but_empty_when_all_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(
            &dir,
            "milestone-01-payloads.md",
            "# 01\n## Tasks\n- [ ] real\n",
        );
        write_milestone(
            &dir,
            "milestone-02-skeletons.md",
            "# 02\n## Tasks\n- [ ] real\n",
        );
        let walk = placeholder_walk();
        let pending = enumerate_pending_milestones(tmp.path(), &walk);
        match pending {
            PendingMilestones::Present { pending } => assert!(
                pending.is_empty(),
                "no stubs with the placeholder remain; got {pending:?}"
            ),
            PendingMilestones::DirectoryMissing => panic!("directory exists; expected Present"),
        }
    }

    #[test]
    fn enumerate_pending_milestones_returns_directory_missing_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let walk = placeholder_walk();
        let pending = enumerate_pending_milestones(tmp.path(), &walk);
        assert_eq!(
            pending,
            PendingMilestones::DirectoryMissing,
            "missing directory is distinct from present-but-empty"
        );
        // Helper `is_empty()` collapses both states for callers
        // that don't care about the distinction.
        assert!(pending.is_empty());
    }

    #[test]
    fn find_milestone_by_name_bare_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "milestone-01-foo.md", "# 01\n");
        write_milestone(&dir, "milestone-02-bar.md", "# 02\n");
        let walk = placeholder_walk();
        let found = find_milestone_by_name(tmp.path(), &walk, "milestone-02-bar.md");
        assert_eq!(
            found,
            CurrentMilestone::File("docs/impl-plan/milestone-02-bar.md".into())
        );
    }

    #[test]
    fn find_milestone_by_name_accepts_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "milestone-01-foo.md", "# 01\n");
        let walk = placeholder_walk();
        let found = find_milestone_by_name(tmp.path(), &walk, "docs/impl-plan/milestone-01-foo.md");
        assert_eq!(
            found,
            CurrentMilestone::File("docs/impl-plan/milestone-01-foo.md".into())
        );
    }

    #[test]
    fn find_milestone_by_name_returns_not_present_for_unknown_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        write_milestone(&dir, "milestone-01-foo.md", "# 01\n");
        let walk = placeholder_walk();
        let found = find_milestone_by_name(tmp.path(), &walk, "milestone-99-nope.md");
        assert_eq!(found, CurrentMilestone::NoMilestonesPresent);
    }

    #[test]
    fn find_milestone_by_name_resolves_regardless_of_pending_state() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        // Fully detailed milestone (no placeholder) -- the parallel
        // walker still needs to scope a Critique session to it.
        write_milestone(&dir, "milestone-01-foo.md", "# 01\n## Tasks\n- [ ] real\n");
        let walk = placeholder_walk();
        let found = find_milestone_by_name(tmp.path(), &walk, "milestone-01-foo.md");
        assert_eq!(
            found,
            CurrentMilestone::File("docs/impl-plan/milestone-01-foo.md".into()),
            "resolved milestones must still be addressable by name"
        );
    }

    fn dm2d_like_walk() -> MilestoneWalkConfig {
        MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: None,
            forbid_deferred: true,
        }
    }

    #[test]
    fn reset_checkbox_milestones_unticks_all_marked_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("milestone-01-payload-types.md");
        std::fs::write(
            &path,
            "# Milestone 1\n\
             \n\
             - [x] First task\n\
             - [X] Second task\n\
             - [-] Third deferred\n\
             - [ ] Fourth still open\n\
             \n\
             Some prose that has `- [x]` inline but no leading marker -- preserved.\n",
        )
        .unwrap();

        let walk = dm2d_like_walk();
        let touched = walk.reset(tmp.path()).unwrap();
        assert_eq!(touched.len(), 1, "the one milestone file got rewritten");

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("- [ ] First task"),
            "[x] flipped to [ ]: {body}"
        );
        assert!(
            body.contains("- [ ] Second task"),
            "[X] flipped to [ ]: {body}"
        );
        assert!(
            body.contains("- [ ] Third deferred"),
            "[-] flipped to [ ]: {body}"
        );
        assert!(
            body.contains("- [ ] Fourth still open"),
            "already-empty rows untouched: {body}"
        );
        assert!(
            body.contains("inline but no leading marker -- preserved."),
            "prose mid-line `- [x]` untouched: {body}",
        );
    }

    #[test]
    fn reset_checkbox_milestones_skips_non_milestone_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        // plan.md and plan-management.md don't match `milestone-NN-*`.
        // They must NOT be touched, even though they may contain `- [x]`.
        let plan = dir.join("plan.md");
        std::fs::write(&plan, "# Plan\n\n- [x] meta-task\n").unwrap();
        let m01 = dir.join("milestone-01-foo.md");
        std::fs::write(&m01, "- [x] real task\n").unwrap();

        let walk = dm2d_like_walk();
        let touched = walk.reset(tmp.path()).unwrap();
        assert_eq!(touched.len(), 1, "only the milestone file was touched");
        assert!(touched[0].ends_with("milestone-01-foo.md"));

        assert_eq!(
            std::fs::read_to_string(&plan).unwrap(),
            "# Plan\n\n- [x] meta-task\n",
            "plan.md is untouched: it doesn't match `milestone-NN-*`",
        );
        assert!(
            std::fs::read_to_string(&m01).unwrap().contains("- [ ]"),
            "milestone-01 was reset",
        );
    }

    #[test]
    fn reset_checkbox_milestones_is_idempotent_and_no_op_when_all_open() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/impl-plan");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("milestone-01-x.md");
        std::fs::write(&path, "- [ ] open\n- [ ] also open\n").unwrap();

        let walk = dm2d_like_walk();
        let touched = walk.reset(tmp.path()).unwrap();
        assert!(touched.is_empty(), "no rewrite when nothing to flip");

        // Second call still a no-op.
        let touched = walk.reset(tmp.path()).unwrap();
        assert!(touched.is_empty());
    }

    #[test]
    fn reset_checkbox_milestones_no_op_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let walk = dm2d_like_walk();
        let touched = walk.reset(tmp.path()).unwrap();
        assert!(touched.is_empty(), "missing dir resets to nothing");
    }

    #[test]
    fn placeholder_mode_reset_is_a_silent_noop() {
        // Placeholder-mode milestones (DM2cd / DM3ad / DM4ad) can't be
        // reset in place -- the stub content isn't preserved. The
        // reset cascade has to handle that by also resetting the
        // upstream outline step; here we just need to NOT propagate
        // an error that would abort the rest of the cascade.
        let walk = placeholder_walk();
        let tmp = tempfile::tempdir().unwrap();
        let touched = walk.reset(tmp.path()).unwrap();
        assert!(
            touched.is_empty(),
            "placeholder mode resets nothing in place"
        );
    }
}
