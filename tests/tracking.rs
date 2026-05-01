//! End-to-end tracking integration: record runs, extract metrics, create
//! and compare baselines, run a sweep. Mirrors the flow a user would hit
//! via the CLI but drives the library directly so the test stays fast.

use sim_flow::tracking::baseline::{compare, create, list};
use sim_flow::tracking::index::{ExperimentIndex, RunFilter};
use sim_flow::tracking::metrics;
use sim_flow::tracking::run_recording::{RecordRunOptions, record_run};
use sim_flow::tracking::sweep::{SweepDefinition, SweepSection, run as sweep_run};

#[test]
fn full_record_extract_baseline_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    let dot = project.join(".sim-flow");
    std::fs::create_dir_all(&dot).unwrap();
    std::fs::write(dot.join("config.toml"), "[client]\nname=\"mock\"\n").unwrap();

    // Record two runs with different metrics.
    let run_a = record_run(
        &project,
        &dot,
        &RecordRunOptions {
            description: "throughput baseline".into(),
            workload: Some("throughput".into()),
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(
        run_a.artifact_dir.join("metrics.json"),
        r#"{"throughput":0.80,"latency_p99":12}"#,
    )
    .unwrap();
    let index = ExperimentIndex::open(&dot).unwrap();
    metrics::extract_and_store(&index, &run_a.run_id, &run_a.artifact_dir)
        .unwrap()
        .unwrap();

    let run_b = record_run(
        &project,
        &dot,
        &RecordRunOptions {
            description: "throughput improved".into(),
            workload: Some("throughput".into()),
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(
        run_b.artifact_dir.join("metrics.json"),
        r#"{"throughput":0.88,"latency_p99":11}"#,
    )
    .unwrap();
    metrics::extract_and_store(&index, &run_b.run_id, &run_b.artifact_dir)
        .unwrap()
        .unwrap();

    // Create a baseline from the first run.
    let baseline = create(&dot, "v1", Some(&run_a.run_id), Some("initial")).unwrap();
    assert_eq!(baseline.run_id, run_a.run_id);

    // List should show one baseline.
    assert_eq!(list(&dot).unwrap().len(), 1);

    // Compare v1 vs most recent (run_b).
    let delta = compare(&dot, "v1", Some(&run_b.run_id)).unwrap();
    assert_eq!(delta.baseline_run_id, run_a.run_id);
    assert_eq!(delta.current_run_id, run_b.run_id);
    let tp = delta
        .entries
        .iter()
        .find(|e| e.metric == "throughput")
        .unwrap();
    assert!(tp.delta.unwrap() > 0.07);
    let lp = delta
        .entries
        .iter()
        .find(|e| e.metric == "latency_p99")
        .unwrap();
    assert_eq!(lp.delta, Some(-1.0));
}

#[test]
fn sweep_records_parent_and_children_in_index() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    let dot = project.join(".sim-flow");
    std::fs::create_dir_all(&dot).unwrap();

    let def = SweepDefinition {
        sweep: SweepSection {
            name: "buffer-depth".into(),
            parameter: "buffer_depth".into(),
            values: vec![
                toml::Value::Integer(4),
                toml::Value::Integer(8),
                toml::Value::Integer(16),
            ],
            workload: "throughput-stress".into(),
            binary: Some("./no-such-binary".into()),
            extra_args: vec![],
        },
    };
    let results = sweep_run(&project, &dot, &def).unwrap();
    assert_eq!(results.child_run_ids.len(), 3);

    let index = ExperimentIndex::open(&dot).unwrap();
    let children = index
        .list_runs(&RunFilter {
            parent_run_id: Some(results.parent_run_id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(children.len(), 3);
    // parent + 3 children
    assert_eq!(index.count_runs().unwrap(), 4);

    // Verify sweep_value propagated for every child.
    let mut values: Vec<_> = children
        .iter()
        .filter_map(|c| c.sweep_value.clone())
        .collect();
    values.sort();
    assert_eq!(values, vec!["16", "4", "8"]);
}

#[test]
fn record_run_captures_git_dirty_flag_for_non_git_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    let dot = project.join(".sim-flow");
    std::fs::create_dir_all(&dot).unwrap();
    let run = record_run(
        &project,
        &dot,
        &RecordRunOptions {
            description: "smoke".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let index = ExperimentIndex::open(&dot).unwrap();
    let row = index.get_run(&run.run_id).unwrap().unwrap();
    assert_eq!(row.git_commit, "unknown-not-a-git-repo");
    assert!(!row.git_dirty);
}
