//! Integration tests for the Phase 4 lance-index CLI subcommands
//! and the underlying build / query pipelines (Chapter 3).
//!
//! Two layers exercised:
//!
//! 1. The compiled `sim-flow` binary: `build-framework-index`,
//!    `build-spec-index`, `refresh-spec` with `--check`.
//! 2. The library-level builder + query API against synthetic
//!    fixtures, using a deterministic SHA-256-derived mock embedder
//!    (no live provider).
//!
//! The mock-embedder path is what the unit tests cover; this file
//! adds the cross-build cases (e.g. spec index referencing a
//! spec-ingest manifest written by Phase 2's pipeline).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use sim_flow::__internal::session::embedder::{EmbedError, EmbeddingClient};
use sim_flow::__internal::session::lance_index::build::{
    FrameworkBuildOpts, SpecBuildOpts, build_framework_index, build_spec_index,
};
use sim_flow::__internal::session::lance_index::staleness::{
    SpecIndexStaleness, is_spec_index_stale,
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sim-flow")
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("spawn sim-flow")
}

/// Deterministic mock embedder mirroring the one in
/// `__internal::session::lance_index::build::framework::tests`.
struct MockEmbedder {
    dimension: usize,
}

#[async_trait]
impl EmbeddingClient for MockEmbedder {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock-embed"
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            let digest = hasher.finalize();
            let mut vec = Vec::with_capacity(self.dimension);
            for i in 0..self.dimension {
                let b = digest[i % digest.len()];
                vec.push((b as f32) / 255.0);
            }
            out.push(vec);
        }
        Ok(out)
    }
}

/// Lay down a synthetic framework fixture: one api-page and one
/// `src/lib.rs` carrying three top-level items.
fn make_synthetic_framework(root: &Path) {
    std::fs::create_dir_all(root.join("api").join("pages")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("api").join("pages").join("intro.md"),
        "# Intro\nFramework introduction.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src").join("lib.rs"),
        "pub fn add(a: u32, b: u32) -> u32 { a + b }\npub struct Adder;\npub trait Compute { fn run(&self) -> u32; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"synthetic\"\nversion = \"9.9.9\"\n",
    )
    .unwrap();
}

/// Lay down a synthetic spec-ingest corpus: one primary chunk, one
/// signals TOML, one references TOML.
fn make_synthetic_project(project: &Path) {
    let ingest = project.join(".sim-flow").join("spec-ingest");
    std::fs::create_dir_all(ingest.join("primary").join("chunks")).unwrap();
    std::fs::create_dir_all(ingest.join("primary").join("tables").join("signals")).unwrap();
    std::fs::create_dir_all(&ingest).unwrap();

    // Manifest with a known source_sha256 so staleness checks work.
    std::fs::write(
        ingest.join("manifest.toml"),
        "schema_version = 1\nsource_kind = \"markdown\"\nsource_sha256 = \"abcdef0123456789\"\n",
    )
    .unwrap();

    std::fs::write(
        ingest.join("primary").join("chunks").join("000-intro.md"),
        "---\nchunk_id: \"chunk-intro\"\nbreadcrumb:\n- \"Introduction\"\nsection_heading: \"Introduction\"\nkind: prose\nsource_page_start: 1\nsource_page_end: 2\n---\nIntroduction body.\n",
    )
    .unwrap();

    std::fs::write(
        ingest
            .join("primary")
            .join("tables")
            .join("signals")
            .join("if.toml"),
        r#"
source_id = "primary"
chunk_id = "chunk-intro"
breadcrumb = ["IF"]

[[rows]]
signal_name = "pc"
direction = "out"
peer = "IF"
description = "program counter"

[[rows]]
signal_name = "ir"
direction = "in"
peer = "IF"
description = "instruction register"
"#,
    )
    .unwrap();

    std::fs::write(
        ingest.join("references.toml"),
        r#"
[[references]]
source_chunk_id = "chunk-intro"
peer_id = "TM"
reference_text = "See TM spec section 4."
referenced_breadcrumbs = ["TM", "Section 4"]
"#,
    )
    .unwrap();
}

#[test]
fn framework_build_emits_expected_rows_against_synthetic_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let fw_root = tmp.path().join("fw");
    make_synthetic_framework(&fw_root);

    let out_root = tmp.path().join("api-index");
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
    let outcome = build_framework_index(
        &FrameworkBuildOpts {
            framework_root: fw_root,
            out_root: out_root.clone(),
            framework_version: "9.9.9".into(),
            framework_workspace_hash: "h".into(),
            force: false,
            vector_index_type: "ivf_flat".into(),
        },
        &embedder,
    )
    .expect("build framework index");

    assert_eq!(outcome.api_pages_count, 1);
    assert!(outcome.src_items_count >= 3);
    assert!(outcome.dataset_path.exists());
    assert!(outcome.manifest_path.exists());
    assert!(outcome.embedder_path.exists());

    // Manifest contains the expected version.
    let body = std::fs::read_to_string(&outcome.manifest_path).unwrap();
    assert!(
        body.contains("framework_version = \"9.9.9\""),
        "manifest = {body}"
    );
}

