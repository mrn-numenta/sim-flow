# Chapter 5: Rig Integration

This chapter specifies how sim-flow uses the rig crate. The
short version: rig provides an HTTP client for embedding
endpoints and nothing else. sim-flow defines its own
`EmbeddingClient` abstraction; the v1 implementation wraps
rig's OpenAI-compat provider. Everything else in rig is
explicitly NOT used.

## 5.1 Purpose

Two needs drive the rig dependency:

- **One wire format spans the deployment contexts.** Local
  development on a MacBook M5 Max uses Ollama, which exposes
  OpenAI-compat embedding endpoints at
  `http://localhost:11434/v1/embeddings`. A shared embed
  server on an A100 GPU uses vLLM, which exposes the same
  endpoint shape at its own base URL. Hosted services (Voyage,
  OpenAI direct) use the same wire format. Pointing rig's
  `openai` provider at a custom base URL covers all three.
- **We need a maintained HTTP embedding client.** Building one
  by hand against ureq adds code we'd own forever. rig is
  already a transitive dependency we'd reach for if/when we
  add new HTTP backends; using its embedding client is the
  cheap slice.

## 5.2 Scope: Embedding Client Only

sim-flow uses rig for:

- `rig::providers::openai::Client` — HTTP client construction.
- `rig::providers::openai::EmbeddingModel` — the embedding
  endpoint wrapper.
- `rig::embeddings::EmbeddingModel` (trait) — the abstraction
  we program against.

sim-flow does NOT use rig for:

- **`rig::Agent` / `rig::agent::*`**. The agent abstraction
  collides with sim-flow's orchestrator, state machine,
  gates, per-step tool scoping, and subprocess-CLI client
  support. Already analyzed in the brainstorm collection.
- **`rig::completions::*`**. Completion routing stays through
  sim-flow's existing adapters
  ([`AnthropicAgent`](../../src/__internal/session/agent/anthropic/mod.rs)
  for the Messages API, the OpenAI-compat agent for vLLM,
  subprocess-CLI clients for Claude Code / codex / copilot).
  Prompt caching, extended thinking, family-specific
  normalizers, and seed handling are sim-flow's
  responsibility.
- **`rig::extractor`**. Structured-output salvage stays in
  sim-flow's existing critique-JSON salvage path plus the
  planned `try_repair_json` helper.
- **`rig::vector_store::*` including the `rig-lancedb`
  companion crate**. sim-flow opens LanceDB directly via the
  `lancedb` crate. The rig wrapper adds another layer that
  doesn't carry its weight given Chapter 3's storage schema
  is sim-flow-specific (separate tables for spec_chunks,
  signal_table_rows, cross_spec_refs).
- **`rig::pipeline` / RAG primitives**. sim-flow's retrieval
  flow is Chapter 4's tools, not a rig pipeline.

## 5.3 The EmbeddingClient Trait

sim-flow defines its own trait so the rig dependency is
swappable:

```rust
#[async_trait::async_trait]
pub trait EmbeddingClient: Send + Sync {
    /// Provider identifier, e.g. "openai-compat".
    fn provider(&self) -> &str;

    /// Model identifier, e.g. "nomic-embed-text".
    fn model_id(&self) -> &str;

    /// Output vector dimension. Constant for the lifetime of the client.
    fn dimension(&self) -> usize;

    /// Embed a batch of texts. Returns one vector per input, in order.
    /// The caller is responsible for any chunking; this call does not
    /// truncate or split inputs beyond what the provider requires.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
}
```

`EmbedError` is sim-flow's error enum:

```rust
pub enum EmbedError {
    DimensionMismatch { expected: usize, got: usize },
    ProviderHttp(String),
    Timeout,
    AuthFailed,
    EmptyResponse,
    Other(Box<dyn std::error::Error + Send + Sync>),
}
```

Consumers (Chapter 3's index-build pipeline, Chapter 4's
retrieval tools) only program against `EmbeddingClient`. The
concrete type is constructed once at startup and held behind
an `Arc<dyn EmbeddingClient>`.

## 5.4 OpenAiCompatEmbedder (v1 Implementation)

