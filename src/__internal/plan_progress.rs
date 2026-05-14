//! Plan-execution progress for the dashboard.
//!
//! Closes one of the MVP-audit items: the dashboard used to parse
//! `docs/<plan>/milestone-NN-*.md` files directly from TypeScript;
//! now the orchestrator owns the parser and emits JSON via
//! `sim-flow plan-progress --json` so any UI surface (VS Code,
//! future web, terminal) consumes the same shape.
//!
//! Three plan shapes are walked, all milestone-per-file:
//!
//!   1. Implementation plan (DM2c/DM2cd write, DM2d executes):
//!      `docs/impl-plan/milestone-NN-<name>.md`.
//!   2. Test plan (DM3a/DM3ad write, DM3b/DM3c execute):
//!      `docs/test-plan/tb-milestone-NN-*.md` (DM3b's slices) +
//!      `docs/test-plan/test-milestone-NN-*.md` (DM3c's slices).
//!      Combined into one pipeline.
//!   3. Performance plan (DM4a/DM4ad write, DM4b executes):
//!      `docs/perf-plan/perf-milestone-NN-<name>.md`.
//!
//! The "current task" is the first `- [ ]` row in the most-recently-
//! modified milestone with pending rows; falls back to "first
//! pending in plan order" when modification times don't help.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    Impl,
    Test,
    Perf,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanMilestone {
    pub id: String,
    pub title: String,
    pub file_path: String,
    /// Always None for milestone-per-file plans; reserved for future
    /// section-driven plans that the dashboard already supports.
    pub file_line: Option<u32>,
    pub done: u32,
    pub deferred: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanProgress {
    pub kind: PlanKind,
    pub milestones: Vec<PlanMilestone>,
    pub current_task: Option<String>,
    pub current_task_file_path: Option<String>,
    pub current_task_line: Option<u32>,
}

impl PlanProgress {
    fn empty(kind: PlanKind) -> Self {
        Self {
            kind,
            milestones: Vec::new(),
            current_task: None,
            current_task_file_path: None,
            current_task_line: None,
        }
    }
}

/// Map a step id to which plan kind drives it. Steps that don't
/// drive a plan return `PlanKind::None`.
pub fn plan_kind_for_step(current_step: &str) -> PlanKind {
    match current_step {
        "DM2c" | "DM2cd" | "DM2d" => PlanKind::Impl,
        "DM3a" | "DM3ad" | "DM3b" | "DM3c" => PlanKind::Test,
        "DM4a" | "DM4ad" | "DM4b" => PlanKind::Perf,
        _ => PlanKind::None,
    }
}

struct PlanDirConfig {
    dir: &'static str,
    prefixes: &'static [&'static str],
}

const IMPL_CONFIG: PlanDirConfig = PlanDirConfig {
    dir: "docs/impl-plan",
    prefixes: &["milestone-"],
};
const TEST_CONFIG: PlanDirConfig = PlanDirConfig {
    dir: "docs/test-plan",
    prefixes: &["tb-milestone-", "test-milestone-"],
};
const PERF_CONFIG: PlanDirConfig = PlanDirConfig {
    dir: "docs/perf-plan",
    prefixes: &["perf-milestone-"],
};

fn config_for(kind: PlanKind) -> Option<&'static PlanDirConfig> {
    match kind {
        PlanKind::Impl => Some(&IMPL_CONFIG),
        PlanKind::Test => Some(&TEST_CONFIG),
        PlanKind::Perf => Some(&PERF_CONFIG),
        PlanKind::None => None,
    }
}

pub fn read_plan_progress_for_kind(project_dir: &Path, kind: PlanKind) -> PlanProgress {
    let Some(cfg) = config_for(kind) else {
        return PlanProgress::empty(PlanKind::None);
    };
    let plan_dir = project_dir.join(cfg.dir);
    if !plan_dir.exists() {
        return PlanProgress::empty(kind);
    }
    let Ok(entries) = std::fs::read_dir(&plan_dir) else {
        return PlanProgress::empty(kind);
    };
    let mut milestone_files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.ends_with(".md") || name.ends_with("-critique.md") {
                return None;
            }
            if !cfg.prefixes.iter().any(|p| name.starts_with(*p)) {
                return None;
            }
            Some((name, path))
        })
        .collect();
    if milestone_files.is_empty() {
        return PlanProgress::empty(kind);
    }
    milestone_files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut milestones = Vec::with_capacity(milestone_files.len());
    let mut most_recent: Option<(std::time::SystemTime, usize)> = None;
    for (name, path) in &milestone_files {
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let counts = count_checkboxes(&body);
        let (id, title) = milestone_label(cfg.prefixes, name);
        milestones.push(PlanMilestone {
            id,
            title,
            file_path: path.to_string_lossy().into_owned(),
            file_line: None,
            done: counts.done,
            deferred: counts.deferred,
            pending: counts.pending,
        });
        if counts.pending > 0
            && most_recent
                .as_ref()
                .is_none_or(|(prev_mtime, _)| mtime > *prev_mtime)
        {
            most_recent = Some((mtime, milestones.len() - 1));
        }
    }

    let target_idx = most_recent
        .map(|(_, idx)| idx)
        .or_else(|| milestones.iter().position(|m| m.pending > 0));

    let (current_task, current_task_file_path, current_task_line) = match target_idx {
        Some(idx) => {
            let target = &milestones[idx];
            let body = std::fs::read_to_string(&target.file_path).unwrap_or_default();
            match first_pending_row(&body) {
                Some((text, line)) => (Some(text), Some(target.file_path.clone()), Some(line)),
                None => (None, None, None),
            }
        }
        None => (None, None, None),
    };

    PlanProgress {
        kind,
        milestones,
        current_task,
        current_task_file_path,
        current_task_line,
    }
}

