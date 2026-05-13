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
        "# Design Spec\nClock: 2 GHz\nGates per cycle: 50\nTech node: 7 nm\n",
    );
    clean_critique(&project, "DM0");
    assert_clean(evaluate(&project, "DM0"), "DM0");
}

#[test]
fn dm0_gate_rejects_missing_gates_per_cycle() {
    let (_tmp, project) = new_project();
    write(
        &project.join("docs/spec.md"),
        "# Design Spec\nClock: 2 GHz\nTech node: 7 nm\n",
    );
    clean_critique(&project, "DM0");
    let report = evaluate(&project, "DM0");
    assert!(!report.is_clean());
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.description.contains("gates-per-cycle"))
    );
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
        "Sequencer: uniform\nDriver: ingress\nMonitor: egress\nScoreboard: ordering\n\
         \n## Implementation Baseline\n\nBaseline: lib:examples/01-three-stage-pipeline/test/\n",
    );
    clean_critique(&project, "DM1");
    assert_clean(evaluate(&project, "DM1"), "DM1");
}

#[test]
fn dm1_gate_rejects_missing_implementation_baseline() {
    // testbench.md names UVM-lite components (Sequencer, Driver,
    // Monitor, Scoreboard) but never picks a `lib:examples/...`
    // baseline. DM3b inherits the baseline choice from DM1's
    // testbench.md; the check enforces that the choice is made
    // upfront rather than re-derived at DM3b time under a tighter
    // context budget.
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
    let report = evaluate(&project, "DM1");
    assert_fails_with(&report, "lib:examples/<NN-name>/test/ baseline");
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

/// Helper: write a minimal-but-passing test-plan directory at
/// `docs/test-plan/`. Index names UVM-lite components and traces
/// back to spec.md / targets.md; at least one tb-milestone and one
/// test-milestone file exist with `- [ ]` checklist rows;
/// coverage.md names cargo-llvm-cov.
fn write_minimal_test_plan(project: &Path) {
    // DM3a now writes only the OUTLINE: index + coverage strategy
    // + per-milestone STUBS with `<!-- detail-pending -->`
    // placeholders. The `- [ ]` task rows land later under DM3ad's
    // milestone-walk. The DM3a gate accepts stubs (placeholder
    // present); DM3ad's gate is what enforces "every stub
    // detailed".
    write(
        &project.join("docs/test-plan/test-plan.md"),
        "# Test Plan\n\n\
         Sequencer drives a Driver into the DUT; Monitor and Scoreboard observe.\n\n\
         ## Traceability\n\nspec.md, targets.md.\n",
    );
    write(
        &project.join("docs/test-plan/tb-milestone-01-payloads-and-sequencers.md"),
        "# Milestone 01: Payloads and Sequencers\n\n\
         ## Scope\n\nstub.\n\n\
         ## Tasks\n\n<!-- detail-pending -->\n",
    );
    write(
        &project.join("docs/test-plan/test-milestone-01-smoke.md"),
        "# Milestone 01: Smoke\n\n\
         ## Scope\n\nstub.\n\n\
         ## Tasks\n\n<!-- detail-pending -->\n",
    );
    write(
        &project.join("docs/test-plan/coverage.md"),
        "# Coverage\n\nRun `cargo llvm-cov --html`; threshold 90%.\n",
    );
}

#[test]
fn dm3a_gate_accepts_outline_with_stubs() {
    let (_tmp, project) = new_project();
    write_minimal_test_plan(&project);
    clean_critique(&project, "DM3a");
    assert_clean(evaluate(&project, "DM3a"), "DM3a");
}

#[test]
fn dm3a_gate_rejects_plan_missing_test_milestone_files() {
    let (_tmp, project) = new_project();
    // Drops every test-milestone-NN-*.md file. The directory-glob
    // gate must catch that DM3c has no slices to walk.
    write_minimal_test_plan(&project);
    for entry in std::fs::read_dir(project.join("docs/test-plan/")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("test-milestone-") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "test-milestone");
}

#[test]
fn dm3a_gate_rejects_plan_missing_tb_milestone_files() {
    let (_tmp, project) = new_project();
    // Drops every tb-milestone-NN-*.md file. DM3b would have no
    // slices to walk; the directory-glob gate must catch this.
    write_minimal_test_plan(&project);
    for entry in std::fs::read_dir(project.join("docs/test-plan/")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("tb-milestone-") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "tb-milestone");
}

#[test]
fn dm3a_gate_rejects_plan_without_llvm_cov_strategy() {
    let (_tmp, project) = new_project();
    // coverage.md exists but doesn't name `llvm-cov`. The gate
    // enforces the chosen tool.
    write_minimal_test_plan(&project);
    write(
        &project.join("docs/test-plan/coverage.md"),
        "# Coverage\n\nWe will pick a coverage tool later.\n",
    );
    clean_critique(&project, "DM3a");
    let report = evaluate(&project, "DM3a");
    assert!(!report.is_clean());
    assert_fails_with(&report, "llvm-cov");
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
        &project.join("docs/perf-plan/perf-plan.md"),
        "# Performance Plan\n\nMilestone 01: Baseline measurement.\n",
    );
    write(
        &project.join("docs/perf-plan/perf-milestone-01-baseline.md"),
        // DM4a's gate requires the milestone files to exist with at
        // least one task row; DM4b's gate (MilestonesAllResolved)
        // requires every `- [ ]` to be resolved before DM4b can
        // advance. The pending row is what DM4a's checklist gate
        // looks for; DM4b-specific tests below override the body to
        // mark rows complete (`- [x]`) before running DM4b's gate.
        "# Milestone 01\n- [ ] run baseline workload\n",
    );
}

