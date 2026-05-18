//! Phase 6 milestones 6.12 + 6.13 — end-to-end integration tests
//! for `run_dm0_work`.
//!
//! These stitch the three Phase 6 sub-streams (auto-populate, Q&A
//! loop, gate) through `dm0::run_dm0_work`. The source-driven test
//! builds a synthetic ingest corpus and asserts auto-populate seeds
//! the persisted spec.md plus the gate flags the remaining TBDs
//! (e.g. Gate budget per cycle, which the LLM is expected to add
//! later in the agent's turn). The no-source test scripts every
//! Q&A reply via `MockAgent` and asserts the resulting spec.md
//! parses and persists.

use std::fs;
use std::path::Path;

use sim_flow::session::dm0::{self, Dm0Mode};
use sim_flow::session::{MockAgent, spec_md};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

#[test]
fn source_driven_run_dm0_work_persists_seeded_spec_md() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();

    // Synthetic ingest corpus: manifest + one parameter shard whose
    // comment hints at a clock frequency the assumptions extractor
    // recognises, plus one signal-table shard (one block).
    let corpus = project.join(".sim-flow").join("spec-ingest");
    write(
        &corpus.join("manifest.toml"),
        "schema_version = 1\n\
         source_kind = \"pdf\"\n\
         source_path = \"docs/main.pdf\"\n\
         source_sha256 = \"\"\n",
    );
    write(
        &corpus.join("primary/tables/parameters/000-clocks.toml"),
        "schema_version = 1\n\
         table_kind = \"parameter_table\"\n\
         source_chunk_id = \"abc\"\n\
         source_page_range = [3, 3]\n\
         group = \"clocks\"\n\
         \n\
         [[rows]]\n\
         name = \"core_freq\"\n\
         default = \"2 GHz\"\n\
         comment = \"target core clock at 7nm\"\n",
    );
    write(
        &corpus.join("primary/tables/signals/000-fetch.toml"),
        "schema_version = 1\n\
         table_kind = \"signal_table\"\n\
         source_chunk_id = \"def\"\n\
         source_page_range = [10, 10]\n\
         stage = \"fetch\"\n\
         \n\
         [[rows]]\n\
         name = \"pc\"\n\
         direction = \"out\"\n\
         peer = \"decode_stage\"\n\
         description = \"program counter\"\n",
    );

    // Mode detection sees the manifest as source-driven.
    assert_eq!(dm0::detect_mode(project).unwrap(), Dm0Mode::SourceDriven);

    // Source-driven mode never consults the LLM during run_dm0_work
    // itself — auto_populate is pure file I/O. The mock is empty.
    let mut llm = MockAgent::new();
    let outcome = dm0::run_dm0_work(project, &mut llm).expect("dm0 work runs");

    assert_eq!(outcome.mode, Some(Dm0Mode::SourceDriven));
    assert!(
        outcome.fields_filled > 0,
        "auto-populate should seed at least one row (blocks + parameters + assumptions)"
    );

    // spec.md was persisted and parses cleanly.
    let body = fs::read_to_string(project.join("docs/spec.md")).expect("spec.md exists");
    let spec = spec_md::parse(&body).expect("spec.md parses after auto-populate");

    // The parameter shard's comment fed the assumptions extractor
    // with a Clock frequency row.
    assert!(
        spec.assumptions
            .quantitative
            .iter()
            .any(|r| r.constraint == "Clock frequency" && r.value == "2 GHz"),
        "expected Clock frequency = 2 GHz in quantitative table, got: {:?}",
        spec.assumptions.quantitative,
    );
    // The signal-table shard produced one Block.
    assert!(
        !spec.blocks.is_empty(),
        "expected at least one block from the signal-table shard"
    );

    // Gate runs: structural pieces pass, but auto-populate alone
    // doesn't yet populate Gate-budget — the agent's LLM turn does.
    // The gate flags this as `missing-gate-budget`; the test asserts
    // the gate ran cleanly and surfaced exactly the expected TBDs.
    let outcome_gate =
        dm0::gate::check_dm0_gate(&project.join("docs/spec.md"), None, Some(project))
            .expect("gate runs");
    let codes: Vec<&str> = outcome_gate
        .failures
        .iter()
        .map(|f| f.code.as_str())
        .collect();
    assert!(
        codes.contains(&"missing-gate-budget"),
        "auto-populated draft should still need Gate budget; gate codes: {codes:?}"
    );
    assert!(
        !codes.contains(&"missing-clock-frequency"),
        "Clock frequency was auto-populated; codes: {codes:?}"
    );
}

#[test]
fn no_source_run_dm0_work_persists_qa_resolved_spec_md() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();

    // No manifest on disk — run_dm0_work falls back to no-source
    // mode and drives the Q&A loop via the mock LLM.
    let spec_for_count = spec_md::SpecMd::default();
    let missing = spec_for_count.missing_required_fields();
    assert!(
        !missing.is_empty(),
        "default-empty spec should expose missing required fields"
    );

    // Script one canned answer per MissingField. The qa_loop tests
    // already prove the per-field validation/chaining/turn-cap logic
    // — here we just need every reply to be acceptable on the first
    // turn so the loop closes cleanly.
    let llm = MockAgent::new();
    for field in &missing {
        let canned = match &field.kind {
            spec_md::MissingFieldKind::SectionApplicability => "no",
            spec_md::MissingFieldKind::ConstrainedScalar { regex } if regex.contains("MHz|GHz") => {
                "1 GHz"
            }
            spec_md::MissingFieldKind::ConstrainedScalar { .. } => "42",
            spec_md::MissingFieldKind::TableRow { .. } => "x | x | x | x",
            spec_md::MissingFieldKind::Scalar | spec_md::MissingFieldKind::Prose => "an answer",
        };
        llm.enqueue(canned);
    }
    let mut llm = llm;
    let outcome = dm0::run_dm0_work(project, &mut llm).expect("dm0 work runs");

    assert_eq!(outcome.mode, Some(Dm0Mode::NoSource));
    assert!(
        outcome.fields_filled > 0,
        "Q&A loop should resolve at least one field"
    );

    // spec.md was persisted and parses cleanly.
    let body = fs::read_to_string(project.join("docs/spec.md")).expect("spec.md exists");
    let spec = spec_md::parse(&body).expect("spec.md parses after Q&A");

    // The scripted "1 GHz" reply landed in the Clock frequency row.
    assert!(
        spec.assumptions
            .quantitative
            .iter()
            .any(|r| r.constraint.eq_ignore_ascii_case("Clock frequency") && r.value == "1 GHz"),
        "expected Clock frequency = 1 GHz from scripted Q&A, got: {:?}",
        spec.assumptions.quantitative,
    );

    // Auto-decisions populate the gate's mode check trivially —
    // automated-mode flag isn't set in this synthetic project so the
    // mode check is a no-op.
    let outcome_gate =
        dm0::gate::check_dm0_gate(&project.join("docs/spec.md"), None, None).expect("gate runs");
    // The exact failure profile depends on which sections the QA loop
    // populated; the contract here is that the gate runs without
    // panicking and the spec.md is structurally valid (parses + no
    // hard parse-error).
    let codes: Vec<&str> = outcome_gate
        .failures
        .iter()
        .map(|f| f.code.as_str())
        .collect();
    assert!(
        !codes.contains(&"parse-error"),
        "spec.md should parse cleanly after Q&A; codes: {codes:?}"
    );
    assert!(
        !codes.contains(&"spec-md-missing"),
        "spec.md should exist on disk after Q&A; codes: {codes:?}"
    );
}
