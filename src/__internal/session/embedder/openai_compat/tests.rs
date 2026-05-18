//! Mock-server unit tests for the v1 `OpenAiCompatEmbedder`.
//!
//! Each test spins up an isolated `wiremock::MockServer` (so they
//! can run in parallel) and points an `OpenAiCompatEmbedder` at it.
//! The mocks answer at `POST /embeddings` with synthetic vectors;
//! the embedder's probe-embed at construction time is satisfied by
//! a base mock that returns a single zero-vector of the configured
//! dimension.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::super::EmbeddingClient;
use super::super::config::{
    ConfigSource, EmbedderConfig, PerformanceConfig, ResolvedAuth, RetryConfig,
};
use super::{ConstructError, OpenAiCompatEmbedder, PROBE_TEXT};
use crate::__internal::session::embedder::EmbedError;

/// Build a synthetic OpenAI-compat embeddings response with one
/// data entry per input. Each vector is filled with `(idx as f64)`
/// so tests can assert input order is preserved across batches.
fn embedding_response(num_inputs: usize, dim: usize) -> Value {
    let data: Vec<Value> = (0..num_inputs)
        .map(|i| {
            let vec: Vec<f64> = (0..dim).map(|_| i as f64).collect();
            json!({
                "object": "embedding",
                "embedding": vec,
                "index": i,
            })
        })
        .collect();
    json!({
        "object": "list",
        "data": data,
        "model": "test-model",
        "usage": {
            "prompt_tokens": 0,
            "total_tokens": 0,
        }
    })
}

/// Build a config that points at the supplied mock-server URI.
fn mk_config(base_url: String, dimension: usize, batch_size: usize) -> EmbedderConfig {
    EmbedderConfig {
        schema_version: 1,
        provider: "openai-compat".into(),
        base_url,
        model: "test-model".into(),
        dimension,
        auth: None,
        performance: PerformanceConfig {
            batch_size,
            max_input_chars: 8000,
            timeout_secs: 5,
        },
        retry: RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 10,
            max_delay_ms: 50,
        },
        source: ConfigSource::Explicit(PathBuf::from("test")),
    }
}

/// Read the JSON body sent to the embedder endpoint. Wiremock's
/// `Request` exposes the bytes; we deserialize for matching.
fn body_json(req: &Request) -> Value {
    serde_json::from_slice(&req.body).expect("request body is JSON")
}

#[tokio::test]
async fn happy_path_single_batch_returns_expected_vectors() {
    let server = MockServer::start().await;
    let dim = 4;

    // One catch-all mock that always returns one embedding per
    // input. Probe + actual embed both flow through this.
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(EchoVectorsResponder { dim })
        .mount(&server)
        .await;

    let cfg = mk_config(server.uri(), dim, 32);
    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");
    let out = emb.embed(&["alpha", "beta", "gamma"]).await.expect("embed");
    assert_eq!(out.len(), 3);
    for v in &out {
        assert_eq!(v.len(), dim);
    }
    // Index-based fill encodes the position-in-batch; the three
    // inputs of the second request (the actual embed call, not the
    // probe) were at positions 0,1,2 -> vectors filled with 0,1,2.
    assert!(out[0].iter().all(|x| (*x - 0.0).abs() < f32::EPSILON));
    assert!(out[1].iter().all(|x| (*x - 1.0).abs() < f32::EPSILON));
    assert!(out[2].iter().all(|x| (*x - 2.0).abs() < f32::EPSILON));
}

