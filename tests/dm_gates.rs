//! Exercise the Phase 3 DMF gate checks against staged artifacts without
//! running any AI client. These tests validate that the gate descriptors
//! registered in `steps/dm.rs` accept well-formed project state and
//! reject the obvious failure modes.

use std::path::{Path, PathBuf};

use sim_flow::gate;
use sim_flow::state::Flow;
use sim_flow::steps::{StepDescriptor, registry_for};

fn step(id: &str) -> StepDescriptor {
    let reg = registry_for(Flow::DirectModeling);
    reg.get(id)
        .unwrap_or_else(|| panic!("{id} must exist"))
        .clone()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn clean_critique(project: &Path, step: &str) {
    write(
        &project.join(format!("docs/critiques/{step}-critique.md")),
        "- RESOLVED: looks good\n",
    );
}

fn evaluate(project: &Path, step: &str) -> gate::GateReport {
    gate::evaluate(project, &self::step(step).gate_checks).unwrap()
}

fn assert_clean(report: gate::GateReport, step: &str) {
    assert!(
        report.is_clean(),
        "{step} expected clean, got failures: {:?}",
        report.failures
    );
}

fn assert_fails_with(report: &gate::GateReport, needle: &str) {
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.reason.contains(needle) || f.description.contains(needle)),
        "expected failure mentioning {needle:?}; got {:?}",
        report.failures
    );
}

fn new_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    (tmp, project)
}

#[test]
fn dm0_gate_accepts_well_formed_spec() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/spec.md"),
        "# Design Spec\nClock: 2 GHz\nTech node: 7 nm\n",
    );
    clean_critique(&project, "DM0");
    assert_clean(evaluate(&project, "DM0"), "DM0");
}

#[test]
fn dm0_gate_rejects_missing_spec() {
    let (_tmp, project) = new_project();
    let report = evaluate(&project, "DM0");
    assert_fails_with(&report, "docs/spec.md");
}

#[test]
fn dm0_gate_rejects_missing_frequency() {
    let (_tmp, project) = new_project();
    write(&project.join("docs/spec.md"), "no frequency, tech 7 nm\n");
    clean_critique(&project, "DM0");
    let report = evaluate(&project, "DM0");
    assert!(!report.is_clean());
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.description.contains("frequency"))
    );
}

#[test]
fn dm0_gate_rejects_blocker_in_critique() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/spec.md"),
        "Clock: 1.5 GHz tech 7 nm functional\n",
    );
    write(
        &project.join("docs/critiques/DM0-critique.md"),
        "- BLOCKER: no functional description\n",
    );
    let report = evaluate(&project, "DM0");
    assert!(!report.is_clean());
    assert_fails_with(&report, "critique");
}

#[test]
fn dm1_gate_accepts_basic_setup() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/targets.md"),
        "# Targets\n\n| metric | value |\n| ------ | ----- |\n| throughput | 1.0 items/cycle |\n",
    );
    write(
        &project.join("docs/testbench.md"),
        "Sequencer: uniform\nDriver: ingress\nMonitor: egress\nScoreboard: ordering\n",
    );
    clean_critique(&project, "DM1");
    assert_clean(evaluate(&project, "DM1"), "DM1");
}

#[test]
fn dm1_gate_rejects_missing_testbench_components() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/targets.md"),
        "throughput: 1.0 items/cycle\n",
    );
    write(&project.join("docs/testbench.md"), "just some text\n");
    clean_critique(&project, "DM1");
    let report = evaluate(&project, "DM1");
    assert!(!report.is_clean());
    assert_fails_with(&report, "UVM-lite");
}

#[test]
fn dm2a_gate_accepts_decomposition_with_operations() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/analysis/decomposition.md"),
        "## Operation: fetch\nbody\n\n## Operation: decode\nbody\n",
    );
    write(&project.join("docs/analysis/data-movement.md"), "data\n");
    clean_critique(&project, "DM2a");
    assert_clean(evaluate(&project, "DM2a"), "DM2a");
}