The single v1 implementation wraps a `rig::providers::openai::Client`
pointed at the configured base URL:

```rust
pub struct OpenAiCompatEmbedder {
    inner: rig::providers::openai::Client,
    model_id: String,
    dimension: usize,
    batch_size: usize,
    timeout: Duration,
    retry: RetryPolicy,
}

#[async_trait]
impl EmbeddingClient for OpenAiCompatEmbedder {
    fn provider(&self) -> &str { "openai-compat" }
    fn model_id(&self) -> &str { &self.model_id }
    fn dimension(&self) -> usize { self.dimension }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        // Batch into <= self.batch_size groups
        // For each batch, call rig's embedding model with retry+timeout
        // Verify each returned vector has length == self.dimension
        // Concatenate batch results in order
    }
}
```

The constructor reads `embedder.toml` (§5.6) and validates the
dimension by issuing a smoke embed of the literal string
`"sim-flow embedder probe"`. If the returned vector length
disagrees with `embedder.toml.dimension`, construction fails;
the user fixes the config and retries.

Future implementations may target rig providers other than
openai-compat (e.g. `rig::providers::anthropic::EmbeddingModel`
if Anthropic ships one, `rig::providers::voyage_ai`,
`rig::providers::cohere`). Each lands as a sibling struct
implementing the same trait; the rest of sim-flow is unchanged.

## 5.5 Recommended Models per Backend

These are operational recommendations, not normative
architecture. The system works with any OpenAI-compat embedding
endpoint.

### Local (Ollama on M5 Max)

- **`nomic-embed-text`** — 768-dim, ~270MB, fast on M5 Max.
  Default recommendation: offline-capable, low memory, good
  general-purpose recall. Loads in ~1-2 seconds on cold start.
- **`mxbai-embed-large`** — 1024-dim, ~700MB, slower but
  higher-quality recall on technical content.
- **`bge-m3`** — 1024-dim, ~2.3GB, multilingual + long-context.
  Overkill for typical English hardware specs.

### Remote (vLLM on A100)

- **`BAAI/bge-m3`** — 1024-dim, recommended for shared infra:
  fast on A100, strong on technical retrieval. Single model
  serves multiple developers.
- **`Snowflake/snowflake-arctic-embed-l`** — 1024-dim, optimized
  for retrieval.

### Hosted

- **`voyage-3` via Voyage** — 1024-dim, purpose-built for code.
  Best-in-class on code retrieval at the time of writing.
  Requires an API key; not offline.
- **`text-embedding-3-small` via OpenAI** — 1536-dim, cheap,
  broad. Acceptable fallback.

The recommended default for new installations is Ollama with
`nomic-embed-text`: zero config, fully local, fast enough on
M5 Max.

## 5.6 Configuration

A single embedder configuration file. The orchestrator looks
up in priority order:

1. `<project>/.sim-flow/embedder.toml` (per-project override).
2. `$SIM_FLOW_EMBEDDER_CONFIG` env var (operator override).
3. `~/.sim-flow/embedder.toml` (user-default).

The first found wins. The format:

```toml
schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768

[auth]
# Optional. Absent means no Authorization header.
header_name = "Authorization"
env_var = "SIM_FLOW_EMBED_API_KEY"
value_prefix = "Bearer "

[performance]
batch_size = 32
max_input_chars = 8000     # truncate at this length before sending; warn on truncate
timeout_secs = 30

[retry]
max_attempts = 3
initial_delay_ms = 200
max_delay_ms = 2000
```

`auth.env_var` names the environment variable holding the API
key. The orchestrator reads the variable at startup; an empty
or missing value with `[auth]` present is a hard config error.
This separates credentials from configuration files (env vars
do not get checked into source control).

`performance.max_input_chars` is a safety cap: inputs longer
than this are truncated to fit the embedder's context window.
A warning fires when truncation happens; the agent's typical
remedy is to re-chunk the input.

## 5.7 Async-to-Sync Boundary

Repeated from Chapter 4 for completeness: the orchestrator's
tool dispatch is synchronous, but rig (and lance) are async.
The boundary lives in `RetrievalService`:

