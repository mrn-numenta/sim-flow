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
}

/// Allowed write-path prefixes for the given (step, kind). Work
/// sessions return the step's `work_write_paths`; critique sessions
/// always return a single-entry list with the canonical critique
/// filename. Path enforcement uses prefix-match for entries ending in
/// `/` and exact-match otherwise.
pub fn allowed_write_paths(step: &StepDescriptor, kind: crate::client::SessionKind) -> Vec<String> {
    match kind {
        crate::client::SessionKind::Work => step
            .work_write_paths
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        crate::client::SessionKind::Critique => {
            vec![format!("docs/critiques/{}-critique.md", step.id)]
        }
    }
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
    }
    reg
}

#[cfg(test)]
mod write_path_tests {
    use super::is_path_allowed_for_writes;

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
}
