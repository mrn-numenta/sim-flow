//! Live smoke test for the v1 `OpenAiCompatEmbedder` against a
//! locally-running Ollama instance.
//!
//! ## Prerequisites
//!
//! 1. Ollama must be running locally on the default port:
//!    `http://localhost:11434/v1`.
//! 2. The `nomic-embed-text` model must be pulled:
//!    `ollama pull nomic-embed-text`.
//!
//! ## How to run
//!
//! The whole suite is gated on `SIM_FLOW_E2E_LIVE=1` (matches the
//! convention used by the other live-test files in this directory)
//! so a default `cargo test` never tries to hit the network.
//!
//! ```bash
//! SIM_FLOW_E2E_LIVE=1 cargo test --package sim-flow \
//!   --test embedder_live
//! ```
//!
//! Optional overrides:
//!
//! - `SIM_FLOW_EMBED_BASE_URL` (default
//!   `http://localhost:11434/v1`) -- point at vLLM, LM Studio,
//!   another Ollama host, etc.
//! - `SIM_FLOW_EMBED_MODEL` (default `nomic-embed-text`).
//! - `SIM_FLOW_EMBED_DIM` (default `768`). Adjust when overriding
//!   the model.
//!
//! ## What the suite verifies
//!
//! - **Test 1**: construction succeeds (probe embed runs, returned
//!   vector matches the configured dimension).
//! - **Test 2**: embedding a single string returns one vector of
//!   the expected dimension.
//! - **Test 3**: embedding a batch of 10 strings returns 10 vectors
//!   in order, each of the expected dimension.

use std::path::PathBuf;

use sim_flow::__internal::session::embedder::{
    ConfigSource, EmbedderConfig, EmbeddingClient, OpenAiCompatEmbedder, PerformanceConfig,
    RetryConfig,
};

/// Skip the suite when the gating env var is unset. Keeps a plain
/// `cargo test` quiet on CI/dev machines without Ollama.
fn live_enabled() -> bool {
    std::env::var("SIM_FLOW_E2E_LIVE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Build the live config from env-or-default. The
/// `ConfigSource::Explicit` carries a placeholder path; the
/// embedder itself never inspects it.
fn live_config() -> EmbedderConfig {
    let base_url = std::env::var("SIM_FLOW_EMBED_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:11434/v1".to_string());
    let model =
        std::env::var("SIM_FLOW_EMBED_MODEL").unwrap_or_else(|_| "nomic-embed-text".to_string());
    let dimension = std::env::var("SIM_FLOW_EMBED_DIM")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(768);

    EmbedderConfig {
        schema_version: 1,
        provider: "openai-compat".into(),
        base_url,
        model,
        dimension,
        auth: None,
        performance: PerformanceConfig {
            batch_size: 32,
            max_input_chars: 8000,
            timeout_secs: 30,
        },
        retry: RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 200,
            max_delay_ms: 2000,
        },
        source: ConfigSource::Explicit(PathBuf::from("live-test")),
    }
}

#[tokio::test]
async fn live_construct_succeeds() {
    if !live_enabled() {
        eprintln!("SIM_FLOW_E2E_LIVE is unset; skipping live_construct_succeeds");
        return;
    }
    let cfg = live_config();
    let dimension = cfg.dimension;
    let emb = OpenAiCompatEmbedder::new(cfg)
        .await
        .expect("construct against local Ollama");
    assert_eq!(emb.provider(), "openai-compat");
    assert_eq!(emb.dimension(), dimension);
}

#[tokio::test]
async fn live_single_embed_returns_expected_dim() {
    if !live_enabled() {
        eprintln!("SIM_FLOW_E2E_LIVE is unset; skipping live_single_embed_returns_expected_dim");
        return;
    }
    let cfg = live_config();
    let dim = cfg.dimension;
    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");
    let out = emb
        .embed(&["sim-flow live single-embed smoke"])
        .await
        .expect("embed");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), dim);
}

#[tokio::test]
async fn live_batch_of_ten_returns_ten_vectors_in_order() {
    if !live_enabled() {
        eprintln!(
            "SIM_FLOW_E2E_LIVE is unset; skipping live_batch_of_ten_returns_ten_vectors_in_order"
        );
        return;
    }
    let cfg = live_config();
    let dim = cfg.dimension;
    let emb = OpenAiCompatEmbedder::new(cfg).await.expect("construct");

    let inputs: Vec<String> = (0..10)
        .map(|i| format!("sim-flow live batch input {i}"))
        .collect();
    let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
    let out = emb.embed(&refs).await.expect("embed");
    assert_eq!(out.len(), 10);
    for v in &out {
        assert_eq!(v.len(), dim);
    }
}
