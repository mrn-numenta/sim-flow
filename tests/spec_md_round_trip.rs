//! Integration test: spec.md -> SpecMd -> spec.md round-trip identity.
//!
//! The contract is `parse(write(parse(fixture))) == parse(fixture)`
//! at the typed-struct level for every fixture under
//! `tests/fixtures/spec_md/`. Byte-equal markdown output is NOT
//! required.

use std::path::PathBuf;

use sim_flow::session::spec_md;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("spec_md")
}

fn round_trip(fixture: &str) {
    let path = fixtures_dir().join(fixture);
    let original =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {fixture}: {e}"));
    let first = spec_md::parse(&original).expect("first parse");
    let rendered = first.to_markdown();
    let second = spec_md::parse(&rendered).expect("second parse");
    assert_eq!(
        first, second,
        "round-trip identity failed for {fixture}\nrendered:\n{rendered}"
    );
}

fn round_trip_validates(fixture: &str) {
    let path = fixtures_dir().join(fixture);
    let original =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {fixture}: {e}"));
    let first = spec_md::parse(&original).expect("first parse");
    let rendered = first.to_markdown();
    let second = spec_md::parse(&rendered).expect("second parse");
    let issues = second.validate();
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == spec_md::IssueSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "validation errors after round-trip for {fixture}: {errors:#?}"
    );
}

#[test]
fn round_trip_minimal() {
    round_trip("minimal.md");
    round_trip_validates("minimal.md");
}

#[test]
fn round_trip_rv12_extract() {
    round_trip("rv12-extract.md");
    round_trip_validates("rv12-extract.md");
}

#[test]
fn round_trip_numenta_stubby() {
    round_trip("numenta-stubby.md");
    round_trip_validates("numenta-stubby.md");
}
