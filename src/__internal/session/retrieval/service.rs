//! Sync / async bridge for the Phase 5 retrieval tools.
//!
//! Constructed once per orchestrator session. Holds:
//!
//! - A current-thread tokio runtime (lazy `block_on` target).
//! - The orchestrator's `EmbeddingClient` (Phase 3).
//! - An optional framework `LanceConnection` (the shared
//!   `~/.sim-flow/lance-index/api/` tree, when built).
//! - An optional spec `LanceConnection` (the per-project
//!   `<project>/.sim-flow/lance-index/` tree, when built).
//!
//! Missing connections are not errors at construction time; the
//! tools surface structured "index missing" errors at call time so
//! the agent can self-correct (e.g. by recommending the user run
//! `sim-flow build-framework-index`).
//!
//! The synchronous query wrappers (`semantic_search_framework_sync`,
//! `semantic_search_spec_sync`, `query_signal_table_sync`,
//! `find_signal_conflicts_sync`) `block_on` the async functions
//! from `lance_index::query`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::runtime::Runtime;

use crate::__internal::session::embedder::{
    EmbedError, EmbedderConfig, EmbeddingClient, OpenAiCompatEmbedder,
};
use crate::__internal::session::lance_index::connection::LanceConnection;
use crate::__internal::session::lance_index::manifests::EmbedderManifest;
use crate::__internal::session::lance_index::query::{
    FrameworkHit, QueryError, SignalConflict, SignalFilter, SignalRow, SpecHit,
    find_signal_conflicts, query_signal_table, semantic_search_framework, semantic_search_spec,
};

/// Errors the retrieval service surfaces. Each variant maps to a
/// structured tool-result error string in the Chapter 4 §§4.2–4.4
/// "Failure modes" sections.
#[derive(Debug)]
pub enum RetrievalError {
    /// The relevant index (framework or spec) is not built. The tool
    /// returns a "run `sim-flow build-*-index`" hint to the agent.
    IndexMissing { which: &'static str },
    /// The embedder rejected the query (network, dimension, etc.).
    Embed(EmbedError),
    /// The lance / arrow layer rejected the query.
    Query(QueryError),
}

impl std::fmt::Display for RetrievalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetrievalError::IndexMissing { which } => {
                write!(f, "{which} index not built")
            }
            RetrievalError::Embed(e) => write!(f, "embedder error: {e}"),
            RetrievalError::Query(e) => write!(f, "lance query error: {e}"),
        }
    }
}

impl std::error::Error for RetrievalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetrievalError::Embed(e) => Some(e),
            RetrievalError::Query(e) => Some(e),
            _ => None,
        }
    }
}

impl From<EmbedError> for RetrievalError {
    fn from(e: EmbedError) -> Self {
        RetrievalError::Embed(e)
    }
}

impl From<QueryError> for RetrievalError {
    fn from(e: QueryError) -> Self {
        RetrievalError::Query(e)
    }
}

/// The bridge. Synchronous from the orchestrator's point of view;
/// internally each query `block_on`s the async function.
pub struct RetrievalService {
    rt: Runtime,
    embedder: Arc<dyn EmbeddingClient>,
    framework: Option<LanceConnection>,
    spec: Option<LanceConnection>,
    /// `true` until the first retrieval call records its
    /// "warming retrieval index" diagnostic. The orchestrator flips
    /// this via [`RetrievalService::mark_warmed`] so subsequent calls
    /// no longer announce the cold-start cost.
    warmed: Mutex<bool>,
}

impl std::fmt::Debug for RetrievalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetrievalService")
            .field("embedder_provider", &self.embedder.provider())
            .field("embedder_model", &self.embedder.model_id())
            .field("embedder_dimension", &self.embedder.dimension())
            .field("has_framework", &self.framework.is_some())
            .field("has_spec", &self.spec.is_some())
            .finish()
    }
}

/// Construction-side outcome variants useful to callers that want to
/// surface a precise reason for a missing connection (e.g. embedder
/// mismatch) rather than the generic "index missing" error.
#[derive(Debug)]
pub enum ServiceConstructError {
    Runtime(std::io::Error),
}

impl std::fmt::Display for ServiceConstructError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceConstructError::Runtime(e) => write!(f, "tokio runtime build: {e}"),
        }
    }
}

impl std::error::Error for ServiceConstructError {}

