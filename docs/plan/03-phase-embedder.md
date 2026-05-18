# Phase 3: Embedder Abstraction

## Goal

Implement the `EmbeddingClient` trait and the v1
`OpenAiCompatEmbedder` that wraps rig's openai-compat provider,
the `embedder.toml` config loader, and the `sim-flow embedder
check` CLI. The acceptance gate is a smoke test against a local
Ollama running `nomic-embed-text` returning vectors of the
expected dimension.

## Inputs

- Architecture Chapter 5 (full).
- The rig crate at exactly version 0.37.0.

## Outputs

- New module `src/__internal/session/embedder/`.
- New CLI subcommand `sim-flow embedder check`.
- Unit tests (mock-server based).
- Smoke test (gated by `SIM_FLOW_E2E_LIVE=1`).

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds with rig
      added.
- [ ] `cargo test --package sim-flow embedder::` passes.
- [ ] `SIM_FLOW_E2E_LIVE=1 cargo test --package sim-flow
      --test embedder_live` passes against local Ollama with
      `nomic-embed-text`.
- [ ] `sim-flow embedder check` returns success against the
      local Ollama default config.

## Milestones

### Milestone 3.1: Dependency + module scaffolding

- [x] Add `rig-core = "=0.37.0"` to
      `tools/sim-flow/Cargo.toml` (exact version pin per
      Chapter 5 §5.8).
- [x] Add `async-trait` and `tokio` (current-thread runtime,
      already a transitive dep but ensure direct).
- [x] Create `src/__internal/session/embedder/mod.rs` with
      module wiring.
- [x] Define `EmbeddingClient` trait per Chapter 5 §5.3.
- [x] Define `EmbedError` enum per Chapter 5 §5.3.
- [x] Verify `cargo build` succeeds; verify no unused-import
      lints.

Gate: `cargo build` succeeds.

### Milestone 3.2: Embedder config types and loader

- [x] In `embedder/config.rs`, define `EmbedderConfig`,
      `AuthConfig`, `PerformanceConfig`, `RetryConfig` types
      with serde derives matching Chapter 5 §5.6.
- [x] Implement `EmbedderConfig::load() ->
      Result<EmbedderConfig, ConfigError>` with the priority
      order:
  - `<cwd>/.sim-flow/embedder.toml`.
  - `$SIM_FLOW_EMBEDDER_CONFIG` env var.
  - `~/.sim-flow/embedder.toml`.
- [x] Apply defaults (per §5.6) for missing keys.
- [x] Read auth value from the named env var; error if
      `[auth]` present but env var unset / empty.
- [x] Unit tests for each priority level (use a tmp HOME).
- [x] Unit test asserting defaults fill in correctly.

Gate: `cargo test embedder::config::` passes.

### Milestone 3.3: OpenAiCompatEmbedder implementation

- [x] In `embedder/openai_compat.rs`, define
      `OpenAiCompatEmbedder` struct holding
      `rig::providers::openai::Client`, model id, dimension,
      batch size, timeout, and retry policy.
- [x] Implement `OpenAiCompatEmbedder::new(config:
      EmbedderConfig) -> Result<Self, ConstructError>`
      including a smoke embed of `"sim-flow embedder probe"`
      to validate the dimension. On dimension mismatch return
      `ConstructError::DimensionMismatch`.
- [x] Implement `#[async_trait] impl EmbeddingClient for
      OpenAiCompatEmbedder`:
  - Batch inputs into groups of `batch_size`.
  - For each batch, call rig's embedding API with timeout and
    retry.
  - Verify each returned vector matches `dimension`; emit
    `EmbedError::DimensionMismatch` on drift.
  - Concatenate batch results in input order.
- [x] Implement retry with exponential backoff per
      `RetryConfig`.

Gate: `cargo build` succeeds; type-check passes.

### Milestone 3.4: Mock-server unit tests for the embedder

- [x] Add `wiremock` (or `mockito`) to `[dev-dependencies]`.
- [x] Write `embedder/openai_compat/tests.rs` with mock-server
      tests covering:
  - Happy path: single batch, returns expected vectors.
  - Multi-batch: input of 80 chunks at batch_size=32 produces
    three batches in order.
  - Dimension mismatch: mock returns 1024-dim vectors against
    a 768-dim config; `EmbedError::DimensionMismatch` raised.
  - Auth header: mock asserts the `Authorization` header was
    present when `[auth]` was configured.
  - Retry: mock fails first attempt with 503, succeeds on
    second; the embedder retries and eventually succeeds.
  - Timeout: mock delays response past `timeout_secs`;
    `EmbedError::Timeout` raised.

Gate: all mock-server unit tests pass.

### Milestone 3.5: Live smoke test against Ollama

- [x] Create `tests/embedder_live.rs` gated by
      `SIM_FLOW_E2E_LIVE=1` (matches the existing
      live-test convention).
- [x] Test 1: construct `OpenAiCompatEmbedder` with
      base_url=`http://localhost:11434/v1`,
      model=`nomic-embed-text`, dimension=`768`; assert
      construction succeeds.
- [x] Test 2: embed a single string; assert the returned
      vector has length 768.
- [x] Test 3: embed a batch of 10 strings; assert 10 vectors
      returned in order.
- [x] Document in the test file's header that Ollama must be
      running locally with `nomic-embed-text` pulled.

Gate: `SIM_FLOW_E2E_LIVE=1 cargo test --test embedder_live`
passes when Ollama is running.

### Milestone 3.6: `sim-flow embedder check` CLI

- [x] In the appropriate CLI module, register an `embedder`
      command with a `check` subcommand.
- [x] `sim-flow embedder check [--config <path>] [--verbose]`
      runs the construction smoke embed and prints the result
      per Chapter 5 §5.9.
- [x] Exit 0 on success, exit 1 on construction failure
      (with the error printed).
- [x] Unit test invoking the CLI handler programmatically
      with a mock embedder server.

Gate: CLI subcommand works against the live Ollama (manual
verification on a developer's machine).

### Milestone 3.7: Documentation

- [ ] Add a section to the sim-flow README or to
      `docs/embedder.md` (new) describing:
  - How to install Ollama and pull `nomic-embed-text`.
  - How to write `embedder.toml`.
  - How to point at a remote vLLM server.
  - How to use a hosted provider (Voyage / OpenAI).
- [ ] Add a `cargo doc` doc-comment header on
      `src/__internal/session/embedder/mod.rs` summarizing
      the trait + the v1 implementation choice.

Gate: documentation file exists; `cargo doc` succeeds without
warnings on the embedder module.

## Out of Scope (deferred to later phases)

- **Wiring the embedder into the lance index build.** Phase
  4.
- **Wiring the embedder into the retrieval tools.** Phase 5.
- **Additional rig providers** (Anthropic, Voyage direct,
  etc.). v1 supports only openai-compat.
- **Per-index embedder selection** (different embedder for
  framework vs spec). v1 uses one global embedder.
- **Streaming embedding.** Not needed at our throughput.