```rust
pub struct RetrievalService {
    rt: tokio::runtime::Runtime,
    embedder: Arc<dyn EmbeddingClient>,
    framework_db: Arc<LanceConnection>,
    spec_db: Arc<LanceConnection>,
    // ...
}

impl RetrievalService {
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        self.rt.block_on(async {
            let vecs = self.embedder.embed(&[text]).await?;
            Ok(vecs.into_iter().next().unwrap())
        })
    }
}
```

Single-threaded current-thread tokio runtime; constructed once
per orchestrator session (lazy on first retrieval-tool call);
lifetime matches the session. No async leakage past
`RetrievalService` — the orchestrator's main loop, tool
dispatch, and all the existing CLI / HTTP / subprocess client
code stay synchronous.

## 5.8 Version Pinning

The rig crate version is pinned exactly in `Cargo.toml`:

```toml
[dependencies]
rig-core = "=0.37.0"
```

Reason: rig's README explicitly warns "future updates will
contain breaking changes." Exact pin (`=0.37.0`, not `^0.37`)
prevents automatic minor-bump churn. Upgrades are explicit
operations that include reviewing the rig changelog and re-
running the embedder smoke tests.

The same pinning policy applies to `lancedb` and any other
embedded-only dependencies. Library crates downstream of
sim-flow that don't have stable APIs get exact pins.

## 5.9 Embedder Smoke Test (CLI)

```
sim-flow embedder check [--config <path>] [--verbose]
```

Validates the configured embedder:

1. Resolves the embedder config (§5.6 priority order).
2. Constructs the `OpenAiCompatEmbedder` (no smoke embed yet).
3. Sends a known probe text (`"sim-flow embedder probe"`).
4. Checks the returned vector dimension matches `embedder.toml.dimension`.
5. Reports success or the specific failure (auth, network,
   dimension mismatch, etc.).

This is the recommended diagnostic when:

- A new project's embedder is being configured for the first
  time.
- An index build fails with a dimension error.
- A retrieval tool returns "embedder unreachable" mid-session.

Output:

```
embedder check: ok
  provider:  openai-compat
  base_url:  http://localhost:11434/v1
  model:     nomic-embed-text
  dimension: 768 (matches config)
  elapsed:   124 ms
```

## 5.10 Privacy and Data Egress

When the configured embedder is hosted (Voyage, OpenAI direct,
etc.), every embedded chunk's text is sent to the provider.
This includes source-spec content (which may be proprietary)
and spec.md content. The implications:

- Local embedders (Ollama, self-hosted vLLM) have zero data
  egress.
- Hosted embedders egress text. Operators of sim-flow are
  responsible for verifying their data-egress policy allows
  this.
- A future revision may add per-source egress policy (e.g.
  "only index sections marked 'public-ok'"); not in v1.

The recommended default of Ollama-on-M5-Max means zero data
egress out of the box.

## 5.11 Failure Modes

- **Embedder unreachable at construction**: `RetrievalService::new`
  returns an error; the orchestrator surfaces it to the user
  as "embedder configuration is unusable — run `sim-flow
  embedder check`."
- **Embedder unreachable at query time**: the retrieval tool
  returns a Tool-result with `status = error` and a diagnostic
  the agent surfaces. The orchestrator does not retry past
  the `[retry]` block's policy.
- **Dimension drift after construction** (e.g. provider
  silently upgraded a model and dimension changed): caught at
  the per-call dimension check inside `embed`. Returns
  `EmbedError::DimensionMismatch`; orchestrator surfaces.

## 5.12 What This Chapter Does Not Specify

- The full set of `rig::providers::openai::Client`
  configuration options sim-flow uses (timeouts, retries,
  connection pooling). Implementation concern; defaults from
  rig + our retry policy on top.
- The CLI for ad-hoc embedding (e.g. `sim-flow embed
  "..."`). Useful for debugging; an implementation-plan
  add-on.
- Multi-embedder configurations (e.g. embed framework with
  voyage but spec with Ollama). Out of scope for v1.
  Conceptually possible by allowing per-index embedder
  configs; future revision.
- Streaming embedding (the rig API supports it for some
  providers). Not needed at our query volumes; non-streaming
  is simpler.