impl RetrievalService {
    /// Build the service from a project root and a pre-constructed
    /// embedder. Both lance connections are opened on a best-effort
    /// basis — a missing or unparseable tree yields `None` and is
    /// surfaced later as a structured tool error.
    ///
    /// The framework tree lives at `~/.sim-flow/lance-index/api/`;
    /// the spec tree at `<project>/.sim-flow/lance-index/`.
    pub fn new(
        project_root: &Path,
        embedder: Arc<dyn EmbeddingClient>,
    ) -> Result<Self, ServiceConstructError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ServiceConstructError::Runtime)?;

        let framework_root = framework_index_root();
        let spec_root = project_root.join(".sim-flow").join("lance-index");

        let runtime_embedder = embedder_manifest_for(embedder.as_ref());

        let framework = framework_root.as_ref().and_then(|fr| {
            if !fr.is_dir() {
                return None;
            }
            rt.block_on(LanceConnection::open_framework(
                fr,
                runtime_embedder.as_ref(),
            ))
            .ok()
        });

        let spec = if spec_root.is_dir() {
            rt.block_on(LanceConnection::open_spec(
                &spec_root,
                runtime_embedder.as_ref(),
            ))
            .ok()
        } else {
            None
        };

        Ok(Self {
            rt,
            embedder,
            framework,
            spec,
            warmed: Mutex::new(false),
        })
    }

    /// Build the service from the project's `embedder.toml` config
    /// file. Equivalent to `new(project_root, OpenAiCompatEmbedder::new(...))`
    /// but with the embedder construction happening behind the same
    /// runtime so callers don't need their own tokio runtime to bring
    /// up an embedder.
    pub fn from_embedder_config(
        project_root: &Path,
        config: EmbedderConfig,
    ) -> Result<Self, ServiceConstructError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ServiceConstructError::Runtime)?;
        // Construct embedder inside the runtime so its async probe
        // runs through the same runtime we hold onto.
        let embedder_arc: Arc<dyn EmbeddingClient> =
            match rt.block_on(async { OpenAiCompatEmbedder::new(config).await }) {
                Ok(e) => Arc::new(e),
                Err(e) => {
                    // Embedder construction failure: surface as a runtime
                    // error wrapping the underlying message. Callers fall
                    // back to "no embedder" semantics, which makes every
                    // retrieval tool return a structured embedder error
                    // at call time.
                    return Err(ServiceConstructError::Runtime(std::io::Error::other(
                        e.to_string(),
                    )));
                }
            };

        // Re-derive paths from the freshly-built runtime.
        let framework_root = framework_index_root();
        let spec_root = project_root.join(".sim-flow").join("lance-index");
        let runtime_embedder = embedder_manifest_for(embedder_arc.as_ref());

        let framework = framework_root.as_ref().and_then(|fr| {
            if !fr.is_dir() {
                return None;
            }
            rt.block_on(LanceConnection::open_framework(
                fr,
                runtime_embedder.as_ref(),
            ))
            .ok()
        });

        let spec = if spec_root.is_dir() {
            rt.block_on(LanceConnection::open_spec(
                &spec_root,
                runtime_embedder.as_ref(),
            ))
            .ok()
        } else {
            None
        };

        Ok(Self {
            rt,
            embedder: embedder_arc,
            framework,
            spec,
            warmed: Mutex::new(false),
        })
    }

    /// Provider id of the embedder this service holds. Surfaced in
    /// each retrieval tool's return shape (`embedder_used` field).
    pub fn embedder_label(&self) -> String {
        format!("{}:{}", self.embedder.provider(), self.embedder.model_id())
    }

    /// `true` when a framework connection is open. Tools check this
    /// before issuing a query so they can return a structured
    /// "index missing" error.
    pub fn has_framework(&self) -> bool {
        self.framework.is_some()
    }

    /// `true` when a spec connection is open.
    pub fn has_spec(&self) -> bool {
        self.spec.is_some()
    }

    /// Embed a single query string through the held embedder. The
    /// retrieval tools use this as the first half of every semantic-
    /// search call.
    pub fn embed_one_sync(&self, text: &str) -> Result<Vec<f32>, RetrievalError> {
        let v = self.rt.block_on(self.embedder.embed(&[text]))?;
        v.into_iter()
            .next()
            .ok_or(RetrievalError::Embed(EmbedError::EmptyResponse))
    }

    /// Top-K vector search over `framework_chunks`. Returns
    /// `IndexMissing { which: "framework" }` when the connection
    /// wasn't opened.
    pub fn semantic_search_framework_sync(
        &self,
        vector: &[f32],
        k: usize,
        kind: Option<&str>,
    ) -> Result<Vec<FrameworkHit>, RetrievalError> {
        let conn = self
            .framework
            .as_ref()
            .ok_or(RetrievalError::IndexMissing { which: "framework" })?;
        let hits = self
            .rt
            .block_on(semantic_search_framework(conn, vector, k, kind))?;
        Ok(hits)
    }

    /// Top-K vector search over `spec_chunks`.
    pub fn semantic_search_spec_sync(
        &self,
        vector: &[f32],
        k: usize,
        source: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<SpecHit>, RetrievalError> {
        let conn = self
            .spec
            .as_ref()
            .ok_or(RetrievalError::IndexMissing { which: "spec" })?;
        let hits = self
            .rt
            .block_on(semantic_search_spec(conn, vector, k, source, kind))?;
        Ok(hits)
    }

    /// Scalar query over `signal_table_rows`.
    pub fn query_signal_table_sync(
        &self,
        filter: &SignalFilter,
        limit: usize,
    ) -> Result<Vec<SignalRow>, RetrievalError> {
        let conn = self
            .spec
            .as_ref()
            .ok_or(RetrievalError::IndexMissing { which: "spec" })?;
        let rows = self.rt.block_on(query_signal_table(conn, filter, limit))?;
        Ok(rows)
    }

    /// spec-md vs source-spec conflicts (joined inside lance).
    pub fn find_signal_conflicts_sync(&self) -> Result<Vec<SignalConflict>, RetrievalError> {
        let conn = self
            .spec
            .as_ref()
            .ok_or(RetrievalError::IndexMissing { which: "spec" })?;
        let conflicts = self.rt.block_on(find_signal_conflicts(conn))?;
        Ok(conflicts)
    }

    /// Returns `true` iff this is the first retrieval call against
    /// the service and the caller should emit the cold-start
    /// diagnostic. Idempotent after the first flip.
    pub fn take_cold_start(&self) -> bool {
        let mut warmed = self.warmed.lock().expect("retrieval warmed mutex");
        if *warmed {
            false
        } else {
            *warmed = true;
            true
        }
    }

    /// Mark the service as warmed without consuming the cold-start
    /// flag. Useful for tests that want to bypass the diagnostic.
    pub fn mark_warmed(&self) {
        let mut warmed = self.warmed.lock().expect("retrieval warmed mutex");
        *warmed = true;
    }
}

