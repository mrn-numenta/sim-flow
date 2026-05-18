//! DM0 (Specification) flow integration.
//!
//! Phase 6 owns this module. DM0 produces `docs/spec.md` either by
//! auto-populating from an ingested source-spec corpus
//! ([`auto_populate`]) or by driving an interactive Q&A loop on top
//! of the `ask_user` tool ([`qa_loop`]). The DM0 gate-check
//! ([`gate`]) validates the artifact against the Phase 1 schema.
//!
//! Public surface is the [`run_dm0_work`] entry point plus the
//! per-section helpers each submodule exposes; the implementation
//! is split across three streams so they can land in parallel
//! without overlapping in this `mod.rs`.

pub mod auto_populate;
pub mod gate;
pub mod prompts;
pub mod qa_loop;

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// DM0 operating mode, derived from `.sim-flow/spec-ingest/manifest.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dm0Mode {
    /// `source_kind` is `pdf | markdown | text`. The auto-populate
    /// step seeds `SpecMd` from the ingest corpus; the agent
    /// completes prose sections and resolves TBDs.
    SourceDriven,
    /// `source_kind = "none"`. The agent drives an interactive Q&A
    /// loop to fill every REQUIRED field.
    NoSource,
}

/// Resolve the on-disk path to a project's ingest manifest. Kept as
/// a helper so the populate steps and `detect_mode` agree on the
/// layout (`<project>/.sim-flow/spec-ingest/manifest.toml`).
pub(crate) fn manifest_path(project_dir: &Path) -> PathBuf {
    project_dir
        .join(".sim-flow")
        .join("spec-ingest")
        .join("manifest.toml")
}

/// Read the ingest manifest at
/// `<project>/.sim-flow/spec-ingest/manifest.toml` and infer the
/// DM0 mode. Returns `Err` if the manifest is missing or malformed
/// — callers decide whether to fall back to `NoSource` or to surface
/// a diagnostic.
pub fn detect_mode(project_dir: &Path) -> Result<Dm0Mode> {
    let path = manifest_path(project_dir);
    let body = std::fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let manifest: ManifestModeOnly = toml::from_str(&body).map_err(|source| Error::TomlParse {
        path: path.clone(),
        source,
    })?;
    match manifest.source_kind.as_str() {
        "pdf" | "markdown" | "text" => Ok(Dm0Mode::SourceDriven),
        "none" => Ok(Dm0Mode::NoSource),
        other => Err(Error::State(format!(
            "dm0 detect_mode: unknown source_kind `{other}` in {}",
            path.display()
        ))),
    }
}

/// Minimal deserialiser for `detect_mode`. The manifest carries far
/// more fields (peers, chunk counts, chrome stripping, embedder
/// expectations, warnings); none of them affect mode selection so we
/// avoid coupling `detect_mode` to the full schema.
#[derive(serde::Deserialize)]
struct ManifestModeOnly {
    source_kind: String,
}

/// Outcome of a DM0 work session. Reported back to the orchestrator
/// for advancement / gate dispatch.
#[derive(Debug, Clone, Default)]
pub struct Dm0Outcome {
    pub mode: Option<Dm0Mode>,
    pub fields_filled: usize,
    pub tbds_recorded: usize,
}

/// Top-level entry point invoked by the orchestrator when the DM0
/// work session starts. Branches on [`detect_mode`] and either runs
/// the auto-populate path ([`auto_populate::run`]) or the Q&A loop
/// ([`qa_loop::drive_qa_loop`]). The orchestrator drives the LLM
/// turn loop separately; this function returns once the
/// auto-populate / Q&A bootstrapping is done.
#[allow(dead_code)]
pub fn run_dm0_work(_project_dir: &Path) -> Result<Dm0Outcome> {
    todo!("Phase 6 milestone 6.1 — wire auto_populate + qa_loop from detect_mode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_manifest(project: &Path, body: &str) {
        let dir = project.join(".sim-flow").join("spec-ingest");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.toml"), body).unwrap();
    }

    #[test]
    fn detect_mode_pdf_is_source_driven() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            "schema_version = 1\nsource_kind = \"pdf\"\nsource_path = \"x.pdf\"\n",
        );
        assert_eq!(detect_mode(tmp.path()).unwrap(), Dm0Mode::SourceDriven);
    }

    #[test]
    fn detect_mode_markdown_is_source_driven() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            "schema_version = 1\nsource_kind = \"markdown\"\nsource_path = \"x.md\"\n",
        );
        assert_eq!(detect_mode(tmp.path()).unwrap(), Dm0Mode::SourceDriven);
    }

    #[test]
    fn detect_mode_text_is_source_driven() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            "schema_version = 1\nsource_kind = \"text\"\nsource_path = \"x.txt\"\n",
        );
        assert_eq!(detect_mode(tmp.path()).unwrap(), Dm0Mode::SourceDriven);
    }

    #[test]
    fn detect_mode_none_is_no_source() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            "schema_version = 1\nsource_kind = \"none\"\nsource_path = \"\"\n",
        );
        assert_eq!(detect_mode(tmp.path()).unwrap(), Dm0Mode::NoSource);
    }

    #[test]
    fn detect_mode_missing_manifest_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        let err = detect_mode(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "got {err:?}");
    }

    #[test]
    fn detect_mode_malformed_toml_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "this is not = valid = toml");
        let err = detect_mode(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::TomlParse { .. }), "got {err:?}");
    }

    #[test]
    fn detect_mode_unknown_source_kind_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "schema_version = 1\nsource_kind = \"binary\"\n");
        let err = detect_mode(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::State(_)), "got {err:?}");
    }
}