#[tokio::test]
async fn multi_batch_splits_input_into_batches_in_order() {
    let server = MockServer::start().await;
    let dim = 3;
    let batch_size = 32;

    // Responder that encodes the *call number* into the vectors so
    // we can verify batches are dispatched and concatenated in
    // order. The probe is call 0; the real embed is calls 1..=3.
    let responder = OrderedBatchResponder::new(dim);
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let cfg = mk_config(server.uri(), dim, batch_size);
    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");

    // 80 inputs at batch_size=32 -> three batches: 32, 32, 16.
    let inputs: Vec<String> = (0..80).map(|i| format!("doc-{i}")).collect();
    let in_refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    let out = emb.embed(&in_refs).await.expect("embed");
    assert_eq!(out.len(), 80);

    // The probe is call 1 (responder counts from 1). The three
    // real-embed batches are calls 2, 3, 4. The responder stamps
    // the call number into the first scalar of every returned
    // vector; we verify the layout matches the expected slicing.
    let first = |i: usize| out[i][0] as u32;
    for i in 0..32 {
        assert_eq!(first(i), 2, "batch-1 vector {i}");
    }
    for i in 32..64 {
        assert_eq!(first(i), 3, "batch-2 vector {i}");
    }
    for i in 64..80 {
        assert_eq!(first(i), 4, "batch-3 vector {i}");
    }
    // Probe + three real batches.
    assert_eq!(responder.call_count(), 4);
}

#[tokio::test]
async fn dimension_mismatch_at_probe_fails_construction() {
    let server = MockServer::start().await;
    // Mock returns 1024-dim vectors regardless of the configured
    // dimension.
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(embedding_response(1, 1024)))
        .mount(&server)
        .await;

    let cfg = mk_config(server.uri(), 768, 32);
    let err = OpenAiCompatEmbedder::new(cfg).await.expect_err("must err");
    match err {
        ConstructError::DimensionMismatch { expected, got } => {
            assert_eq!(expected, 768);
            assert_eq!(got, 1024);
        }
        other => panic!("expected DimensionMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn auth_header_is_sent_when_configured() {
    let server = MockServer::start().await;
    let dim = 4;
    // SAFETY: process-wide env; var name is uniquely scoped to
    // this test.
    let var = "SIM_FLOW_EMBED_TEST_HAPPY_AUTH";
    unsafe {
        std::env::set_var(var, "k-secret-123");
    }

    // Header-asserting mock: every request must carry the
    // Authorization header. Wiremock's `header_exists` matches on
    // presence; we then also assert the value by inspecting the
    // captured request after the call.
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(header_exists("authorization"))
        .respond_with(EchoVectorsResponder { dim })
        .mount(&server)
        .await;

    let mut cfg = mk_config(server.uri(), dim, 32);
    cfg.auth = Some(ResolvedAuth {
        header_name: "Authorization".into(),
        env_var: var.into(),
        value_prefix: "Bearer ".into(),
        header_value: "Bearer k-secret-123".into(),
    });

    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");
    let out = emb.embed(&["hello"]).await.expect("embed");
    assert_eq!(out.len(), 1);

    // Verify on the actual captured request that the bearer is
    // what we sent.
    let received = server.received_requests().await.expect("captured requests");
    let last = received.last().expect("at least one request");
    let auth = last
        .headers
        .get("authorization")
        .expect("authorization present");
    assert_eq!(auth.to_str().unwrap(), "Bearer k-secret-123");

    // SAFETY: cleanup.
    unsafe {
        std::env::remove_var(var);
    }
}

#[tokio::test]
async fn retry_succeeds_after_a_transient_503() {
    let server = MockServer::start().await;
    let dim = 4;
    let responder = FlakyResponder::new(dim, /*fail_until=*/ 2);
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut cfg = mk_config(server.uri(), dim, 32);
    // The probe also goes through the responder; with `fail_until
    // = 2` the first call (probe) returns 503, the retry of the
    // probe returns 200. The actual embed call after construction
    // succeeds first-try because the responder's counter has moved
    // on. With `max_attempts = 3` and short delays the whole test
    // completes in ~10ms.
    cfg.retry = RetryConfig {
        max_attempts: 3,
        initial_delay_ms: 5,
        max_delay_ms: 20,
    };

    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");
    let out = emb.embed(&["x", "y"]).await.expect("embed");
    assert_eq!(out.len(), 2);
    // Probe (1 fail + 1 success) + embed (1 success) = 3 total.
    assert_eq!(responder.call_count(), 3);
}

#[tokio::test]
async fn timeout_returns_timeout_error() {
    let server = MockServer::start().await;
    let dim = 4;

    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(embedding_response(1, dim)),
        )
        .mount(&server)
        .await;

    let mut cfg = mk_config(server.uri(), dim, 32);
    // Short timeout, no retry. The probe-embed will fire the
    // timeout path inside `new`.
    cfg.performance.timeout_secs = 1;
    cfg.retry = RetryConfig {
        max_attempts: 1,
        initial_delay_ms: 5,
        max_delay_ms: 5,
    };

    let err = OpenAiCompatEmbedder::new(cfg)
        .await
        .expect_err("must time out");
    match err {
        ConstructError::Probe(EmbedError::Timeout) => {}
        other => panic!("expected Probe(Timeout), got {other:?}"),
    }
}