/// Best-effort discovery of the framework index root. Mirrors the
/// path written by `sim-flow build-framework-index`. Returns `None`
/// when we can't resolve a home directory (very rare).
fn framework_index_root() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|b| {
        b.home_dir()
            .join(".sim-flow")
            .join("lance-index")
            .join("api")
    })
}

/// Build an `EmbedderManifest` shaped object describing the runtime
/// embedder. Used by `LanceConnection::open_*` to refuse opening
/// trees built with a different embedder. `base_url` and
/// `indexed_at` are placeholders — only `provider`, `model`, and
/// `dimension` participate in `matches`.
fn embedder_manifest_for(embedder: &dyn EmbeddingClient) -> Option<EmbedderManifest> {
    Some(EmbedderManifest {
        schema_version: 1,
        provider: embedder.provider().to_string(),
        base_url: String::new(),
        model: embedder.model_id().to_string(),
        dimension: embedder.dimension(),
        indexed_at: String::new(),
        auth: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// Trivial mock embedder for unit tests.
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
            Ok(texts.iter().map(|_| vec![0.0; self.dimension]).collect())
        }
    }

    #[test]
    fn constructs_cleanly_with_no_indexes_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        assert!(!service.has_framework());
        assert!(!service.has_spec());
    }

    #[test]
    fn framework_search_errors_when_no_index() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        let v = service.embed_one_sync("test").expect("embed");
        let err = service
            .semantic_search_framework_sync(&v, 5, None)
            .expect_err("missing framework");
        assert!(matches!(
            err,
            RetrievalError::IndexMissing { which: "framework" }
        ));
    }

    #[test]
    fn spec_search_errors_when_no_index() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        let v = service.embed_one_sync("test").expect("embed");
        let err = service
            .semantic_search_spec_sync(&v, 5, None, None)
            .expect_err("missing spec");
        assert!(matches!(
            err,
            RetrievalError::IndexMissing { which: "spec" }
        ));
    }

    #[test]
    fn signal_table_query_errors_when_no_spec_index() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        let err = service
            .query_signal_table_sync(&SignalFilter::default(), 10)
            .expect_err("missing spec");
        assert!(matches!(
            err,
            RetrievalError::IndexMissing { which: "spec" }
        ));
    }

    #[test]
    fn cold_start_flag_flips_once() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        assert!(service.take_cold_start());
        assert!(!service.take_cold_start());
        assert!(!service.take_cold_start());
    }

    #[test]
    fn cold_start_flag_is_shared_across_tool_callsites() {
        // Verify the cold-start diagnostic fires exactly once per
        // RetrievalService -- even when the first call comes from
        // a different retrieval tool than the second.
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        // First tool callsite (e.g. api_semantic_search): cold-start
        // returns true.
        assert!(service.take_cold_start());
        // Second tool callsite (e.g. spec_semantic_search) on the
        // SAME service: returns false -- the diagnostic is already
        // emitted.
        assert!(!service.take_cold_start());
        // mark_warmed remains idempotent.
        service.mark_warmed();
        assert!(!service.take_cold_start());
    }

    #[test]
    fn embedder_label_combines_provider_and_model() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let service = RetrievalService::new(tmp.path(), embedder).expect("construct");
        assert_eq!(service.embedder_label(), "mock:mock-embed");
    }
}
