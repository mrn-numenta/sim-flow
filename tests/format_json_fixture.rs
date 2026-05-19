//! Integration test: `format.json` fixture loads, round-trips, and
//! re-parses without drift.
//!
//! Covers milestone 9.2's fixture under
//! `tests/fixtures/format_json/rv12-example.json` — a minimal valid
//! descriptor containing one section role + one signal table + one
//! figure + one glossary entry + one chrome regex.

use std::path::PathBuf;

use sim_flow::session::spec_ingest::format::{
    FigureKind, FigureTarget, FontWeight, FormatJson, GlossarySource, Layer, SpecMdRole, TableKind,
    TableTarget, WrapStrategy,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("format_json")
        .join("rv12-example.json")
}

#[test]
fn rv12_example_fixture_loads_with_expected_shape() {
    let path = fixture_path();
    let loaded = FormatJson::load(&path).expect("load fixture");

    assert_eq!(loaded.schema_version, FormatJson::current_schema_version());
    assert_eq!(loaded.model, "claude-sonnet-4-6");
    assert_eq!(loaded.prompt_version, "2026-05-19");

    assert_eq!(loaded.section_roles.len(), 1);
    let role = &loaded.section_roles[0];
    assert_eq!(role.heading, "Instruction Fetch (IF)");
    assert_eq!(role.font_weight, FontWeight::Bold);
    assert_eq!(role.layer, Layer::Micro);
    assert_eq!(
        role.spec_md_role,
        SpecMdRole::Block {
            block_name: "Instruction Fetch (IF)".to_string()
        }
    );

    assert_eq!(loaded.tables.len(), 1);
    let table = &loaded.tables[0];
    assert_eq!(table.id, "tbl_023");
    assert_eq!(table.kind, TableKind::SignalTable);
    assert_eq!(table.wrap_strategy, WrapStrategy::MergeContinuationRows);
    assert_eq!(
        table.spec_md_target,
        TableTarget::BlockSignals {
            block_name: "Instruction Fetch (IF)".to_string()
        }
    );
    assert_eq!(table.column_map.len(), 4);
    assert_eq!(table.column_map[0].canonical, "name");

    assert_eq!(loaded.figures.len(), 1);
    let figure = &loaded.figures[0];
    assert_eq!(figure.kind, FigureKind::BlockDiagram);
    assert_eq!(
        figure.spec_md_target,
        FigureTarget::BlockDiagram {
            block_name: "Instruction Fetch (IF)".to_string()
        }
    );

    assert_eq!(loaded.glossary.len(), 1);
    assert_eq!(loaded.glossary[0].acronym, "IF");
    assert_eq!(
        loaded.glossary[0].source,
        GlossarySource::ParenthesisedFirstMention
    );

    assert_eq!(loaded.chrome.len(), 1);
    assert_eq!(loaded.chrome[0].match_count, 95);

    assert_eq!(loaded.validation.section_roles_assigned, 1);
    assert_eq!(
        loaded.validation.tables_classified.get("signal_table"),
        Some(&1)
    );

    let key = loaded.content_key();
    assert_eq!(key.model, "claude-sonnet-4-6");
    assert_eq!(key.prompt_version, "2026-05-19");
}

#[test]
fn rv12_example_fixture_round_trips_through_disk() {
    let path = fixture_path();
    let original = FormatJson::load(&path).expect("load fixture");

    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("format.json");
    original.write(&out).expect("write");

    let reloaded = FormatJson::load(&out).expect("reload");
    assert_eq!(original, reloaded);
}