#[test]
fn dm2a_gate_rejects_decomposition_without_operation_headings() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/analysis/decomposition.md"),
        "Just prose, no operation headings.\n",
    );
    write(&project.join("docs/analysis/data-movement.md"), "data\n");
    clean_critique(&project, "DM2a");
    let report = evaluate(&project, "DM2a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "Operation:");
}

#[test]
fn dm2b_gate_accepts_pipeline_mapping() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/analysis/pipeline-mapping.md"),
        "## Stage 0\nfetch\n\n## Stage 1\ndecode\n",
    );
    clean_critique(&project, "DM2b");
    assert_clean(evaluate(&project, "DM2b"), "DM2b");
}

/// Helper: a minimal test-plan body that satisfies every DM3a
/// gate regex (UVM-lite components, four required sections,
/// `tarpaulin`, checklist row, spec/targets reference).
fn minimal_test_plan() -> &'static str {
    "## Testbench\n\
     Sequencer drives a Driver into the DUT; Monitor and Scoreboard observe.\n\n\
     ## Smoke\n\
     - [ ] elaborates -- covers spec.md section 1\n\n\
     ## Edge\n\
     - [ ] zero_input -- covers targets.md row 2\n\n\
     ## Stress\n\
     - [ ] long_run_1k_cycles -- covers targets.md throughput row\n\n\
     ## Random\n\
     - [ ] sweep_seed_42 -- covers spec.md section 2\n\n\
     ## Coverage\n\
     Run `cargo tarpaulin --out Html`; threshold 90%.\n\n\
     ## Traceability\n\
     spec.md, targets.md.\n"
}

#[test]
fn dm3a_gate_accepts_full_test_plan() {
    let (_tmp, project) = new_project();
    write(&project.join("docs/plan/test-plan.md"), minimal_test_plan());
    clean_critique(&project, "DM3a");
    assert_clean(evaluate(&project, "DM3a"), "DM3a");
}

#[test]
fn dm3a_gate_rejects_plan_without_checklist() {
    let (_tmp, project) = new_project();
    // Has every required section header but no `- [ ]` rows.
    write(
        &project.join("docs/plan/test-plan.md"),
        "## Testbench\nSequencer.\n## Smoke\n## Edge\n## Stress\n## Random\n\
         ## Coverage\ntarpaulin\n## Traceability\nspec.md\n",
    );
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "checklist");
}

#[test]
fn dm3a_gate_rejects_plan_missing_random_category() {
    let (_tmp, project) = new_project();
    // Drops the Random section; the four-categories gate must
    // catch the omission.
    write(
        &project.join("docs/plan/test-plan.md"),
        "## Testbench\nSequencer.\n\
         ## Smoke\n- [ ] s\n## Edge\n- [ ] e\n## Stress\n- [ ] x\n\
         ## Coverage\ntarpaulin\n## Traceability\nspec.md\n",
    );
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "Random");
}

#[test]
fn dm3a_gate_rejects_plan_without_tarpaulin_strategy() {
    let (_tmp, project) = new_project();
    // Has all sections but doesn't name `tarpaulin` in the
    // Coverage section -- the gate enforces the chosen tool.
    write(
        &project.join("docs/plan/test-plan.md"),
        "## Testbench\nSequencer.\n\
         ## Smoke\n- [ ] s\n## Edge\n- [ ] e\n## Stress\n- [ ] x\n## Random\n- [ ] r\n\
         ## Coverage\nWe will pick a coverage tool later.\n\
         ## Traceability\nspec.md\n",
    );
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "tarpaulin");
}