#[tokio::test]
async fn empty_input_returns_empty_output() {
    let server = MockServer::start().await;
    let dim = 4;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(EchoVectorsResponder { dim })
        .mount(&server)
        .await;

    let cfg = mk_config(server.uri(), dim, 32);
    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");
    let out = emb.embed(&[]).await.expect("embed");
    assert!(out.is_empty());
    // The probe still happened; the empty-input call must not
    // hit the wire.
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "only the probe request was sent");
    // Sanity-check that the probe carried the documented text.
    let body = body_json(&received[0]);
    assert_eq!(body["input"][0].as_str().unwrap(), PROBE_TEXT);
}

// ---------------------------------------------------------------
// Wiremock responders
// ---------------------------------------------------------------

/// Returns one embedding per requested input, with each vector
/// pre-filled by the input's position in the batch. Mirrors what a
/// real provider returns shape-wise; tests use the per-index fill to
/// confirm input order is preserved.
#[derive(Clone)]
struct EchoVectorsResponder {
    dim: usize,
}

impl Respond for EchoVectorsResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let n = body
            .get("input")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        ResponseTemplate::new(200).set_body_json(embedding_response(n, self.dim))
    }
}

/// Returns one vector per input, with the vector pre-filled by the
/// monotonically-increasing *call counter* (so multi-batch tests can
/// see which batch each vector came from).
#[derive(Clone)]
struct OrderedBatchResponder {
    dim: usize,
    counter: Arc<AtomicUsize>,
}

impl OrderedBatchResponder {
    fn new(dim: usize) -> Self {
        Self {
            dim,
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.counter.load(Ordering::SeqCst)
    }
}

impl Respond for OrderedBatchResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let call = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let n = body
            .get("input")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let dim = self.dim;
        let data: Vec<Value> = (0..n)
            .map(|i| {
                let vec: Vec<f64> = (0..dim).map(|_| call as f64).collect();
                json!({
                    "object": "embedding",
                    "embedding": vec,
                    "index": i,
                })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": data,
            "model": "test-model",
            "usage": { "prompt_tokens": 0, "total_tokens": 0 }
        }))
    }
}

/// Returns 503 for the first `fail_until - 1` calls and a normal
/// 200 thereafter. Used to verify retry-with-backoff.
#[derive(Clone)]
struct FlakyResponder {
    dim: usize,
    fail_until: usize,
    counter: Arc<AtomicUsize>,
}

impl FlakyResponder {
    fn new(dim: usize, fail_until: usize) -> Self {
        Self {
            dim,
            fail_until,
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.counter.load(Ordering::SeqCst)
    }
}

impl Respond for FlakyResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let call = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        if call < self.fail_until {
            ResponseTemplate::new(503).set_body_string("upstream busy")
        } else {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let n = body
                .get("input")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            ResponseTemplate::new(200).set_body_json(embedding_response(n, self.dim))
        }
    }
}
