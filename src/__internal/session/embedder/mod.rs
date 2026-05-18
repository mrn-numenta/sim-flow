//! Embedding-client abstraction (Chapter 5 of the spec/lance
//! architecture).
//!
//! This module owns sim-flow's only embedding-API surface. The
//! orchestrator and the (future) retrieval pipeline program against
//! the [`EmbeddingClient`] trait; the v1 concrete implementation is
//! [`OpenAiCompatEmbedder`] which wraps `rig::providers::openai::Client`
//! pointed at the configured base URL.
//!
//! The trait deliberately mirrors the slice of rig's
//! `EmbeddingModel` we need (a single async `embed(&[&str])`) without
//! re-exporting any rig types. Consumers carry an
//! `Arc<dyn EmbeddingClient>` and never see rig at all; the rig
//! dependency stays a pure implementation detail.
//!
//! Async lives entirely inside this module (rig is async-only). The
//! orchestrator's tool dispatch stays synchronous; the
//! `RetrievalService` (Chapter 5 §5.7, lands in a later phase) is
//! the only caller that owns the bridge `Runtime` and exposes
//! blocking `embed_one` / `embed_many` helpers built on top of this
//! trait.
//!
//! Rig usage in v1 is scoped to:
//!
//! - `rig::providers::openai::Client` — HTTP client construction.
//! - `rig::providers::openai::EmbeddingModel` — the endpoint wrapper.
//! - `rig::embeddings::EmbeddingModel` (trait) — the abstraction we
//!   program against.
//!
//! Everything else in rig (agents, completions, vector stores,
//! extractors, RAG pipelines) is explicitly NOT used — see Chapter 5
//! §5.2 for the rationale.

pub mod config;
pub mod openai_compat;

use std::fmt;

pub use config::{AuthConfig, ConfigError, EmbedderConfig, PerformanceConfig, RetryConfig};
pub use openai_compat::{ConstructError, OpenAiCompatEmbedder};

/// Single-batch embedding client. One instance is constructed at
/// startup behind an `Arc` and shared across every retrieval-tool
/// call for the orchestrator's lifetime.
///
/// Implementors are responsible for any required input batching;
/// callers pass an unbounded slice and receive one vector per input
/// in order. Batching, retry, and timeout policy live on the
/// concrete implementation, configured via the embedder config file.
#[async_trait::async_trait]
pub trait EmbeddingClient: Send + Sync {
    /// Provider identifier, e.g. `"openai-compat"`.
    fn provider(&self) -> &str;

    /// Model identifier, e.g. `"nomic-embed-text"`.
    fn model_id(&self) -> &str;

    /// Output vector dimension. Constant for the lifetime of the
    /// client; the constructor verifies the provider's actual
    /// dimension matches the configured one with a probe embed.
    fn dimension(&self) -> usize;

    /// Embed a batch of texts. Returns one vector per input, in
    /// order. The caller is responsible for any chunking; this call
    /// does not truncate or split inputs beyond what the provider
    /// requires.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Error returned from [`EmbeddingClient::embed`]. Matches the
/// surface enumerated in Chapter 5 §5.3.
#[derive(Debug)]
pub enum EmbedError {
    /// The provider returned a vector whose length does not match
    /// the dimension the client was constructed with. Indicates the
    /// provider silently switched models, the config is wrong, or
    /// the model itself drifted. Always a hard error.
    DimensionMismatch { expected: usize, got: usize },
    /// HTTP-layer failure (connection refused, 5xx after retries,
    /// malformed response). The string carries the upstream message
    /// for the agent to surface.
    ProviderHttp(String),
    /// Wall-clock timeout firing before the provider responded.
    /// Distinct from `ProviderHttp` so callers can identify the
    /// "server is slow vs server is wrong" case.
    Timeout,
    /// 401 / 403 from the provider. Typically a missing or expired
    /// API key.
    AuthFailed,
    /// Provider returned 200 but with zero embedding records. Always
    /// a provider bug; surfaced rather than papered over.
    EmptyResponse,
    /// Anything else, boxed for the rare case rig surfaces an error
    /// shape we don't have a categorical match for.
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbedError::DimensionMismatch { expected, got } => write!(
                f,
                "embedding dimension mismatch: expected {expected}, provider returned {got}"
            ),
            EmbedError::ProviderHttp(msg) => write!(f, "embedding provider HTTP error: {msg}"),
            EmbedError::Timeout => write!(f, "embedding provider timed out"),
            EmbedError::AuthFailed => write!(f, "embedding provider authentication failed"),
            EmbedError::EmptyResponse => {
                write!(f, "embedding provider returned an empty response")
            }
            EmbedError::Other(err) => write!(f, "embedding error: {err}"),
        }
    }
}

impl std::error::Error for EmbedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EmbedError::Other(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}
