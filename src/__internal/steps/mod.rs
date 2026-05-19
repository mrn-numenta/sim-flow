//! Step registry.
//!
//! Every flow (DM, DS) registers its step descriptors here. Each descriptor
//! captures the step id, its ordered position in the flow, its prerequisite
//! step (if any), the slug used to look up instruction files, whether the
//! step is per-candidate, and its gate checks.

pub mod dm;
pub mod ds;
pub mod milestones;
pub mod sv;
pub mod write_paths;

pub use milestones::{
    CurrentMilestone, MilestoneManager, MilestoneWalkConfig, PendingMilestones,
    enumerate_pending_milestones, find_current_milestone, find_milestone_by_name,
    milestone_is_resolved, tick_resolved_milestone_tasks,
};
pub use write_paths::{
    READ_ONLY_CONVENTION_PATHS, READ_ONLY_EXTENSIONS, ReadOnlyReason, allowed_write_paths,
    classify_read_only, is_path_allowed_for_writes, is_read_only_convention_path,
};

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
    "read_markdown",
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
    // Phase 5 (Chapter 4) tools: three retrieval tools backed by the
    // lance index plus the user-interaction `ask_user` tool. These
    // require runtime state (RetrievalService / AskUserRuntime);
    // call sites that don't have them get the catalog without these
    // entries -- the dispatcher's stateful builder
    // (`build_dispatcher_with_runtime`) is the production wiring.
    "api_semantic_search",
    "spec_semantic_search",
    "signal_table_query",
    "ask_user",
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
