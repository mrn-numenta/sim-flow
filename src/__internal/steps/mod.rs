//! Step registry.
//!
//! Every flow (DM, DS) registers its step descriptors here. Each descriptor
//! captures the step id, its ordered position in the flow, its prerequisite
//! step (if any), the slug used to look up instruction files, whether the
//! step is per-candidate, and its gate checks.

pub mod dm;
pub mod ds;

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
    "search",
    "run_cargo",
];

#[derive(Debug, Clone)]
pub struct StepDescriptor {
    pub id: &'static str,
    pub flow: Flow,
    pub prerequisite: Option<&'static str>,
    pub instruction_slug: &'static str,
    pub per_candidate: bool,
    pub gate_checks: Vec<GateCheck>,
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
    /// Phase pipeline for the work session, e.g. `["chat"]` for
    /// non-code steps or `["author", "build", "test"]` for code
    /// steps. The orchestrator runs validators between phases; see
    /// docs/architecture/ai-flow/08-orchestrator-tools.md.
    pub work_phases: &'static [&'static str],
    /// Phase pipeline for the critique session. Always `["chat"]`
    /// in M3.
    pub critique_phases: &'static [&'static str],
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
    }
    reg
}