#[test]
fn spec_build_emits_three_tables_against_synthetic_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    make_synthetic_project(project);

    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
    let outcome = build_spec_index(
        &SpecBuildOpts {
            project_root: project.to_path_buf(),
            force: false,
        },
        &embedder,
    )
    .expect("build spec index");

    assert_eq!(outcome.spec_chunks_rows, 1);
    assert_eq!(outcome.signal_table_rows, 2);
    assert_eq!(outcome.cross_spec_refs_rows, 1);

    let idx = project.join(".sim-flow").join("lance-index");
    assert!(idx.join("spec_chunks.lance").exists());
    assert!(idx.join("signal_table_rows.lance").exists());
    assert!(idx.join("cross_spec_refs.lance").exists());
    assert!(idx.join("manifest.toml").exists());
    assert!(idx.join("embedder.toml").exists());
}

#[test]
fn spec_build_picks_up_spec_md_signal_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    make_synthetic_project(project);

    // Add a docs/spec.md with a Block carrying a signal.
    let docs = project.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(
        docs.join("spec.md"),
        r#"# Spec

## Blocks

### Block: IF

**Role:** Instruction Fetch
**Parent:** (none -- top-level)
**Clock domain:** cpu

#### I/O Signals

| Signal | Direction | Peer | Description |
| --- | --- | --- | --- |
| `pc` | out | IF | program counter |
"#,
    )
    .unwrap();

    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
    let outcome = build_spec_index(
        &SpecBuildOpts {
            project_root: project.to_path_buf(),
            force: false,
        },
        &embedder,
    )
    .expect("build spec index with spec.md");
    // The fixture contributes 2 source-spec rows and the spec.md
    // adds at least 1 spec-md row.
    assert!(
        outcome.signal_table_rows >= 3,
        "got {}",
        outcome.signal_table_rows
    );
}

#[test]
fn build_spec_index_check_reports_source_changed_without_index() {
    // No prior index on disk; staleness check returns SourceChanged.
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    make_synthetic_project(project);

    let staleness = is_spec_index_stale(project, None);
    assert_eq!(staleness, SpecIndexStaleness::SourceChanged);
}

#[test]
fn cli_build_spec_index_check_runs_against_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    make_synthetic_project(&project);

    let out = run_cli(&[
        "--project",
        project.to_str().unwrap(),
        "build-spec-index",
        "--check",
    ]);
    assert!(
        out.status.success(),
        "stdout = {}, stderr = {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("build-spec-index --check:"),
        "stdout = {stdout}"
    );
}

#[test]
fn cli_build_framework_index_help_lists_flags() {
    let out = run_cli(&["build-framework-index", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--framework-root"));
    assert!(stdout.contains("--out"));
    assert!(stdout.contains("--embedder"));
    assert!(stdout.contains("--force"));
}

#[test]
fn cli_build_spec_index_help_lists_flags() {
    let out = run_cli(&["build-spec-index", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--project"));
    assert!(stdout.contains("--embedder"));
    assert!(stdout.contains("--force"));
    assert!(stdout.contains("--check"));
}

#[test]
fn cli_refresh_spec_help_lists_flags() {
    let out = run_cli(&["refresh-spec", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--project"));
}

#[test]
fn cli_refresh_spec_errors_when_no_ingest_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_cli(&["--project", tmp.path().to_str().unwrap(), "refresh-spec"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("manifest")
            || stderr.contains("refresh-spec")
            || stderr.contains("read manifest"),
        "stderr = {stderr}"
    );
}

#[test]
fn semantic_search_round_trip_via_mock_embedder() {
    // Build a spec index against the synthetic fixture, then open
    // the resulting tree via LanceConnection and run a
    // semantic_search_spec call. Asserts the round-trip produces at
    // least one hit and the hit decodes cleanly into our SpecHit
    // shape.
    use sim_flow::__internal::session::lance_index::connection::LanceConnection;
    use sim_flow::__internal::session::lance_index::query::semantic_search_spec;

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    make_synthetic_project(project);

    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
    build_spec_index(
        &SpecBuildOpts {
            project_root: project.to_path_buf(),
            force: false,
        },
        &embedder,
    )
    .expect("build");

    let index_root = project.join(".sim-flow").join("lance-index");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let hits = rt.block_on(async {
        let conn = LanceConnection::open_spec(&index_root, None)
            .await
            .expect("open spec");
        let vec = embedder.embed(&["introduction"]).await.unwrap();
        semantic_search_spec(&conn, &vec[0], 5, None, None)
            .await
            .expect("search")
    });

    assert!(!hits.is_empty(), "expected at least one spec hit");
    assert_eq!(hits[0].source_id, "primary");
}

#[test]
fn signal_table_query_via_filter() {
    use sim_flow::__internal::session::lance_index::connection::LanceConnection;
    use sim_flow::__internal::session::lance_index::query::{SignalFilter, query_signal_table};

    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    make_synthetic_project(project);

    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
    build_spec_index(
        &SpecBuildOpts {
            project_root: project.to_path_buf(),
            force: false,
        },
        &embedder,
    )
    .expect("build");

    let index_root = project.join(".sim-flow").join("lance-index");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let rows = rt.block_on(async {
        let conn = LanceConnection::open_spec(&index_root, None)
            .await
            .expect("open spec");
        query_signal_table(
            &conn,
            &SignalFilter {
                signal_name: Some("pc".into()),
                ..Default::default()
            },
            10,
        )
        .await
        .expect("query")
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].signal_name, "pc");
    assert_eq!(rows[0].direction, "out");
}

/// Suppress dead-code warnings in fixtures unused by every test.
#[allow(dead_code)]
fn _suppress_unused_warning(_p: PathBuf) {}