/// Mark every row in every `perf-milestone-NN-*.md` file as
/// Stage a minimal Rust crate so `cargo fmt --check` and
/// `cargo clippy --all-targets` (DM2d / DM3b / DM3c / DM4b
/// gate checks) can run. Empty `src/lib.rs` formats cleanly
/// and produces no clippy diagnostics, so any test that uses
/// this stays gate-clean for the fmt + clippy checks while
/// exercising whatever step-specific gates the test cares about.
fn stage_minimal_crate(project: &Path) {
    write(
        &project.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    );
    // cargo fmt rejects an entirely empty file (it wants the
    // file to end with at least one newline). A single blank line
    // is the minimum that formats cleanly AND triggers no clippy
    // diagnostics.
    write(&project.join("src/lib.rs"), "\n");
}

/// resolved (`- [x]`). DM4b's `MilestonesAllResolved` gate check
/// only passes once every row is `- [x]` or `- [-]`.
fn complete_perf_milestones(project: &Path) {
    let dir = project.join("docs/perf-plan");
    let entries = std::fs::read_dir(&dir).unwrap();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("perf-milestone-") || !name.ends_with(".md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let resolved = body.replace("- [ ]", "- [x]");
        std::fs::write(&path, resolved).unwrap();
    }
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
        &project.join("docs/perf-plan/perf-plan.md"),
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
    stage_minimal_crate(&project);
    stage_experiments_db(&project);
    stage_perf_plan(&project);
    complete_perf_milestones(&project);
    write(
        &project.join("docs/analysis/summary.md"),
        "# Analysis\nThroughput: 1.0 items/cycle\nLatency p99: 12 cycles\n",
    );
    clean_critique(&project, "DM4b");
    assert_clean(evaluate(&project, "DM4b"), "DM4b");
}

#[test]
fn dm4b_gate_rejects_unresolved_milestone_rows() {
    // The new MilestonesAllResolved gate check on DM4b: even when
    // experiments.db has rows, the analysis report exists, and the
    // critique is clean, DM4b cannot advance while any
    // perf-milestone task row is still `- [ ]`.
    let (_tmp, project) = new_project();
    stage_experiments_db(&project);
    stage_perf_plan(&project); // leaves a `- [ ]` row in milestone-01
    write(
        &project.join("docs/analysis/summary.md"),
        "# Analysis\nThroughput: 1.0\nLatency p99: 12 cycles\n",
    );
    clean_critique(&project, "DM4b");
    let report = evaluate(&project, "DM4b");
    assert!(!report.is_clean());
    assert_fails_with(&report, "perf-milestone-01-baseline");
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
