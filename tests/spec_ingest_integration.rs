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

/// Milestone 2.16 drift detection: the RV12 fixture's sha256 must
/// match what `tests/fixtures/specs/CHECKSUMS.toml` records. If a
/// developer intentionally refreshes the fixture, update the
/// checksum in that file in the same commit.
#[test]
fn rv12_fixture_matches_recorded_checksum() {
    use sha2::{Digest, Sha256};
    let pdf = fixture("rv12.pdf");
    if !pdf.exists() {
        panic!("RV12 fixture missing at {}", pdf.display());
    }
    let bytes = std::fs::read(&pdf).unwrap();
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = format!("{:x}", h.finalize());

    let checksums_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/specs/CHECKSUMS.toml");
    let body = std::fs::read_to_string(&checksums_path).unwrap();
    // We don't want to depend on toml in tests; a simple scan
    // suffices here.
    let mut expected: Option<&str> = None;
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("sha256 = \"") {
            expected = Some(rest.trim_end_matches('"'));
            break;
        }
    }
    let expected = expected.expect("first sha256 entry in CHECKSUMS.toml");
    assert_eq!(
        got, expected,
        "rv12.pdf checksum drift: recompute and update CHECKSUMS.toml in the same commit if this was intentional"
    );
}

// ---------------------------------------------------------------------
// Milestone 2.17: golden-output snapshot test.
//
// The snapshot is a compact category summary -- per-directory file
// counts -- rather than the full file list. The full list contains
// thousands of chunk paths whose names depend on the PDF text
// extraction; small font-encoding differences ripple through them
// and cause unrelated diffs. The category summary stays stable
// across pdfium-render patch versions while still catching real
// layout changes (a new top-level file, a renamed subdirectory).
// ---------------------------------------------------------------------

#[test]
fn rv12_layout_snapshot_matches() {
    let pdf = fixture("rv12.pdf");
    if !pdf.exists() {
        panic!("RV12 fixture missing at {}", pdf.display());
    }
    let project = fresh_project();
    let _ = ingest(&pdf, project.path());
    let root = project.path().join(".sim-flow/spec-ingest");
    let observed = summarize_layout(&root);

    let snap_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spec-ingest-snapshots/rv12/layout.txt");
    if std::env::var("UPDATE_INGEST_SNAPSHOTS").is_ok() {
        if let Some(parent) = snap_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&snap_path, &observed).unwrap();
        eprintln!("snapshot updated at {}", snap_path.display());
        return;
    }
    if !snap_path.exists() {
        // First run -- bootstrap the snapshot. Avoids forcing devs
        // to flip an env var on first checkout.
        if let Some(parent) = snap_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&snap_path, &observed).unwrap();
        eprintln!(
            "spec-ingest snapshot bootstrapped at {}",
            snap_path.display()
        );
        return;
    }
    let expected = std::fs::read_to_string(&snap_path).unwrap();
    assert_eq!(
        expected, observed,
        "RV12 layout snapshot drift. To accept: UPDATE_INGEST_SNAPSHOTS=1 cargo test"
    );
}

/// Walk the spec-ingest output and return a deterministic summary:
/// for each subdirectory, the number of files it contains and a
/// list of any well-known fixed-name files (manifest.toml,
/// stubs.toml, tbds.toml, references.toml). Avoids per-chunk
/// filenames so we don't snapshot text-extraction details.
fn summarize_layout(root: &Path) -> String {
    let mut out = String::new();
    let mut entries: Vec<PathBuf> = Vec::new();
    walk(root, &mut entries);
    entries.sort();
    // Group by parent directory (relative to root).
    use std::collections::BTreeMap;
    let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &entries {
        let rel = p.strip_prefix(root).unwrap();
        let parent = rel
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = rel
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        by_dir.entry(parent).or_default().push(name);
    }
    for (dir, names) in &by_dir {
        let label = if dir.is_empty() { "<root>" } else { dir };
        let well_known: Vec<&String> = names
            .iter()
            .filter(|n| {
                matches!(
                    n.as_str(),
                    "manifest.toml" | "stubs.toml" | "tbds.toml" | "references.toml"
                )
            })
            .collect();
        out.push_str(&format!("{label}: {} files", names.len()));
        if !well_known.is_empty() {
            let mut sorted: Vec<&String> = well_known.clone();
            sorted.sort();
            out.push_str(" [");
            out.push_str(
                &sorted
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push(']');
        }
        out.push('\n');
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------
// Milestone 2.14: CLI subcommand exit code + manifest verification.
// ---------------------------------------------------------------------

#[test]
fn cli_ingest_subcommand_produces_manifest() {
    let pdf = fixture("rv12.pdf");
    if !pdf.exists() {
        panic!("RV12 fixture missing at {}", pdf.display());
    }
    let project = fresh_project();
    let bin = env!("CARGO_BIN_EXE_sim-flow");
    let out = std::process::Command::new(bin)
        .arg("--project")
        .arg(project.path())
        .arg("ingest")
        .arg("--source")
        .arg(&pdf)
        .arg("--out")
        .arg(project.path())
        .output()
        .expect("spawn sim-flow");
    assert!(
        out.status.success(),
        "sim-flow ingest failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let manifest = project.path().join(".sim-flow/spec-ingest/manifest.toml");
    assert!(
        manifest.exists(),
        "expected manifest at {}",
        manifest.display()
    );
    let body = std::fs::read_to_string(&manifest).unwrap();
    assert!(body.contains("source_kind = \"pdf\""));
    assert!(body.contains("primary_chunk_count"));

    // `--status` should print the same manifest.
    let status_out = std::process::Command::new(bin)
        .arg("--project")
        .arg(project.path())
        .arg("ingest")
        .arg("--status")
        .arg("--out")
        .arg(project.path())
        .output()
        .expect("spawn sim-flow --status");
    assert!(status_out.status.success());
    let stdout = String::from_utf8_lossy(&status_out.stdout);
    assert!(
        stdout.contains("source_kind"),
        "status stdout missing manifest: {stdout}"
    );
}
