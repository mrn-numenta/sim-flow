//! Per-check evaluation tests. Each test builds a synthetic project
//! directory with `tempfile::tempdir()`, runs the gate against one or
//! more `GateCheck` values, and asserts on the resulting
//! `GateReport`. Helpers (`marker`, `expand_candidate_files`) are
//! pulled in from the sibling `evaluators` module.

use super::evaluators::{expand_candidate_files, marker};
use super::*;
use tempfile::tempdir;

#[test]
fn file_exists_fails_when_missing() {
    let dir = tempdir().unwrap();
    let report = evaluate(
        dir.path(),
        &[GateCheck::FileExists {
            path: PathBuf::from("spec.md"),
            description: "spec.md exists".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn file_exists_passes_when_present() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("spec.md"), "hello").unwrap();
    let report = evaluate(
        dir.path(),
        &[GateCheck::FileExists {
            path: PathBuf::from("spec.md"),
            description: "spec.md exists".into(),
        }],
    )
    .unwrap();
    assert!(report.is_clean());
}

#[test]
fn file_matches_fails_when_pattern_absent() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("spec.md"), "no frequency here").unwrap();
    let report = evaluate(
        dir.path(),
        &[GateCheck::FileMatches {
            path: PathBuf::from("spec.md"),
            pattern: r"\d+\s*(MHz|GHz)".into(),
            description: "spec has frequency".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn critique_clean_fails_on_gate_failing_findings() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("crit.md"),
        "- UNRESOLVED: coverage gap remains\n- BLOCKER: missing test for X\n",
    )
    .unwrap();
    let report = evaluate(
        dir.path(),
        &[GateCheck::CritiqueClean {
            path: PathBuf::from("crit.md"),
            description: "critique clean".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    let reason = &report.failures[0].reason;
    assert!(reason.contains("UNRESOLVED"));
    assert!(reason.contains("BLOCKER"));
}

fn write_shard_json(dir: &Path, shard: &str, body: &str) {
    let p = dir.join(format!("{shard}.json"));
    std::fs::write(p, body).unwrap();
}

#[test]
fn critique_clean_dir_mode_passes_when_all_shards_clean() {
    let tmp = tempdir().unwrap();
    let shard_dir = tmp.path().join("docs/critiques/DM2cd");
    std::fs::create_dir_all(&shard_dir).unwrap();
    write_shard_json(
        &shard_dir,
        "milestone-01",
        r#"{"step":"DM2cd","summary":"ok","findings":[],"notes":""}"#,
    );
    write_shard_json(
        &shard_dir,
        "milestone-02",
        r#"{"step":"DM2cd","summary":"ok","findings":[],"notes":""}"#,
    );
    let report = evaluate(
        tmp.path(),
        &[GateCheck::CritiqueClean {
            path: PathBuf::from("docs/critiques/DM2cd"),
            description: "DM2cd shards clean".into(),
        }],
    )
    .unwrap();
    assert!(report.is_clean(), "got failures: {:?}", report.failures);
}

#[test]
fn critique_clean_dir_mode_collects_blockers_across_all_shards() {
    // Three shards: A and C blocking, B clean. The dir-mode gate
    // must aggregate ALL of A's and C's blockers into a single
    // failure reason -- not exit early on A.
    let tmp = tempdir().unwrap();
    let shard_dir = tmp.path().join("docs/critiques/DM2cd");
    std::fs::create_dir_all(&shard_dir).unwrap();
    write_shard_json(
        &shard_dir,
        "milestone-01",
        r#"{"step":"DM2cd","summary":"","findings":[
            {"kind":"blocker","section":"Tasks","title":"missing trace","body":"task 3"}
        ],"notes":""}"#,
    );
    write_shard_json(
        &shard_dir,
        "milestone-02",
        r#"{"step":"DM2cd","summary":"","findings":[],"notes":""}"#,
    );
    write_shard_json(
        &shard_dir,
        "milestone-03",
        r#"{"step":"DM2cd","summary":"","findings":[
            {"kind":"unresolved","section":"Trace","title":"vague trace","body":""},
            {"kind":"blocker","section":"Scope","title":"scope too narrow","body":""}
        ],"notes":""}"#,
    );
    let report = evaluate(
        tmp.path(),
        &[GateCheck::CritiqueClean {
            path: PathBuf::from("docs/critiques/DM2cd"),
            description: "DM2cd shards clean".into(),
        }],
    )
    .unwrap();
    assert_eq!(
        report.failures.len(),
        1,
        "expected a single aggregated failure"
    );
    let reason = &report.failures[0].reason;
    assert!(reason.contains("3 blocking finding(s) across 3 shard(s)"));
    assert!(reason.contains("[milestone-01]"));
    assert!(reason.contains("[milestone-03]"));
    assert!(reason.contains("missing trace"));
    assert!(reason.contains("scope too narrow"));
    assert!(reason.contains("vague trace"));
    // milestone-02 has no findings; should NOT appear in the listing.
    assert!(!reason.contains("[milestone-02]"));
}

#[test]
fn critique_clean_dir_mode_empty_dir_fails() {
    let tmp = tempdir().unwrap();
    let shard_dir = tmp.path().join("docs/critiques/DM2cd");
    std::fs::create_dir_all(&shard_dir).unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::CritiqueClean {
            path: PathBuf::from("docs/critiques/DM2cd"),
            description: "DM2cd shards clean".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0]
            .reason
            .contains("critique directory empty")
    );
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

#[test]
fn milestones_all_resolved_passes_when_every_row_is_checked() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/test-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(
        &dir.join("tb-milestone-01.md"),
        "- [x] done\n- [-] deferred\n  - defer reason: skipped\n",
    );
    write(&dir.join("tb-milestone-02.md"), "- [x] done\n");
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/test-plan/"),
            file_prefixes: vec!["tb-milestone-".into()],
            placeholder_marker: None,
            description: "every tb-milestone resolved".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert!(report.is_clean(), "got failures: {:?}", report.failures);
}

