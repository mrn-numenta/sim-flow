//! Shared `GateCheck` constructors used by every DM tier file.
//!
//! Re-exported via `pub(super) use super::helpers::*` from each
//! tier module so the per-step descriptors read like a DSL
//! (`file_matches(...)`, `shell(...)`) instead of repeating the
//! enum-variant boilerplate.

use std::path::PathBuf;

use crate::gate::GateCheck;

pub(super) fn critique_clean(step: &str) -> GateCheck {
    GateCheck::CritiqueClean {
        path: PathBuf::from(format!("docs/critiques/{step}-critique.md")),
        description: format!("{step} critique has no blockers"),
    }
}

/// Sharded critique variant for the plan-detail parallel walks
/// (DM2cd / DM3ad / DM4ad). Each parallel worker writes its own
/// `docs/critiques/<step>/<milestone>.json` shard via
/// [`crate::__internal::worktree::merge_contributions`]; this gate
/// scans the whole directory and collects every blocker /
/// unresolved across every shard without early exit.
pub(super) fn critique_dir_clean(step: &str) -> GateCheck {
    GateCheck::CritiqueClean {
        path: PathBuf::from(format!("docs/critiques/{step}")),
        description: format!("{step} per-milestone critique shards have no blockers"),
    }
}

pub(super) fn file_exists(path: &str, description: &str) -> GateCheck {
    GateCheck::FileExists {
        path: PathBuf::from(path),
        description: description.to_string(),
    }
}

pub(super) fn file_matches(path: &str, pattern: &str, description: &str) -> GateCheck {
    GateCheck::FileMatches {
        path: PathBuf::from(path),
        pattern: pattern.to_string(),
        description: description.to_string(),
    }
}

pub(super) fn shell(cmd: &str, args: &[&str], description: &str) -> GateCheck {
    GateCheck::Shell {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        description: description.to_string(),
    }
}

pub(super) fn any_exists(paths: &[&str], description: &str) -> GateCheck {
    GateCheck::AnyExists {
        paths: paths.iter().map(PathBuf::from).collect(),
        description: description.to_string(),
    }
}

pub(super) fn any_matches(paths: &[&str], pattern: &str, description: &str) -> GateCheck {
    GateCheck::AnyMatches {
        paths: paths.iter().map(PathBuf::from).collect(),
        pattern: pattern.to_string(),
        description: description.to_string(),
    }
}

/// `GateCheck::SpecMdStructured` constructor used by DM0 to swap the
/// legacy regex-driven dispatch for the Phase 1 parser + validator.
/// `spec_md_path` and `manifest_path` are relative to the project
/// dir; the evaluator joins them with the project root.
pub(super) fn spec_md_structured(
    spec_md_path: &str,
    manifest_path: Option<&str>,
    description: &str,
) -> GateCheck {
    GateCheck::SpecMdStructured {
        spec_md_path: PathBuf::from(spec_md_path),
        manifest_path: manifest_path.map(PathBuf::from),
        description: description.to_string(),
    }
}

/// Pair this with a `StepDescriptor::milestone_walk` so the step's
/// gate cannot pass while any milestone file under `dir` is still
/// pending. Defaults to execution-step semantics (`- [ ]` rows must
/// resolve); use `milestones_all_detailed` for planning-detail
/// steps where the placeholder-marker mode applies. `- [-]`
/// (deferred) rows are TREATED AS RESOLVED here; use
/// `milestones_all_implemented` (forbid_deferred=true) for steps
/// where deferring drops downstream-required work.
pub(super) fn milestones_all_resolved(
    dir: &str,
    file_prefix: &str,
    description: &str,
) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: vec![file_prefix.to_string()],
        placeholder_marker: None,
        description: description.to_string(),
        forbid_deferred: false,
    }
}

/// Strict execution-mode variant: like `milestones_all_resolved`
/// but `- [-]` rows ALSO count as pending. Used by DM2d / DM3c /
/// DM4b -- the model-impl, test-impl, perf-impl gates -- where a
/// silent "defer this task" by the agent would leak into the
/// downstream step's predecessor inputs and the work would never
/// get done. DM3b (testbench skeletons) keeps the lenient default
/// since some integration shims can legitimately be deferred to
/// DM3c.
pub(super) fn milestones_all_implemented(
    dir: &str,
    file_prefix: &str,
    description: &str,
) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: vec![file_prefix.to_string()],
        placeholder_marker: None,
        description: description.to_string(),
        forbid_deferred: true,
    }
}

/// Planning-detail variant of `milestones_all_resolved`: gate is
/// clean iff no milestone file under `dir` (matching any prefix in
/// `file_prefixes`) still contains `placeholder_marker` in its body.
/// The detail step replaces stub bodies with full task lists; the
/// outline step's `- [ ]` task rows are intentionally left pending
/// (they're for the downstream execution step), so the row-count
/// gate would never advance here.
pub(super) fn milestones_all_detailed(
    dir: &str,
    file_prefixes: &[&str],
    placeholder_marker: &str,
    description: &str,
) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: file_prefixes.iter().map(|s| (*s).to_string()).collect(),
        placeholder_marker: Some(placeholder_marker.to_string()),
        description: description.to_string(),
        forbid_deferred: false,
    }
}
