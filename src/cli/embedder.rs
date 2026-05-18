//! Handler for `sim-flow embedder check` (Chapter 5 §5.9).
//!
//! Resolves the configured embedder, runs `OpenAiCompatEmbedder::new`
//! (which issues a probe-embed and verifies the dimension), then
//! prints a human-readable success report or a precise failure
//! diagnostic.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use sim_flow::__internal::session::embedder::{ConfigError, EmbedderConfig, OpenAiCompatEmbedder};

/// Run the embedder smoke test.
///
/// Exit semantics (returned via `Result` to the top-level CLI
/// runner, which translates the error into a non-zero process exit
/// per Chapter 5 §5.9):
///
/// - `Ok(())` -- probe embed succeeded; the configured dimension
///   matches the provider's actual returned dimension.
/// - `Err(...)` -- config could not be resolved / parsed, or the
///   probe embed failed (auth, network, dimension mismatch, etc.).
///
/// `--config <path>` bypasses the project / env / home priority
/// resolution. `--verbose` prints the auth header presence and the
/// active retry policy in addition to the default fields.
pub(crate) fn check(config_path: Option<&Path>, verbose: bool) -> sim_flow::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let cfg = match config_path {
        Some(p) => EmbedderConfig::load_explicit(p),
        None => EmbedderConfig::load(),
    }
    .map_err(config_to_sim_flow_error)?;

    // Surface the resolved source path before we attempt the network
    // call so a debugging user sees which file fed the run even if
    // the probe fails.
    let _ = writeln!(
        out,
        "embedder check: resolving config from {}",
        cfg.source.path().display()
    );

    let cfg_for_print = (
        cfg.provider.clone(),
        cfg.base_url.clone(),
        cfg.model.clone(),
        cfg.dimension,
        cfg.auth.is_some(),
        cfg.retry.max_attempts,
        cfg.performance.timeout_secs,
    );

    let start = Instant::now();
    let result = run_async(OpenAiCompatEmbedder::new(cfg));
    let elapsed = start.elapsed();

    match result {
        Ok(_emb) => {
            let (provider, base_url, model, dimension, has_auth, retry_attempts, timeout_secs) =
                cfg_for_print;
            let _ = writeln!(out, "embedder check: ok");
            let _ = writeln!(out, "  provider:  {provider}");
            let _ = writeln!(out, "  base_url:  {base_url}");
            let _ = writeln!(out, "  model:     {model}");
            let _ = writeln!(out, "  dimension: {dimension} (matches config)");
            let _ = writeln!(out, "  elapsed:   {} ms", elapsed.as_millis());
            if verbose {
                let _ = writeln!(
                    out,
                    "  auth:      {}",
                    if has_auth { "configured" } else { "none" }
                );
                let _ = writeln!(out, "  retry:     up to {retry_attempts} attempts");
                let _ = writeln!(out, "  timeout:   {timeout_secs}s per attempt");
            }
            Ok(())
        }
        Err(err) => {
            let _ = writeln!(out, "embedder check: failed -- {err}");
            Err(sim_flow::Error::Config(format!(
                "embedder check failed: {err}"
            )))
        }
    }
}

/// Map the embedder-config error into the crate-wide `Error::Config`
/// variant. Preserves the underlying message for the human-facing
/// CLI output.
fn config_to_sim_flow_error(err: ConfigError) -> sim_flow::Error {
    sim_flow::Error::Config(err.to_string())
}

/// Drive an async future to completion using a private
/// current-thread tokio runtime. Mirrors §5.7's
/// `RetrievalService::block_on` shape but local to the CLI handler
/// because the CLI process is short-lived and there's no shared
/// `RetrievalService` to reuse.
fn run_async<F>(fut: F) -> F::Output
where
    F: std::future::Future,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio current-thread runtime");
    rt.block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Tokio runtime for the test fixtures: we hold a single runtime
    /// across multiple sync calls because `check()` itself owns one
    /// per call internally.
    fn make_rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn embedding_response(dim: usize) -> Value {
        let vec: Vec<f64> = (0..dim).map(|_| 0.0).collect();
        json!({
            "object": "list",
            "data": [
                { "object": "embedding", "embedding": vec, "index": 0 }
            ],
            "model": "test-model",
            "usage": { "prompt_tokens": 0, "total_tokens": 0 }
        })
    }

    /// Write a minimal embedder.toml pointing at the supplied URL.
    fn write_config_file(dir: &Path, base_url: &str, dim: usize) -> PathBuf {
        let p = dir.join("embedder.toml");
        std::fs::write(
            &p,
            format!(
                r#"schema_version = 1
provider = "openai-compat"
base_url = "{base_url}"
model = "test-model"
dimension = {dim}
[performance]
batch_size = 8
timeout_secs = 5
[retry]
max_attempts = 1
initial_delay_ms = 1
max_delay_ms = 1
"#
            ),
        )
        .unwrap();
        p
    }

    #[test]
    fn check_succeeds_against_mock_server_with_explicit_config() {
        let rt = make_rt();
        let (server, _guard) = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/embeddings"))
                .respond_with(ResponseTemplate::new(200).set_body_json(embedding_response(4)))
                .mount(&server)
                .await;
            (server, ())
        });

        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = write_config_file(tmp.path(), &server.uri(), 4);

        let result = check(Some(&cfg_path), /*verbose=*/ false);
        assert!(result.is_ok(), "expected check success, got {result:?}");
    }

    #[test]
    fn check_surfaces_dimension_mismatch_as_error() {
        let rt = make_rt();
        // Mock returns 1024-dim vectors against a config of 768.
        let (server, _guard) = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/embeddings"))
                .respond_with(ResponseTemplate::new(200).set_body_json(embedding_response(1024)))
                .mount(&server)
                .await;
            (server, ())
        });

        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = write_config_file(tmp.path(), &server.uri(), 768);

        let result = check(Some(&cfg_path), /*verbose=*/ true);
        let err = result.expect_err("check must error on dim mismatch");
        let msg = err.to_string();
        assert!(
            msg.contains("dimension") || msg.contains("768") || msg.contains("1024"),
            "msg = {msg}"
        );
    }
}