#[test]
fn milestones_all_resolved_fails_when_any_pending_row_remains() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/test-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir.join("tb-milestone-01.md"), "- [x] done\n");
    write(
        &dir.join("tb-milestone-02.md"),
        "- [ ] still pending\n- [x] done\n",
    );
    write(&dir.join("tb-milestone-03.md"), "- [ ] also pending\n");
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/test-plan/"),
            file_prefixes: vec!["tb-milestone-".into()],
            placeholder_marker: None,
            description: "every tb-milestone resolved".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    let reason = &report.failures[0].reason;
    assert!(reason.contains("tb-milestone-02"), "reason: {reason}");
    assert!(reason.contains("tb-milestone-03"), "reason: {reason}");
    assert!(!reason.contains("tb-milestone-01"));
}

#[test]
fn milestones_all_resolved_fails_when_no_milestone_files_exist() {
    // Configuration error case: the planning step (DM3a) didn't
    // produce any milestone files, but the dir exists.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs/test-plan")).unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/test-plan/"),
            file_prefixes: vec!["tb-milestone-".into()],
            placeholder_marker: None,
            description: "every tb-milestone resolved".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].reason.contains("no `tb-milestone-NN-"));
}

#[test]
fn milestones_all_resolved_isolates_to_one_file_prefix() {
    // Same dir holds DM3b's tb-milestone-* AND DM3c's
    // test-milestone-* files. The check must only inspect the
    // files matching its own prefix, so DM3b's gate doesn't
    // fail because DM3c hasn't started yet (or vice versa).
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/test-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir.join("tb-milestone-01.md"), "- [x] done\n");
    write(&dir.join("test-milestone-01.md"), "- [ ] DM3c pending\n");
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/test-plan/"),
            file_prefixes: vec!["tb-milestone-".into()],
            placeholder_marker: None,
            description: "tb only".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert!(
        report.is_clean(),
        "DM3b's gate should NOT see DM3c's pending rows: {:?}",
        report.failures
    );
}

