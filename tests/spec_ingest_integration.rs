//! End-to-end smoke tests for the spec-ingest pipeline against the
//! four canonical sample specs (RV12, Apical NoC, Numenta SoC,
//! Spatial Pooler). Only the RV12 fixture is checked in; the other
//! three are `#[ignore]`-gated and skip when their PDFs aren't on
//! disk. See `tools/sim-flow/docs/plan/02-phase-spec-ingest-pipeline.md`
//! milestone 2.16 for the contract.

use std::path::{Path, PathBuf};

use sim_flow::session::spec_ingest::{
    IngestConfig, IngestRequest, SourceSpec, pipeline::run as run_pipeline,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("specs")
        .join(name)
}

fn fresh_project() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ingest(pdf: &Path, project: &Path) -> sim_flow::session::spec_ingest::IngestOutcome {
    let request = IngestRequest {
        primary: Some(SourceSpec::new(pdf)),
        peers: Vec::new(),
        config: IngestConfig::default(),
        project_root: project.to_path_buf(),
    };
    run_pipeline(request).expect("ingest succeeds")
}

// ---------------------------------------------------------------------
// Milestone 2.11: RV12 figure-render verification.
// ---------------------------------------------------------------------

#[test]
fn rv12_ingest_renders_page_013_figure() {
    let pdf = fixture("rv12.pdf");
    if !pdf.exists() {
        panic!(
            "RV12 fixture missing at {}; copy it from sim-models/users/mneilly/rv12/docs/",
            pdf.display()
        );
    }
    let project = fresh_project();
    let outcome = ingest(&pdf, project.path());

    // Manifest should record a primary figure for the IF block
    // diagram on page 13.
    assert!(outcome.primary_figure_count >= 1, "no figures rendered");

    let figures_dir = project.path().join(".sim-flow/spec-ingest/primary/figures");
    let png = figures_dir.join("page-013.png");
    assert!(
        png.exists(),
        "expected RV12 page-013 figure at {}",
        png.display()
    );
    let bytes = std::fs::read(&png).unwrap();
    assert!(
        bytes.len() > 50 * 1024,
        "RV12 page-013 PNG is too small ({} bytes); expected > 50 KB",
        bytes.len()
    );

    // Validate the PNG header and dimensions. The image crate gives
    // us width / height without decoding the full pixel buffer.
    let img = image::ImageReader::open(&png)
        .expect("open png")
        .with_guessed_format()
        .expect("guess format")
        .decode()
        .expect("decode png");
    let (w, h) = (img.width(), img.height());
    assert!(
        w >= 800 && h >= 800,
        "RV12 page-013 PNG dimensions too small: {w}x{h}; expected >= 800x800"
    );
}

// ---------------------------------------------------------------------
// Milestone 2.16: integration test against four sample specs.
// ---------------------------------------------------------------------

#[test]
fn rv12_full_ingest_meets_acceptance_floors() {
    let pdf = fixture("rv12.pdf");
    if !pdf.exists() {
        panic!("RV12 fixture missing at {}", pdf.display());
    }
    let project = fresh_project();
    let outcome = ingest(&pdf, project.path());

    // Manifest exists and is structured.
    assert!(outcome.manifest_path.exists());
    let manifest = std::fs::read_to_string(&outcome.manifest_path).unwrap();
    assert!(manifest.contains("source_kind = \"pdf\""));
    assert!(manifest.contains("primary_chunk_count"));

    // Acceptance floors: at least 1 chunk and 1 figure for RV12.
    assert!(outcome.primary_chunk_count >= 1);
    assert!(outcome.primary_figure_count >= 1);

    // Output directory layout.
    let root = project.path().join(".sim-flow/spec-ingest/primary");
    assert!(root.join("chunks").is_dir());
    assert!(root.join("figures").is_dir());
}

// The remaining three sample specs are present in user trees but not
// checked into the repo. The tests below run when the corresponding
// PDF exists at tests/fixtures/specs/, and are otherwise ignored so
// the test suite still completes on a fresh clone.

#[test]
fn apical_noc_full_ingest() {
    let pdf = fixture("apical-noc.pdf");
    if !pdf.exists() {
        eprintln!(
            "skipping apical-noc: fixture not present at {}",
            pdf.display()
        );
        return;
    }
    let project = fresh_project();
    let outcome = ingest(&pdf, project.path());
    assert!(outcome.primary_chunk_count >= 1);
}

#[test]
fn numenta_soc_full_ingest_records_known_stubs() {
    let pdf = fixture("numenta-soc.pdf");
    if !pdf.exists() {
        eprintln!(
            "skipping numenta-soc: fixture not present at {}",
            pdf.display()
        );
        return;
    }
    let project = fresh_project();
    let outcome = ingest(&pdf, project.path());
    assert!(outcome.primary_chunk_count >= 1);
    // Numenta SoC is the canonical stub-heavy spec.
    let stubs_path = project
        .path()
        .join(".sim-flow/spec-ingest/primary/stubs.toml");
    let stubs = std::fs::read_to_string(stubs_path).unwrap();
    let known_sections = ["HTM", "CPU System", "NoC", "Boot"];
    let mut hits = 0;
    for k in known_sections {
        if stubs.contains(k) {
            hits += 1;
        }
    }
    assert!(
        hits >= 2,
        "expected at least 2 of the canonical Numenta SoC stubs in stubs.toml; got {hits}"
    );
}

#[test]
fn spatial_pooler_full_ingest() {
    let pdf = fixture("spatial-pooler.pdf");
    if !pdf.exists() {
        eprintln!(
            "skipping spatial-pooler: fixture not present at {}",
            pdf.display()
        );
        return;
    }
    let project = fresh_project();
    let outcome = ingest(&pdf, project.path());
    assert!(outcome.primary_chunk_count >= 1);
}
