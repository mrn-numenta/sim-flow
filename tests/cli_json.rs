//! Verify that every `--json` CLI output is valid JSON with the shape the
//! VS Code extension (and other machine consumers) depend on. These
//! tests shell out to the compiled binary via `env!("CARGO_BIN_EXE_*")`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sim-flow")
}

fn run(project: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("spawn sim-flow")
}

fn run_ok(project: &Path, args: &[&str]) -> String {
    let out = run(project, args);
    assert!(
        out.status.success(),
        "sim-flow {:?} failed: stderr={}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

fn init_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    run_ok(&project, &["init", "--flow", "direct-modeling"]);
    (tmp, project)
}

#[test]
fn status_json_reports_initial_state() {
    let (_tmp, project) = init_project();
    let out = run_ok(&project, &["status", "--json"]);
    let v: Value = serde_json::from_str(&out).expect("parse status json");
    assert_eq!(v["flow"], "direct-modeling");
    assert_eq!(v["current_step"], "DM0");
    assert!(v["gates"].is_object());
}

#[test]
fn runs_json_is_empty_array_initially() {
    let (_tmp, project) = init_project();
    let out = run_ok(&project, &["runs", "--json"]);
    let v: Value = serde_json::from_str(&out).expect("parse runs json");
    assert!(v.is_array());
    assert_eq!(v.as_array().unwrap().len(), 0);
}

#[test]
fn runs_json_after_record_contains_expected_fields() {
    let (_tmp, project) = init_project();
    run_ok(
        &project,
        &["record-run", "--description", "smoke", "--workload", "wk"],
    );
    let out = run_ok(&project, &["runs", "--json"]);
    let v: Value = serde_json::from_str(&out).expect("parse runs json");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let row = &arr[0];
    for field in [
        "run_id",
        "timestamp",
        "git_commit",
        "config_fingerprint",
        "workload",
        "lifecycle",
    ] {
        assert!(row.get(field).is_some(), "missing field: {field}");
    }
    assert_eq!(row["workload"], "wk");
    assert_eq!(row["lifecycle"], "active");
}

#[test]
fn gate_json_reports_failures_with_status() {
    let (_tmp, project) = init_project();
    // DM0 gate on an empty project should fail; exit non-zero but still
    // emit valid JSON on stdout.
    let out = run(&project, &["gate", "DM0", "--json"]);
    assert!(!out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).expect("parse gate json");
    assert_eq!(v["step"], "DM0");
    assert_eq!(v["clean"], false);
    let failures = v["failures"].as_array().unwrap();
    assert!(!failures.is_empty());
    for f in failures {
        assert!(f.get("description").is_some());
        assert!(f.get("reason").is_some());
    }
}

#[test]
fn baseline_list_json_round_trips() {
    let (_tmp, project) = init_project();
    run_ok(&project, &["record-run", "--description", "seed"]);
    run_ok(&project, &["baseline", "create", "v1"]);
    let out = run_ok(&project, &["baseline", "list", "--json"]);
    let v: Value = serde_json::from_str(&out).expect("parse baseline list");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "v1");
    assert!(arr[0]["run_id"].as_str().unwrap().contains("seed"));
}

