//! LanceConnection -- opaque holder for an open Lance database
//! handle plus the per-tree manifests (Chapter 3 §3.11).
//!
//! Open paths refuse the connection when:
//!
//! - `manifest.toml` is missing, malformed, or carries an unknown
//!   `schema_version`.
//! - `embedder.toml` is missing or its `provider`/`model`/`dimension`
//!   triple does not match the orchestrator's runtime embedder.
//! - A required `*.lance/` dataset directory is absent.
//!
//! The struct deliberately does not expose the underlying
//! `lancedb::Connection` directly; queries flow through `query.rs`
//! which knows the table names and schemas.

use std::path::{Path, PathBuf};

use lancedb::Connection;

use super::manifests::{ApiIndexManifest, EmbedderManifest, ManifestError, SpecIndexManifest};

/// Discriminator for which tree this connection backs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    /// `~/.sim-flow/lance-index/api/`.
    Framework,
    /// `<project>/.sim-flow/lance-index/`.
    Spec,
}

/// Either tree's index manifest.
#[derive(Debug, Clone)]
pub enum IndexManifest {
    Framework(ApiIndexManifest),
    Spec(SpecIndexManifest),
}

/// An open Lance connection plus the manifests we read on open.
pub struct LanceConnection {
    pub kind: TreeKind,
    pub root: PathBuf,
    pub manifest: IndexManifest,
    pub embedder: EmbedderManifest,
    pub conn: Connection,
}

#[derive(Debug)]
pub enum OpenError {
    MissingRoot(PathBuf),
    MissingDataset(PathBuf),
    Manifest(ManifestError),
    EmbedderMismatch {
        expected_provider: String,
        expected_model: String,
        expected_dimension: usize,
        got_provider: String,
        got_model: String,
        got_dimension: usize,
    },
    Lance(lancedb::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::MissingRoot(p) => write!(f, "lance-index root missing: {}", p.display()),
            OpenError::MissingDataset(p) => {
                write!(f, "lance-index dataset missing: {}", p.display())
            }
            OpenError::Manifest(e) => write!(f, "lance-index manifest: {e}"),
            OpenError::EmbedderMismatch {
                expected_provider,
                expected_model,
                expected_dimension,
                got_provider,
                got_model,
                got_dimension,
            } => write!(
                f,
                "embedder mismatch: index built with {got_provider}/{got_model}/dim={got_dimension}, \
                 runtime expects {expected_provider}/{expected_model}/dim={expected_dimension}"
            ),
            OpenError::Lance(e) => write!(f, "lance error: {e}"),
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OpenError::Manifest(e) => Some(e),
            OpenError::Lance(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ManifestError> for OpenError {
    fn from(e: ManifestError) -> Self {
        OpenError::Manifest(e)
    }
}

impl From<lancedb::Error> for OpenError {
    fn from(e: lancedb::Error) -> Self {
        OpenError::Lance(e)
    }
}

impl LanceConnection {
    /// Open a framework tree at `root` (typically
    /// `~/.sim-flow/lance-index/api/`).
    ///
    /// `runtime_embedder`, when supplied, is the orchestrator's
    /// configured embedder. The connection refuses to open if it
    /// doesn't match the manifest's embedder.
    pub async fn open_framework(
        root: &Path,
        runtime_embedder: Option<&EmbedderManifest>,
    ) -> Result<Self, OpenError> {
        if !root.is_dir() {
            return Err(OpenError::MissingRoot(root.to_path_buf()));
        }
        let manifest = ApiIndexManifest::load(&root.join("manifest.toml"))?;
        let embedder = EmbedderManifest::load(&root.join("embedder.toml"))?;
        if let Some(rt) = runtime_embedder
            && !embedder.matches(rt)
        {
            return Err(OpenError::EmbedderMismatch {
                expected_provider: rt.provider.clone(),
                expected_model: rt.model.clone(),
                expected_dimension: rt.dimension,
                got_provider: embedder.provider.clone(),
                got_model: embedder.model.clone(),
                got_dimension: embedder.dimension,
            });
        }
        let dataset = root.join("framework_chunks.lance");
        if !dataset.is_dir() {
            return Err(OpenError::MissingDataset(dataset));
        }
        let conn = lancedb::connect(
            root.to_str()
                .ok_or_else(|| OpenError::MissingRoot(root.to_path_buf()))?,
        )
        .execute()
        .await?;
        Ok(Self {
            kind: TreeKind::Framework,
            root: root.to_path_buf(),
            manifest: IndexManifest::Framework(manifest),
            embedder,
            conn,
        })
    }

    /// Open a per-project spec tree at `root` (typically
    /// `<project>/.sim-flow/lance-index/`).
    pub async fn open_spec(
        root: &Path,
        runtime_embedder: Option<&EmbedderManifest>,
    ) -> Result<Self, OpenError> {
        if !root.is_dir() {
            return Err(OpenError::MissingRoot(root.to_path_buf()));
        }
        let manifest = SpecIndexManifest::load(&root.join("manifest.toml"))?;
        let embedder = EmbedderManifest::load(&root.join("embedder.toml"))?;
        if let Some(rt) = runtime_embedder
            && !embedder.matches(rt)
        {
            return Err(OpenError::EmbedderMismatch {
                expected_provider: rt.provider.clone(),
                expected_model: rt.model.clone(),
                expected_dimension: rt.dimension,
                got_provider: embedder.provider.clone(),
                got_model: embedder.model.clone(),
                got_dimension: embedder.dimension,
            });
        }
        for name in [
            "spec_chunks.lance",
            "signal_table_rows.lance",
            "cross_spec_refs.lance",
        ] {
            let dataset = root.join(name);
            if !dataset.is_dir() {
                return Err(OpenError::MissingDataset(dataset));
            }
        }
        let conn = lancedb::connect(
            root.to_str()
                .ok_or_else(|| OpenError::MissingRoot(root.to_path_buf()))?,
        )
        .execute()
        .await?;
        Ok(Self {
            kind: TreeKind::Spec,
            root: root.to_path_buf(),
            manifest: IndexManifest::Spec(manifest),
            embedder,
            conn,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refuses_missing_root() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let err = LanceConnection::open_framework(&missing, None).await;
        assert!(matches!(err, Err(OpenError::MissingRoot(_))));
    }

    #[tokio::test]
    async fn refuses_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("framework_chunks.lance")).unwrap();
        let err = LanceConnection::open_framework(root, None).await;
        assert!(matches!(err, Err(OpenError::Manifest(_))));
    }

    #[tokio::test]
    async fn refuses_embedder_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let api = ApiIndexManifest {
            schema_version: 1,
            indexed_at: "now".into(),
            framework_version: "0.1".into(),
            framework_workspace_hash: "h".into(),
            vector_index_type: "ivf_flat".into(),
            row_count: 0,
        };
        api.save(&root.join("manifest.toml")).unwrap();

        let emb = EmbedderManifest {
            schema_version: 1,
            provider: "openai-compat".into(),
            base_url: "u".into(),
            model: "model-a".into(),
            dimension: 768,
            indexed_at: "now".into(),
            auth: None,
        };
        emb.save(&root.join("embedder.toml")).unwrap();
        std::fs::create_dir_all(root.join("framework_chunks.lance")).unwrap();

        let runtime = EmbedderManifest {
            model: "model-b".into(),
            ..emb.clone()
        };
        let err = LanceConnection::open_framework(root, Some(&runtime)).await;
        assert!(matches!(err, Err(OpenError::EmbedderMismatch { .. })));
    }
}
