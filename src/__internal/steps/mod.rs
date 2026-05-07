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
    /// Filename prefix (before the `NN-...` suffix) that
    /// identifies milestone files in `dir`. Example:
    /// `"tb-milestone-"` for DM3b, `"test-milestone-"` for DM3c.
    pub file_prefix: &'static str,
    /// Project-relative path of the plan-index file that lives
    /// alongside the milestone files. Always inlined in the
    /// session inputs so the agent can see the TOC, traceability,
    /// etc. Example: `"docs/test-plan/test-plan.md"`.
    pub index_file: &'static str,
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
/// Modes:
/// - **Fresh / advance** (`prior_critique_has_blockers = false`):
///   return the FIRST milestone file (lexicographic order) that
///   has at least one open `- [ ]` row. This is the "no retry,
///   move forward" path.
/// - **Retry** (`prior_critique_has_blockers = true`): return the
///   HIGHEST-NUMBERED milestone file that already has at least
///   one `- [x]` row (i.e., the agent's most recent target).
///   The critique flagged BLOCKERs about that milestone and the
///   retry must address THE SAME milestone, not jump ahead --
///   even if the agent prematurely flipped its rows to `- [x]`.
///   Falls back to the first-pending behavior if no `- [x]` rows
///   exist anywhere (no work has actually started).
///
/// Filenames are matched case-sensitively against
/// `<file_prefix><digits>...md`. The lexicographic sort over the
/// matching subset gives the natural numeric order as long as
/// callers pad NN to two digits (`01`, `02`, ...), which the
/// prompts and `plan-management.md` enforce.
pub fn find_current_milestone(
    project_dir: &std::path::Path,
    walk: &MilestoneWalkConfig,
    prior_critique_has_blockers: bool,
) -> CurrentMilestone {
    let dir = project_dir.join(walk.dir.trim_end_matches('/'));
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return CurrentMilestone::NoMilestonesPresent,
    };
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(walk.file_prefix) || !name.ends_with(".md") {
            continue;
        }
        // Skip the prefix and require a digit-prefixed remainder
        // so `plan.md` / `plan-management.md` etc. don't sneak in
        // when the file_prefix is short.
        let rest = &name[walk.file_prefix.len()..];
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
    if files.is_empty() {
        return CurrentMilestone::NoMilestonesPresent;
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    if prior_critique_has_blockers {
        // Retry mode: highest-numbered milestone with at least
        // one `- [x]` row.
        for (name, path) in files.iter().rev() {
            if milestone_has_checked_row(path) {
                let rel = format!(
                    "{}{}",
                    if walk.dir.ends_with('/') {
                        walk.dir.to_string()
                    } else {
                        format!("{}/", walk.dir)
                    },
                    name
                );
                return CurrentMilestone::File(rel);
            }
        }
        // No `- [x]` anywhere yet: fall through to first-pending.
    }

    // Fresh / advance: first milestone with at least one `- [ ]`.
    for (name, path) in &files {
        if milestone_has_pending_row(path) {
            let rel = format!(
                "{}{}",
                if walk.dir.ends_with('/') {
                    walk.dir.to_string()
                } else {
                    format!("{}/", walk.dir)
                },
                name
            );
            return CurrentMilestone::File(rel);
        }
    }
    CurrentMilestone::AllResolved
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
    let CurrentMilestone::File(rel) = find_current_milestone(project_dir, &walk, true) else {
        return 0;
    };
    let path = project_dir.join(&rel);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let mut flipped = 0usize;
    let new_body: String = body
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- [ ]") {
                return line.to_string();
            }
            let after = trimmed.trim_start_matches("- [ ]").trim_start();
            let Some(token) = first_backtick_token(after) else {
                return line.to_string();
            };
            if !task_artifact_resolved(project_dir, token) {
                return line.to_string();
            }
            flipped += 1;
            line.replacen("- [ ]", "- [x]", 1)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let new_body = if body.ends_with('\n') && !new_body.ends_with('\n') {
        format!("{new_body}\n")
    } else {
        new_body
    };
    if flipped > 0 && std::fs::write(&path, new_body).is_err() {
        return 0;
    }
    flipped
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
            file_prefix: "tb-milestone-",
            index_file: "docs/test-plan/test-plan.md",
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
            file_prefix: "test-milestone-",
            index_file: "docs/test-plan/test-plan.md",
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
    fn auto_tick_is_no_op_when_step_has_no_milestone_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let step = StepDescriptor {
            id: "DM2a",
            flow: Flow::DirectModeling,
            prerequisite: None,
            instruction_slug: "dm2a-decomposition",
            per_candidate: false,
            gate_checks: Vec::new(),
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
}