#[test]
fn milestones_all_resolved_placeholder_mode_passes_when_no_marker_left() {
    // Detail-step gate: every stub has had its placeholder
    // marker removed (the agent wrote real task lists). Real
    // `- [ ]` rows in those task lists are FOR the downstream
    // execution step; the planning-detail gate must ignore
    // them.
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/impl-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(
        &dir.join("milestone-01-payloads.md"),
        "# Milestone 01\n\n## Tasks\n- [ ] real task here\n",
    );
    write(
        &dir.join("milestone-02-skeletons.md"),
        "# Milestone 02\n\n## Tasks\n- [ ] another task\n- [ ] and another\n",
    );
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/impl-plan/"),
            file_prefixes: vec!["milestone-".into()],
            placeholder_marker: Some("<!-- detail-pending".into()),
            description: "every stub detailed".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert!(
        report.is_clean(),
        "placeholder-mode gate should ignore `- [ ]` rows: {:?}",
        report.failures
    );
}

#[test]
fn milestones_all_resolved_placeholder_mode_fails_when_any_stub_still_has_marker() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/impl-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(
        &dir.join("milestone-01-payloads.md"),
        "# Milestone 01\n\n## Tasks\n- [ ] real task\n",
    );
    write(
        &dir.join("milestone-02-skeletons.md"),
        "# Milestone 02\n\n## Tasks\n<!-- detail-pending\n",
    );
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/impl-plan/"),
            file_prefixes: vec!["milestone-".into()],
            placeholder_marker: Some("<!-- detail-pending".into()),
            description: "every stub detailed".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    let reason = &report.failures[0].reason;
    assert!(reason.contains("milestone-02"), "reason: {reason}");
    assert!(reason.contains("placeholder"), "reason: {reason}");
    assert!(!reason.contains("milestone-01"));
}

#[test]
fn milestones_all_resolved_walks_multiple_prefixes_in_one_check() {
    // DM3ad walks BOTH tb-milestone-* and test-milestone-*
    // files in `docs/test-plan/` -- one gate, two prefixes.
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("docs/test-plan");
    std::fs::create_dir_all(&dir).unwrap();
    write(
        &dir.join("tb-milestone-01-payloads.md"),
        "# tb-Milestone 01\n## Tasks\n- [ ] task\n",
    );
    write(
        &dir.join("test-milestone-01-smoke.md"),
        "# test-Milestone 01\n## Tasks\n<!-- detail-pending\n",
    );
    let report = evaluate(
        tmp.path(),
        &[GateCheck::MilestonesAllResolved {
            dir: PathBuf::from("docs/test-plan/"),
            file_prefixes: vec!["tb-milestone-".into(), "test-milestone-".into()],
            placeholder_marker: Some("<!-- detail-pending".into()),
            description: "all detailed".into(),
            forbid_deferred: false,
        }],
    )
    .unwrap();
    // Only test-milestone-01 has the marker -- the gate should
    // fail on it but pass tb-milestone-01.
    assert_eq!(report.failures.len(), 1);
    let reason = &report.failures[0].reason;
    assert!(reason.contains("test-milestone-01"), "reason: {reason}");
    assert!(!reason.contains("tb-milestone-01"));
}

#[test]
fn any_exists_passes_for_single_file_layout() {
    // Legacy / small-spec layout: docs/spec.md is the spec.
    // The directory candidate is missing; the gate still
    // passes because the file candidate exists with content.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
    std::fs::write(tmp.path().join("docs/spec.md"), "hi").unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyExists {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            description: "spec exists".into(),
        }],
    )
    .unwrap();
    assert!(report.is_clean(), "failures: {:?}", report.failures);
}

#[test]
fn any_exists_passes_for_paginated_layout() {
    // New paginated layout: docs/spec.md absent, sections live
    // under docs/spec/. Gate passes via the directory branch.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
    std::fs::write(tmp.path().join("docs/spec/01-overview.md"), "x").unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyExists {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            description: "spec exists".into(),
        }],
    )
    .unwrap();
    assert!(report.is_clean(), "failures: {:?}", report.failures);
}

