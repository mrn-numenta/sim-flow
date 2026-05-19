//! Spec-ingest pipeline (Phase 2).
//!
//! The pipeline takes a primary source spec (PDF / markdown / text)
//! plus optional peer specs and produces a structured on-disk corpus
//! under `<project>/.sim-flow/spec-ingest/`. The full contract is in
//! `tools/sim-flow/docs/architecture/01-spec-ingest-pipeline.md`.
//!
//! This file re-exports both the new pipeline API (`pipeline::run`,
//! `IngestRequest`, `IngestOutcome`, ...) and the legacy
//! `ingest_spec_file` / `SpecIngestSummary` shim the rest of the
//! crate still calls into until DM0 is rewired in Phase 6. The shim
//! preserves the old `.sim-flow/spec-pages/` + `source-spec-toc.md`
//! layout so the orchestrator and `e2e_*` binaries keep compiling
//! and behaving the way they did before this refactor.

pub mod format;
pub mod legacy;
pub mod pipeline;
pub mod stages;

pub use legacy::{INLINE_THRESHOLD, SpecIngestSummary, SpecKind, ingest_spec_file};
pub use pipeline::{
    IngestConfig, IngestOutcome, IngestRequest, IngestWarning, PeerSpec, Pipeline, SourceKind,
    SourceSpec, run,
};

#[cfg(test)]
mod smoke_tests {
    use super::*;

    /// Milestone 2.1 gate: an empty pipeline (no primary source)
    /// constructs and runs without panicking, producing the
    /// expected empty-corpus manifest.
    #[test]
    fn empty_pipeline_runs_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let request = IngestRequest {
            primary: None,
            peers: Vec::new(),
            config: IngestConfig::default(),
            project_root: tmp.path().to_path_buf(),
        };
        let outcome = run(request).expect("empty pipeline runs");
        assert_eq!(outcome.primary_chunk_count, 0);
        assert!(outcome.manifest_path.exists());
        let manifest = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        assert!(manifest.contains("source_kind = \"none\""));
    }

    /// Milestone 2.13: load config with overrides; missing keys
    /// inherit defaults.
    #[test]
    fn config_load_applies_overrides_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let dot = tmp.path().join(".sim-flow");
        std::fs::create_dir_all(&dot).unwrap();
        let cfg_path = dot.join("spec-ingest.config.toml");
        std::fs::write(
            &cfg_path,
            r#"
[figures]
dpi = 220

[chrome_stripping]
appearance_threshold = 0.75
"#,
        )
        .unwrap();
        let cfg = IngestConfig::load(tmp.path()).unwrap();
        assert_eq!(cfg.figures.dpi, 220);
        assert_eq!(cfg.figures.format, "png"); // default
        assert!((cfg.chrome_stripping.appearance_threshold - 0.75).abs() < 1e-6);
        assert!(cfg.chrome_stripping.enabled); // default
        // Untouched section keeps defaults.
        assert_eq!(cfg.chunking.max_chunk_chars, 8000);
    }

    #[test]
    fn config_load_returns_defaults_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = IngestConfig::load(tmp.path()).unwrap();
        assert_eq!(cfg.figures.dpi, 150);
        assert!(cfg.chrome_stripping.enabled);
    }

    /// Milestone 2.18: a degenerate-fixture (no headings detected)
    /// produces a manifest carrying the expected warning entry plus
    /// a single-root section.
    #[test]
    fn degenerate_markdown_surfaces_no_headings_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        let spec = project.join("src.md");
        std::fs::write(&spec, "just some text\nwith no headings at all\n").unwrap();
        let outcome = pipeline::run(IngestRequest {
            primary: Some(SourceSpec::new(spec)),
            peers: Vec::new(),
            config: IngestConfig::default(),
            project_root: project.clone(),
        })
        .expect("run succeeds");
        // The outcome carries the warning back to the programmatic
        // caller.
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.code == "no_headings_detected"),
            "expected no_headings_detected warning, got {:?}",
            outcome.warnings
        );
        // And the manifest records it under [[warnings]].
        let body = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        assert!(
            body.contains("[[warnings]]"),
            "manifest is missing [[warnings]] block:\n{body}"
        );
        assert!(
            body.contains("no_headings_detected"),
            "manifest is missing no_headings_detected warning:\n{body}"
        );
    }

    /// Milestone 2.15: the chapter 1.9 programmatic API surface is
    /// callable from outside the module. We construct an
    /// `IngestRequest` against a tiny markdown source and call
    /// `pipeline::run` directly; the outcome's counts match the
    /// expected manifest.
    #[test]
    fn programmatic_api_runs_against_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().to_path_buf();
        let spec = project.join("src.md");
        std::fs::write(&spec, "# Top\nbody\n\n## Sub\nmore body\n").unwrap();
        let outcome = pipeline::run(IngestRequest {
            primary: Some(SourceSpec::new(spec)),
            peers: Vec::new(),
            config: IngestConfig::default(),
            project_root: project.clone(),
        })
        .expect("run succeeds");
        assert_eq!(outcome.primary_chunk_count, 2);
        assert!(outcome.manifest_path.exists());
        let body = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        assert!(body.contains("source_kind = \"markdown\""));
        assert!(body.contains("primary_chunk_count = 2"));
    }
}
