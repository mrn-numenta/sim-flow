//! V1 `OpenAiCompatEmbedder` (Chapter 5 §5.4).
//!
//! Wraps `rig::providers::openai::Client` pointed at a configurable
//! base URL. The provider handles wire format; this module owns
//! input batching, dimension verification, retry, and timeout.
//!
//! All async lives inside this module. Callers either drive it from
//! their own runtime or via the `RetrievalService` bridge that lands
//! in a later phase (§5.7).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use rig::client::EmbeddingsClient;
use rig::embeddings::EmbeddingModel as RigEmbeddingModel;
use rig::providers::openai::Client as OpenAiClient;

use super::EmbedError;
use super::EmbeddingClient;
use super::config::{EmbedderConfig, RetryConfig};

/// Probe text the constructor embeds to verify the provider's
/// actual dimension matches the configured one. Constant so tests
/// can assert on the request body when mocking.
pub const PROBE_TEXT: &str = "sim-flow embedder probe";

/// Error returned from [`OpenAiCompatEmbedder::new`].
#[derive(Debug)]
pub enum ConstructError {
    /// The auth header carried a value we couldn't turn into a
    /// valid HTTP header (non-ASCII, control character, etc.).
    InvalidAuthHeader(String),
    /// rig's HTTP client builder returned an error (typically a bad
    /// URL or an unparseable header).
    Http(String),
    /// Probe embed at construction time exposed a dimension drift
    /// between config and provider. The dimension is part of the
    /// table schema downstream, so this is a hard error.
    DimensionMismatch { expected: usize, got: usize },
    /// Probe embed failed for any reason other than dimension
    /// mismatch (timeout, auth, provider 5xx after retries, etc.).
    Probe(EmbedError),
}

impl fmt::Display for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstructError::InvalidAuthHeader(msg) => {
                write!(f, "embedder auth header is not a valid HTTP header: {msg}")
            }
            ConstructError::Http(msg) => {
                write!(f, "embedder HTTP client construction failed: {msg}")
            }
            ConstructError::DimensionMismatch { expected, got } => write!(
                f,
                "embedder dimension mismatch at construction: config says {expected}, \
                 provider returned {got} for the probe embed"
            ),
            ConstructError::Probe(err) => write!(f, "embedder probe embed failed: {err}"),
        }
    }
}

impl std::error::Error for ConstructError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConstructError::Probe(err) => Some(err),
            _ => None,
        }
    }
}

/// The single v1 `EmbeddingClient` implementation.
///
/// Construction validates the provider's dimension with a probe
/// embed; once built the embedder is cheap to clone (it holds an
/// `Arc` over rig's HTTP client internally) and safe to share across
/// threads (§5.3 contract requires `Send + Sync`).
pub struct OpenAiCompatEmbedder {
    inner: Arc<OpenAiClient>,
    model_id: String,
    dimension: usize,
    batch_size: usize,
    timeout: Duration,
    retry: RetryConfig,
}

impl fmt::Debug for OpenAiCompatEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatEmbedder")
            .field("model_id", &self.model_id)
            .field("dimension", &self.dimension)
            .field("batch_size", &self.batch_size)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

impl OpenAiCompatEmbedder {
    /// Construct the embedder from a resolved config, including the
    /// probe-embed dimension check.
    pub async fn new(config: EmbedderConfig) -> Result<Self, ConstructError> {
        let mut builder = OpenAiClient::builder().base_url(&config.base_url);

        let mut extra_headers = HeaderMap::new();
        if let Some(auth) = &config.auth {
            let name = HeaderName::try_from(auth.header_name.as_str()).map_err(|e| {
                ConstructError::InvalidAuthHeader(format!(
                    "invalid header name {:?}: {e}",
                    auth.header_name
                ))
            })?;
            let value = HeaderValue::try_from(auth.header_value.as_str()).map_err(|e| {
                ConstructError::InvalidAuthHeader(format!(
                    "invalid header value for {:?}: {e}",
                    auth.header_name
                ))
            })?;
            extra_headers.insert(name, value);
        }
        if !extra_headers.is_empty() {
            builder = builder.http_headers(extra_headers);
        }
        // rig's openai builder requires an api key in its typestate
        // even though our auth header (if any) is already installed
        // above. The build-time inserter only fills `Authorization`
        // when not already present, so:
        //
        // - auth header_name == "Authorization": the pre-installed
        //   value wins; the api_key value below is dropped.
        // - any other auth header_name: the api_key contributes an
        //   `Authorization: Bearer <empty>` header. Local servers
        //   (Ollama, vLLM) ignore it; hosted providers reject the
        //   blank bearer, which is what we want -- a user pointing
        //   sim-flow at OpenAI direct without configuring
        //   `Authorization` auth gets a fast, clean 401.
        // - no auth at all: same blank-bearer behavior as above.
        let placeholder_key = "";
        let inner = builder
            .api_key(placeholder_key)
            .build()
            .map_err(|e| ConstructError::Http(e.to_string()))?;

        let timeout = Duration::from_secs(config.performance.timeout_secs);
        let this = Self {
            inner: Arc::new(inner),
            model_id: config.model.clone(),
            dimension: config.dimension,
            batch_size: config.performance.batch_size,
            timeout,
            retry: config.retry.clone(),
        };

        // Probe embed: validate the configured dimension matches
        // what the provider actually returns. Done eagerly so a
        // misconfigured embedder fails at startup rather than at
        // first index build. We special-case dimension mismatch
        // out of the inner `EmbedError` so callers see the more
        // specific `ConstructError::DimensionMismatch`.
        let probe = this.embed_one_batch(&[PROBE_TEXT.to_string()]).await;
        match probe {
            Ok(vecs) => {
                let got = vecs
                    .first()
                    .map(|v| v.len())
                    .ok_or(ConstructError::Probe(EmbedError::EmptyResponse))?;
                if got != this.dimension {
                    return Err(ConstructError::DimensionMismatch {
                        expected: this.dimension,
                        got,
                    });
                }
                Ok(this)
            }
            Err(EmbedError::DimensionMismatch { expected, got }) => {
                Err(ConstructError::DimensionMismatch { expected, got })
            }
            Err(other) => Err(ConstructError::Probe(other)),
        }
    }