#[test]
fn any_exists_fails_when_neither_form_present() {
    let tmp = tempdir().unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyExists {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            description: "spec exists".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn any_exists_skips_empty_files() {
    // Empty file fails the "non-empty" requirement; gate falls
    // through to the directory candidate, which is also empty
    // -- so the overall gate fails.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
    std::fs::write(tmp.path().join("docs/spec.md"), "").unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyExists {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            description: "spec non-empty".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn any_matches_finds_pattern_in_paginated_section() {
    // Clock frequency lives in section 04, not in any
    // top-level docs/spec.md. The gate must scan section
    // files to satisfy the regex.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
    std::fs::write(
        tmp.path().join("docs/spec/01-overview.md"),
        "# Overview\nA pipeline.\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("docs/spec/04-timing.md"),
        "# Timing\nClock frequency: 1 GHz.\n",
    )
    .unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyMatches {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            pattern: r"\d+\s*(MHz|GHz)".into(),
            description: "spec has frequency".into(),
        }],
    )
    .unwrap();
    assert!(report.is_clean(), "failures: {:?}", report.failures);
}

#[test]
fn any_matches_skips_index_files_when_scanning_directory() {
    // The auto-generated `README.md` index typically just
    // links to section files and shouldn't be a substitute
    // for the section content; the gate's expansion rule
    // excludes it. Pattern lives only in README.md here, so
    // the gate must fail.
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs/spec")).unwrap();
    std::fs::write(
        tmp.path().join("docs/spec/README.md"),
        "# Spec\nClock: 1 GHz\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("docs/spec/01-overview.md"),
        "# Overview\nNo numbers here.\n",
    )
    .unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyMatches {
            paths: vec![PathBuf::from("docs/spec/")],
            pattern: r"\d+\s*GHz".into(),
            description: "spec has frequency".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn any_matches_fails_with_helpful_message_when_no_candidates() {
    let tmp = tempdir().unwrap();
    let report = evaluate(
        tmp.path(),
        &[GateCheck::AnyMatches {
            paths: vec![PathBuf::from("docs/spec.md"), PathBuf::from("docs/spec/")],
            pattern: r"\d+\s*GHz".into(),
            description: "spec has frequency".into(),
        }],
    )
    .unwrap();
    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].reason.contains("no candidate files"));
}

#[test]
fn marker_maps_each_finding_variant() {
    use crate::critique::Finding;
    assert_eq!(marker(&Finding::Resolved("x".into())), "RESOLVED");
    assert_eq!(marker(&Finding::Unresolved("x".into())), "UNRESOLVED");
    assert_eq!(marker(&Finding::Blocker("x".into())), "BLOCKER");
}

#[test]
fn expand_candidate_files_walks_dirs_and_skips_index_files() {
    let tmp = tempdir().unwrap();
    // Build a spec directory tree.
    let dir = tmp.path().join("docs/spec");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("section-01.md"), "body").unwrap();
    std::fs::write(dir.join("section-02.md"), "body").unwrap();
    // Index files that must be skipped.
    std::fs::write(dir.join("README.md"), "summary").unwrap();
    std::fs::write(dir.join("_toc.md"), "toc").unwrap();
    std::fs::write(dir.join("index.md"), "idx").unwrap();
    std::fs::write(dir.join(".gitkeep"), "").unwrap();
    // A direct file that should be included as-is.
    std::fs::write(tmp.path().join("docs/extra.md"), "extra").unwrap();
    let got = expand_candidate_files(
        tmp.path(),
        &[
            PathBuf::from("docs/spec"),
            PathBuf::from("docs/extra.md"),
            PathBuf::from("docs/missing.md"),
        ],
    );
    let names: Vec<String> = got
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    assert!(names.contains(&"section-01.md".to_string()));
    assert!(names.contains(&"section-02.md".to_string()));
    assert!(names.contains(&"extra.md".to_string()));
    assert!(!names.contains(&"README.md".to_string()));
    assert!(!names.contains(&"_toc.md".to_string()));
    assert!(!names.contains(&"index.md".to_string()));
    // Case-insensitive skip on the README spelling.
    std::fs::write(dir.join("Readme.md"), "x").unwrap();
    let got2 = expand_candidate_files(tmp.path(), &[PathBuf::from("docs/spec")]);
    let names2: Vec<String> = got2
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    assert!(!names2.iter().any(|n| n.eq_ignore_ascii_case("readme.md")));
}