/// Convenience: pick the plan that applies to the project's current
/// step (per `state.toml`), or `kind = None` when the step doesn't
/// drive a plan.
pub fn read_plan_progress(project_dir: &Path, current_step: &str) -> PlanProgress {
    let kind = plan_kind_for_step(current_step);
    read_plan_progress_for_kind(project_dir, kind)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllPlanProgress {
    #[serde(rename = "impl")]
    pub impl_: PlanProgress,
    pub test: PlanProgress,
    pub perf: PlanProgress,
}

/// Read progress for all three plan kinds in one call. Matches the
/// dashboard's "every plan-related step shows the milestone pipeline
/// regardless of current_step" behavior.
pub fn read_all_plan_progress(project_dir: &Path) -> AllPlanProgress {
    AllPlanProgress {
        impl_: read_plan_progress_for_kind(project_dir, PlanKind::Impl),
        test: read_plan_progress_for_kind(project_dir, PlanKind::Test),
        perf: read_plan_progress_for_kind(project_dir, PlanKind::Perf),
    }
}

struct CheckboxCounts {
    done: u32,
    deferred: u32,
    pending: u32,
}

fn count_checkboxes(text: &str) -> CheckboxCounts {
    let mut counts = CheckboxCounts {
        done: 0,
        deferred: 0,
        pending: 0,
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Match `- [.]` or `* [.]` where `.` is space / x / X / -.
        let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        else {
            continue;
        };
        let Some(after_open) = rest.strip_prefix('[') else {
            continue;
        };
        let mut chars = after_open.chars();
        let Some(state) = chars.next() else {
            continue;
        };
        if chars.next() != Some(']') {
            continue;
        }
        match state {
            ' ' => counts.pending += 1,
            'x' | 'X' => counts.done += 1,
            '-' => counts.deferred += 1,
            _ => {}
        }
    }
    counts
}

fn first_pending_row(text: &str) -> Option<(String, u32)> {
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        else {
            continue;
        };
        // Require exactly `[ ]` (pending). `[x]`, `[X]`, `[-]` skip.
        let Some(after) = rest.strip_prefix("[ ] ") else {
            continue;
        };
        let task = after.trim().to_string();
        if !task.is_empty() {
            return Some((task, idx as u32));
        }
    }
    None
}

