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
}
