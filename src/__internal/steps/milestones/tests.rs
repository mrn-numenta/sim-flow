//! Tests for the milestone walker, state checks, reset, and tick.
//!
//! Co-located in a single test module rather than per-submodule because
//! the helper fixtures (write_milestone, dm3b_walk, dm3b_step_with_walk,
//! placeholder_walk, dm2d_like_walk) cross-cut. The submodule
//! visibility lets us call reset_checkbox_milestones() and the other
//! pub(super) functions via the trait-method facade where appropriate.

use crate::state::Flow;
use crate::steps::StepDescriptor;

use super::tick::tick_resolved_milestone_tasks;
use super::walk::{enumerate_pending_milestones, find_current_milestone, find_milestone_by_name};
use super::{CurrentMilestone, MilestoneManager, MilestoneWalkConfig, PendingMilestones};

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
