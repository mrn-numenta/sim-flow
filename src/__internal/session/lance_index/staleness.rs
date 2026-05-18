//! Staleness detection helpers (Chapter 3 §3.10).
//!
//! These helpers compare a fresh on-disk world against the manifests
//! the index carries to decide whether the index needs a rebuild
//! (full or partial). The `sim-flow build-spec-index --check`
//! subcommand exposes the `spec` check as a read-only diagnostic.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::manifests::{ApiIndexManifest, EmbedderManifest, SpecIndexManifest};

/// Result of [`is_spec_index_stale`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecIndexStaleness {
    /// The on-disk world matches the recorded manifest.
    Fresh,
    /// The ingest pipeline's source SHA differs -- full re-ingest +
    /// re-index is required.
    SourceChanged,
    /// The spec.md SHA differs -- a partial rebuild of the
    /// `signal_table_rows` rows with `source_kind = "spec-md"` is
    /// sufficient.
    SpecMdChanged,
    /// The configured embedder no longer matches -- full re-embed
    /// required.
    EmbedderChanged,
}

/// Compare the manifest at `<index_root>/manifest.toml` against the
/// passed `current_framework_version` plus the workspace hash.
/// Returns `true` if the index is stale and should be rebuilt.
pub fn is_framework_index_stale(root: &Path, current_framework_version: &str) -> bool {
    let manifest_path = root.join("manifest.toml");
    let Ok(manifest) = ApiIndexManifest::load(&manifest_path) else {
        // No / unreadable manifest implies stale.
        return true;
    };
    manifest.framework_version != current_framework_version
}

/// Read the spec-ingest manifest's `source_sha256` line. Returns
/// `None` if the field is absent.
fn read_spec_ingest_source_sha256(manifest_path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(manifest_path).ok()?;
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("source_sha256") {
            let v = rest.trim().trim_start_matches('=').trim();
            let v = v.trim_matches('"');
            if v.is_empty() {
                return None;
            }
            return Some(v.to_string());
        }
    }
    None
}

/// Inspect the project tree at `project_root` and decide which kind
/// of refresh, if any, the spec index needs.
pub fn is_spec_index_stale(
    project_root: &Path,
    runtime_embedder: Option<&EmbedderManifest>,
) -> SpecIndexStaleness {
    let index_root = project_root.join(".sim-flow").join("lance-index");
    let manifest_path = index_root.join("manifest.toml");
    let embedder_path = index_root.join("embedder.toml");

    // Anything missing => treat as source-changed (full rebuild).
    let Ok(index_manifest) = SpecIndexManifest::load(&manifest_path) else {
        return SpecIndexStaleness::SourceChanged;
    };
    let Ok(index_embedder) = EmbedderManifest::load(&embedder_path) else {
        return SpecIndexStaleness::EmbedderChanged;
    };

    if let Some(rt) = runtime_embedder
        && !index_embedder.matches(rt)
    {
        return SpecIndexStaleness::EmbedderChanged;
    }

    let ingest_manifest_path = project_root
        .join(".sim-flow")
        .join("spec-ingest")
        .join("manifest.toml");
    if let Some(current_source) = read_spec_ingest_source_sha256(&ingest_manifest_path)
        && current_source != index_manifest.spec_ingest_source_sha256
    {
        return SpecIndexStaleness::SourceChanged;
    }

    // spec.md staleness: hash the live file and compare. Missing
    // file is treated as "" (the manifest also stores "" in that
    // case).
    let spec_md_path = project_root.join("docs").join("spec.md");
    let current_spec_sha = if spec_md_path.is_file() {
        sha256_of_file(&spec_md_path).unwrap_or_default()
    } else {
        String::new()
    };
    if current_spec_sha != index_manifest.spec_md_sha256 {
        return SpecIndexStaleness::SpecMdChanged;
    }

    SpecIndexStaleness::Fresh
}