fn stage_experiments_db(project: &Path) {
    use sim_flow::tracking::index::{ExperimentIndex, RunRow};
    let dot = project.join(".sim-flow");
    std::fs::create_dir_all(&dot).unwrap();
    let index = ExperimentIndex::open(&dot).unwrap();
    index
        .insert_run(&RunRow {
            id: 0,
            run_id: "001-fixture".into(),
            timestamp: "t".into(),
            git_commit: "c".into(),
            git_branch: None,
            git_dirty: false,
            config_fingerprint: "fp".into(),
            manifest_path: None,
            workload: Some("fixture".into()),
            candidate: None,
            study: None,
            metrics_summary: None,
            parent_run_id: None,
            sweep_parameter: None,
            sweep_value: None,
            tags: None,
            notes: None,
            lifecycle: "active".into(),
        })
        .unwrap();
}

/// Helper: a minimal perf-plan body that satisfies every DM4a
/// gate regex (numbered milestone, plus at least one
/// `perf-milestone-NN-*.md` file).
fn stage_perf_plan(project: &Path) {
    write(
        &project.join("docs/plan/perf-plan.md"),
        "# Performance Plan\n\nMilestone 01: Baseline measurement.\n",
    );
    write(
        &project.join("docs/plan/perf-milestone-01-baseline.md"),
        "# Milestone 01\n- [ ] run baseline workload\n",
    );
}

#[test]
fn dm4a_gate_accepts_perf_plan_with_milestone() {
    let (_tmp, project) = new_project();
    stage_perf_plan(&project);
    clean_critique(&project, "DM4a");
    assert_clean(evaluate(&project, "DM4a"), "DM4a");
}

#[test]
fn dm4a_gate_rejects_perf_plan_without_milestone_files() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/plan/perf-plan.md"),
        "# Performance Plan\n\nMilestone 01: Baseline measurement.\n",
    );
    clean_critique(&project, "DM4a");
    let report = evaluate(&project, "DM4a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "perf-milestone");
}

#[test]
fn dm4b_gate_accepts_analysis_report_with_tracked_run() {
    let (_tmp, project) = new_project();
    stage_experiments_db(&project);
    stage_perf_plan(&project);
    write(
        &project.join("docs/analysis/summary.md"),
        "# Analysis\nThroughput: 1.0 items/cycle\nLatency p99: 12 cycles\n",
    );
    clean_critique(&project, "DM4b");
    assert_clean(evaluate(&project, "DM4b"), "DM4b");
}

#[test]
fn dm4b_gate_rejects_missing_analysis_report() {
    let (_tmp, project) = new_project();
    stage_experiments_db(&project);
    stage_perf_plan(&project);
    clean_critique(&project, "DM4b");
    let report = evaluate(&project, "DM4b");
    assert!(!report.is_clean());
    assert_fails_with(&report, "report");
}

#[test]
fn dm4b_gate_rejects_missing_experiments_db() {
    let (_tmp, project) = new_project();
    stage_perf_plan(&project);
    write(
        &project.join("docs/analysis/summary.md"),
        "# Analysis\nthroughput/latency content\n",
    );
    clean_critique(&project, "DM4b");
    let report = evaluate(&project, "DM4b");
    assert!(!report.is_clean());
    assert_fails_with(&report, "experiments.db");
}

#[test]
fn reset_cascades_across_dm_order() {
    use sim_flow::state::State;

    let registry = registry_for(Flow::DirectModeling);
    let order: Vec<&'static str> = registry.order_for(Flow::DirectModeling);

    let mut state = State::new(Flow::DirectModeling, "DM0");
    for id in &order {
        state.mark_passed(id, "t");
    }
    assert!(state.is_passed("DM4b"));

    state.reset("DM2b", &order).unwrap();

    assert!(state.is_passed("DM0"));
    assert!(state.is_passed("DM1"));
    assert!(state.is_passed("DM2a"));
    for id in &[
        "DM2b", "DM2c", "DM2d", "DM3a", "DM3b", "DM3c", "DM4a", "DM4b",
    ] {
        assert!(!state.is_passed(id), "{id} should be reset");
    }
    assert_eq!(state.current_step, "DM2b");
}