    /// Issue one HTTP request to the provider for the supplied
    /// batch, applying timeout and retry policy. Returns vectors in
    /// input order. Each returned vector's dimension is checked
    /// against `self.dimension`.
    async fn embed_one_batch(&self, batch: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let model = self
            .inner
            .embedding_model_with_ndims(&self.model_id, self.dimension);

        let mut delay = Duration::from_millis(self.retry.initial_delay_ms);
        let max_delay = Duration::from_millis(self.retry.max_delay_ms);
        let max_attempts = self.retry.max_attempts.max(1);

        for attempt in 1..=max_attempts {
            let call = model.embed_texts(batch.iter().cloned());
            let result = tokio::time::timeout(self.timeout, call).await;

            match result {
                Err(_elapsed) => {
                    if attempt == max_attempts {
                        return Err(EmbedError::Timeout);
                    }
                }
                Ok(Err(rig_err)) => {
                    let mapped = map_rig_embedding_error(rig_err);
                    if !is_retryable(&mapped) || attempt == max_attempts {
                        return Err(mapped);
                    }
                }
                Ok(Ok(rig_vecs)) => {
                    if rig_vecs.is_empty() {
                        return Err(EmbedError::EmptyResponse);
                    }
                    let mut out = Vec::with_capacity(rig_vecs.len());
                    for rec in rig_vecs {
                        let v: Vec<f32> = rec.vec.into_iter().map(|x| x as f32).collect();
                        if v.len() != self.dimension {
                            return Err(EmbedError::DimensionMismatch {
                                expected: self.dimension,
                                got: v.len(),
                            });
                        }
                        out.push(v);
                    }
                    return Ok(out);
                }
            }

            // Exponential backoff before the next attempt.
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(max_delay);
        }

        // The loop always returns on the final attempt; this path
        // is unreachable but kept defensive.
        Err(EmbedError::Other(
            "retry loop exited without a result".into(),
        ))
    }
}

#[async_trait]
impl EmbeddingClient for OpenAiCompatEmbedder {
    fn provider(&self) -> &str {
        "openai-compat"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch_size) {
            // Materialize as owned strings because rig's
            // `embed_texts` takes `IntoIterator<Item = String>`.
            let owned: Vec<String> = chunk.iter().map(|s| (*s).to_string()).collect();
            let batch_out = self.embed_one_batch(&owned).await?;
            if batch_out.len() != owned.len() {
                return Err(EmbedError::Other(
                    format!(
                        "provider returned {} vectors for a {}-input batch",
                        batch_out.len(),
                        owned.len()
                    )
                    .into(),
                ));
            }
            out.extend(batch_out);
        }
        Ok(out)
    }
}

/// Map a rig `EmbeddingError` into our `EmbedError`. The provider
/// strings carry the HTTP status info (rig stringifies the response
/// body); we sniff them for auth-rejection signals.
fn map_rig_embedding_error(err: rig::embeddings::EmbeddingError) -> EmbedError {
    use rig::embeddings::EmbeddingError as E;
    match err {
        E::HttpError(http_err) => {
            let msg = http_err.to_string();
            if looks_like_auth(&msg) {
                EmbedError::AuthFailed
            } else {
                EmbedError::ProviderHttp(msg)
            }
        }
        E::ProviderError(msg) => {
            if looks_like_auth(&msg) {
                EmbedError::AuthFailed
            } else {
                EmbedError::ProviderHttp(msg)
            }
        }
        E::ResponseError(msg) => EmbedError::ProviderHttp(msg),
        E::JsonError(e) => EmbedError::Other(Box::new(e)),
        E::UrlError(e) => EmbedError::Other(Box::new(e)),
        E::DocumentError(e) => EmbedError::Other(e),
    }
}

fn looks_like_auth(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
}

fn is_retryable(err: &EmbedError) -> bool {
    match err {
        EmbedError::ProviderHttp(msg) => {
            let lower = msg.to_ascii_lowercase();
            // 5xx and rate-limit (429) are retryable. 4xx other
            // than 429 stays terminal because retrying won't help.
            lower.contains("500")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
                || lower.contains("429")
                || lower.contains("connection")
                || lower.contains("connect error")
        }
        // Timeout is handled separately (the outer loop retries on
        // its own); the function is only consulted on rig errors.
        _ => false,
    }
}

#[cfg(test)]
mod tests;