pub fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn write_index_manifest(root: &Path, source_sha: &str, spec_md_sha: &str) {
        let m = SpecIndexManifest {
            schema_version: 1,
            indexed_at: "now".into(),
            spec_ingest_manifest: "".into(),
            spec_ingest_source_sha256: source_sha.into(),
            spec_md_sha256: spec_md_sha.into(),
            counts: BTreeMap::new(),
        };
        m.save(&root.join("manifest.toml")).unwrap();
    }

    fn write_index_embedder(root: &Path, model: &str) {
        let m = EmbedderManifest {
            schema_version: 1,
            provider: "openai-compat".into(),
            base_url: "u".into(),
            model: model.into(),
            dimension: 768,
            indexed_at: "now".into(),
            auth: None,
        };
        m.save(&root.join("embedder.toml")).unwrap();
    }

    fn write_ingest_manifest(project: &Path, source_sha: &str) {
        let dir = project.join(".sim-flow").join("spec-ingest");
        fs::create_dir_all(&dir).unwrap();
        let body = format!(
            "schema_version = 1\nsource_sha256 = \"{source_sha}\"\nsource_kind = \"markdown\"\n"
        );
        fs::write(dir.join("manifest.toml"), body).unwrap();
    }

    #[test]
    fn spec_index_fresh_when_everything_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let idx = project.join(".sim-flow").join("lance-index");
        fs::create_dir_all(&idx).unwrap();
        write_index_manifest(&idx, "abc", "");
        write_index_embedder(&idx, "nomic-embed-text");
        write_ingest_manifest(project, "abc");

        let staleness = is_spec_index_stale(project, None);
        assert_eq!(staleness, SpecIndexStaleness::Fresh);
    }

    #[test]
    fn spec_index_source_changed_when_ingest_sha_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let idx = project.join(".sim-flow").join("lance-index");
        fs::create_dir_all(&idx).unwrap();
        write_index_manifest(&idx, "abc", "");
        write_index_embedder(&idx, "nomic-embed-text");
        write_ingest_manifest(project, "xyz");

        assert_eq!(
            is_spec_index_stale(project, None),
            SpecIndexStaleness::SourceChanged
        );
    }

    #[test]
    fn spec_index_spec_md_changed_when_file_appears() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let idx = project.join(".sim-flow").join("lance-index");
        fs::create_dir_all(&idx).unwrap();
        write_index_manifest(&idx, "abc", "");
        write_index_embedder(&idx, "nomic-embed-text");
        write_ingest_manifest(project, "abc");

        // Now add a spec.md.
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("spec.md"), "# Spec\n").unwrap();

        assert_eq!(
            is_spec_index_stale(project, None),
            SpecIndexStaleness::SpecMdChanged
        );
    }

    #[test]
    fn spec_index_embedder_changed_when_runtime_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let idx = project.join(".sim-flow").join("lance-index");
        fs::create_dir_all(&idx).unwrap();
        write_index_manifest(&idx, "abc", "");
        write_index_embedder(&idx, "old-model");
        write_ingest_manifest(project, "abc");

        let runtime = EmbedderManifest {
            schema_version: 1,
            provider: "openai-compat".into(),
            base_url: "u".into(),
            model: "new-model".into(),
            dimension: 768,
            indexed_at: "now".into(),
            auth: None,
        };
        assert_eq!(
            is_spec_index_stale(project, Some(&runtime)),
            SpecIndexStaleness::EmbedderChanged
        );
    }

    #[test]
    fn framework_index_stale_on_version_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let m = ApiIndexManifest {
            schema_version: 1,
            indexed_at: "now".into(),
            framework_version: "0.1.0".into(),
            framework_workspace_hash: "h".into(),
            vector_index_type: "ivf_flat".into(),
            row_count: 0,
        };
        m.save(&root.join("manifest.toml")).unwrap();

        assert!(!is_framework_index_stale(root, "0.1.0"));
        assert!(is_framework_index_stale(root, "0.2.0"));
    }
}