fn milestone_label(prefixes: &[&str], filename: &str) -> (String, String) {
    // Match the longest registered prefix that the filename starts
    // with -- otherwise "tb-milestone-" would shadow
    // "test-milestone-" for `tb-` files.
    let matched_prefix = prefixes
        .iter()
        .filter(|p| filename.starts_with(*p))
        .max_by_key(|p| p.len())
        .copied()
        .unwrap_or("");
    let remainder = filename
        .strip_prefix(matched_prefix)
        .unwrap_or(filename)
        .strip_suffix(".md")
        .unwrap_or(filename);
    let (num_part, title_part) = match remainder.find('-') {
        Some(pos) => (&remainder[..pos], &remainder[pos + 1..]),
        None => (remainder, ""),
    };
    let is_num =
        !num_part.is_empty() && num_part.chars().next().is_some_and(|c| c.is_ascii_digit());
    let id = if is_num {
        format!("M{num_part}")
    } else {
        filename.to_string()
    };
    let title_suffix = if is_num {
        title_part.replace('-', " ")
    } else {
        filename.trim_end_matches(".md").to_string()
    };
    let prefix_tag = match matched_prefix {
        "tb-milestone-" => Some("TB"),
        "test-milestone-" => Some("Test"),
        _ => None,
    };
    let title = match prefix_tag {
        Some(tag) if !title_suffix.is_empty() => format!("{tag} {id}: {title_suffix}"),
        Some(tag) => format!("{tag} {id}"),
        None if !title_suffix.is_empty() => format!("{id}: {title_suffix}"),
        None => id.clone(),
    };
    (id, title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn plan_kind_for_step_covers_all_plan_steps() {
        assert_eq!(plan_kind_for_step("DM2d"), PlanKind::Impl);
        assert_eq!(plan_kind_for_step("DM3c"), PlanKind::Test);
        assert_eq!(plan_kind_for_step("DM4b"), PlanKind::Perf);
        assert_eq!(plan_kind_for_step("DM0"), PlanKind::None);
        assert_eq!(plan_kind_for_step("DM2a"), PlanKind::None);
    }

    #[test]
    fn empty_when_plan_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let prog = read_plan_progress(tmp.path(), "DM4b");
        assert_eq!(prog.kind, PlanKind::Perf);
        assert!(prog.milestones.is_empty());
        assert_eq!(prog.current_task, None);
    }

    #[test]
    fn counts_checkboxes_across_all_states() {
        let text = "- [ ] one\n- [x] two\n- [X] three\n- [-] four\n* [ ] five\n";
        let counts = count_checkboxes(text);
        assert_eq!(counts.pending, 2);
        assert_eq!(counts.done, 2);
        assert_eq!(counts.deferred, 1);
    }

    #[test]
    fn first_pending_row_returns_text_and_line() {
        let text = "- [x] done\n- [ ] this one\n- [ ] not this\n";
        let (task, line) = first_pending_row(text).expect("present");
        assert_eq!(task, "this one");
        assert_eq!(line, 1);
    }

    #[test]
    fn first_pending_row_none_when_all_resolved() {
        let text = "- [x] one\n- [-] two\n";
        assert!(first_pending_row(text).is_none());
    }

    #[test]
    fn reads_impl_plan_milestones_and_picks_current_task() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/impl-plan/milestone-01-foo.md"),
            "- [x] alpha\n- [ ] bravo\n",
        );
        write(
            &tmp.path().join("docs/impl-plan/milestone-02-bar.md"),
            "- [ ] charlie\n",
        );
        let prog = read_plan_progress(tmp.path(), "DM2d");
        assert_eq!(prog.kind, PlanKind::Impl);
        assert_eq!(prog.milestones.len(), 2);
        assert_eq!(prog.milestones[0].id, "M01");
        assert_eq!(prog.milestones[0].done, 1);
        assert_eq!(prog.milestones[0].pending, 1);
        assert_eq!(prog.milestones[1].id, "M02");
        assert!(prog.current_task.is_some());
    }

    #[test]
    fn test_plan_combines_tb_and_test_prefixes() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/test-plan/tb-milestone-01-tb.md"),
            "- [ ] tb-task\n",
        );
        write(
            &tmp.path().join("docs/test-plan/test-milestone-01-test.md"),
            "- [ ] test-task\n",
        );
        let prog = read_plan_progress(tmp.path(), "DM3c");
        assert_eq!(prog.kind, PlanKind::Test);
        assert_eq!(prog.milestones.len(), 2);
        // tb-milestone- sorts before test-milestone- lexicographically.
        assert_eq!(prog.milestones[0].title, "TB M01: tb");
        assert_eq!(prog.milestones[1].title, "Test M01: test");
    }

    #[test]
    fn critique_files_are_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/perf-plan/perf-milestone-01-foo.md"),
            "- [ ] task\n",
        );
        write(
            &tmp.path()
                .join("docs/perf-plan/perf-milestone-01-foo-critique.md"),
            "- [ ] critique\n",
        );
        let prog = read_plan_progress(tmp.path(), "DM4b");
        assert_eq!(prog.milestones.len(), 1);
    }

    #[test]
    fn all_plan_progress_walks_three_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/impl-plan/milestone-01-foo.md"),
            "- [x] done\n",
        );
        write(
            &tmp.path().join("docs/perf-plan/perf-milestone-01-bar.md"),
            "- [ ] pending\n",
        );
        let all = read_all_plan_progress(tmp.path());
        assert_eq!(all.impl_.kind, PlanKind::Impl);
        assert_eq!(all.impl_.milestones.len(), 1);
        assert_eq!(all.test.kind, PlanKind::Test);
        assert!(all.test.milestones.is_empty());
        assert_eq!(all.perf.kind, PlanKind::Perf);
        assert_eq!(all.perf.milestones.len(), 1);
    }

    #[test]
    fn current_task_filled_when_at_least_one_pending_exists() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/impl-plan/milestone-01-foo.md"),
            "- [ ] early\n",
        );
        write(
            &tmp.path().join("docs/impl-plan/milestone-02-bar.md"),
            "- [ ] late\n",
        );
        let prog = read_plan_progress(tmp.path(), "DM2d");
        assert!(prog.current_task.is_some());
        assert!(prog.current_task_file_path.is_some());
    }
}