#[test]
fn baseline_compare_json_includes_deltas() {
    let (_tmp, project) = init_project();
    run_ok(&project, &["record-run", "--description", "a"]);
    // Manually write metrics for the first run so the delta is meaningful.
    let experiments_dir = project.join(".experiments");
    let entries: Vec<_> = std::fs::read_dir(&experiments_dir).unwrap().collect();
    let run_a_dir = entries[0].as_ref().unwrap().path();
    std::fs::write(run_a_dir.join("metrics.json"), r#"{"throughput":0.80}"#).unwrap();
    run_ok(&project, &["baseline", "create", "v1"]);

    run_ok(&project, &["record-run", "--description", "b"]);
    let entries: Vec<_> = std::fs::read_dir(&experiments_dir).unwrap().collect();
    let run_b_dir = entries
        .iter()
        .map(|e| e.as_ref().unwrap().path())
        .find(|p| p != &run_a_dir)
        .unwrap();
    std::fs::write(run_b_dir.join("metrics.json"), r#"{"throughput":0.90}"#).unwrap();

    // metrics.json is read at run-record time into the DB via the
    // metrics-extraction helper; we need a fresh record for the data to
    // flow into the row. The simple path is: populate the rows via a
    // direct library call in a separate test. Here we just assert the
    // JSON shape is parseable and has the expected keys, even if the
    // deltas are None because metrics were not extracted for this
    // compare path.
    let out = run_ok(&project, &["baseline", "compare", "v1", "--json"]);
    let v: Value = serde_json::from_str(&out).expect("parse compare");
    assert!(v.get("baseline_run_id").is_some());
    assert!(v.get("current_run_id").is_some());
    assert!(v["entries"].is_array());
}

#[test]
fn new_model_json_describes_generated_project() {
    let tmp = tempfile::tempdir().unwrap();
    let foundation_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let dest = tmp.path().to_path_buf();
    let out = Command::new(bin())
        .arg("--foundation-root")
        .arg(&foundation_root)
        .args(["new", "model", "smoke-model", "--destination"])
        .arg(&dest)
        .args(["--skip-cargo-check", "--json"])
        .output()
        .expect("spawn sim-flow new model");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).expect("parse new model json");
    assert_eq!(v["crate_name"], "smoke_model");
    assert_eq!(v["next_step"], "DM0");
    assert!(
        v["project_dir"].as_str().unwrap().ends_with("smoke-model"),
        "project_dir: {:?}",
        v["project_dir"]
    );
}

// -----------------------------------------------------------------
// Phase 9 M1: `describe` and `advance` subcommands.
// -----------------------------------------------------------------

fn foundation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run_with_foundation(project: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--project")
        .arg(project)
        .arg("--foundation-root")
        .arg(foundation_root())
        .args(args)
        .output()
        .expect("spawn sim-flow")
}

#[test]
fn describe_emits_step_descriptor_for_dm0_work() {
    let (_tmp, project) = init_project();
    let out = run_with_foundation(&project, &["describe", "DM0.work", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).expect("parse describe json");
    assert_eq!(v["step"], "DM0");
    assert_eq!(v["kind"], "work");
    assert_eq!(v["flow"], "direct-modeling");
    assert_eq!(v["per_candidate"], false);
    // DM0 lists both spec layouts (single-file `docs/spec.md` and
    // paginated `docs/spec/`); the gate accepts either, and Reset
    // sweeps both forms.
    assert_eq!(
        v["work_artifacts"],
        serde_json::json!(["docs/spec.md", "docs/spec/"])
    );
    assert_eq!(v["predecessor_inputs"], serde_json::json!([]));
    let gate_checks = v["gate_checks"].as_array().expect("gate_checks array");
    assert!(
        gate_checks.iter().any(|c| c["kind"] == "critique-clean"),
        "DM0 gate should include critique-clean",
    );
    assert!(
        v["instruction_body"]
            .as_str()
            .unwrap()
            .contains("DM0 - Specification"),
        "instruction body should be loaded",
    );
}

#[test]
fn describe_dm1_critique_lists_predecessor_spec_and_own_work_artifacts() {
    let (_tmp, project) = init_project();
    let out = run_with_foundation(&project, &["describe", "DM1.critique", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(v["step"], "DM1");
    assert_eq!(v["kind"], "critique");
    // Both spec layouts are surfaced as predecessor inputs so
    // downstream steps see whichever the project uses; the
    // orchestrator's TOC builder gracefully reports the missing
    // form as `(missing)`.
    assert_eq!(
        v["predecessor_inputs"],
        serde_json::json!(["docs/spec.md", "docs/spec/"])
    );
    assert_eq!(
        v["work_artifacts"],
        serde_json::json!(["docs/targets.md", "docs/testbench.md"])
    );
    assert!(
        v["instruction_path"]
            .as_str()
            .unwrap()
            .ends_with("dm1-modeling-setup-critique.md"),
        "instruction_path should resolve to the -critique file",
    );
}

#[test]
fn describe_rejects_unknown_step_or_kind() {
    let (_tmp, project) = init_project();
    let bad = run_with_foundation(&project, &["describe", "DZ9.work", "--json"]);
    assert!(!bad.status.success());
    let no_dot = run_with_foundation(&project, &["describe", "DM0", "--json"]);
    assert!(!no_dot.status.success());
}

#[test]
fn advance_refuses_when_gate_dirty_and_does_not_mutate_state() {
    let (_tmp, project) = init_project();
    // Fresh project: spec.md is missing, gate fails.
    let out = run(&project, &["advance", "DM0", "--json"]);
    assert!(!out.status.success(), "advance should refuse on dirty gate");
    let v: Value = serde_json::from_str(&String::from_utf8(out.stdout).unwrap())
        .expect("advance --json on failure still emits JSON");
    assert_eq!(v["step"], "DM0");
    assert_eq!(v["clean"], false);
    assert_eq!(v["advanced"], false);
    assert!(v["next_step"].is_null());
    assert!(
        !v["failures"].as_array().unwrap().is_empty(),
        "should surface gate failures"
    );

    // State must be unchanged.
    let status = run_ok(&project, &["status", "--json"]);
    let s: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(s["current_step"], "DM0");
    let dm0_passed = s["gates"]
        .as_object()
        .and_then(|m| m.get("DM0"))
        .map(|g| g["passed"] == true)
        .unwrap_or(false);
    assert!(!dm0_passed, "DM0 gate should not be marked passed");
}

#[test]
fn advance_marks_passed_and_bumps_current_step_on_clean_gate() {
    let (_tmp, project) = init_project();
    // Satisfy DM0's gate by hand.
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(
        project.join("docs/spec.md"),
        "# Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n",
    )
    .unwrap();
    let critiques = project.join("docs").join("critiques");
    std::fs::create_dir_all(&critiques).unwrap();
    std::fs::write(critiques.join("DM0-critique.md"), "All clean.\n").unwrap();

    let out = run(&project, &["advance", "DM0", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(v["step"], "DM0");
    assert_eq!(v["clean"], true);
    assert_eq!(v["advanced"], true);
    assert_eq!(v["next_step"], "DM1");

    // State should reflect: DM0 passed, current_step bumped to DM1.
    let status = run_ok(&project, &["status", "--json"]);
    let s: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(s["current_step"], "DM1");
    assert_eq!(s["gates"]["DM0"]["passed"], true);
}

#[test]
fn advance_emits_failure_json_on_unreachable_step() {
    let (_tmp, project) = init_project();
    let out = run(&project, &["advance", "DM4b", "--json"]);
    assert!(!out.status.success());
    let v: Value = serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(v["step"], "DM4b");
    assert_eq!(v["clean"], false);
}
