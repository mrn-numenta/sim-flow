# Embedder Configuration

sim-flow's spec/framework retrieval pipeline (Chapters 3 and 4)
runs every chunk and every retrieval query through an embedding
client. v1 supports one provider family: any service that speaks
the OpenAI-compat `POST /v1/embeddings` wire format. That covers
the three deployment contexts sim-flow targets:

- Local development on a MacBook M5 Max via **Ollama**.
- Shared embed servers on an A100 GPU via **vLLM**.
- Hosted services like **Voyage** or **OpenAI direct**.

Architecture detail: Chapter 5 of the `docs/architecture/`
collection. This document is operator-facing: how to install,
configure, and verify the embedder.

## Recommended default: Ollama on macOS

Zero data egress, no API keys, fast enough on Apple Silicon.

```bash
# 1. Install Ollama from https://ollama.ai or via brew:
brew install --cask ollama

# 2. Start the server (also auto-starts on login once installed).
ollama serve &

# 3. Pull the default embedding model (~270 MB).
ollama pull nomic-embed-text
```

The default `nomic-embed-text` model returns 768-dim vectors and
loads in ~1-2s on cold start. Higher-quality alternatives are
documented in Chapter 5 §5.5 (e.g. `mxbai-embed-large` at 1024-dim,
`bge-m3` at 1024-dim multilingual).

## Configuring `embedder.toml`

sim-flow looks for the embedder config file in priority order:

1. `<project>/.sim-flow/embedder.toml` (per-project override).
2. `$SIM_FLOW_EMBEDDER_CONFIG` env var pointing at a file path.
3. `~/.sim-flow/embedder.toml` (user-default).

The first file that exists wins. A minimal Ollama config:

```toml
schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
```

`[performance]` and `[retry]` blocks are optional; each field
defaults to a reasonable production value per Chapter 5 §5.6:

```toml
[performance]
batch_size = 32           # texts per HTTP request
max_input_chars = 8000    # truncate at this length; warns on truncate
timeout_secs = 30         # per-request wall-clock cap

[retry]
max_attempts = 3
initial_delay_ms = 200
max_delay_ms = 2000
```

`[auth]` is optional. Absent means no Authorization header is sent
(fine for Ollama, vLLM behind a private VPC, etc.). Present means
the named env var must be set at startup or sim-flow errors with a
clear message.

```toml
[auth]
header_name = "Authorization"     # defaults to "Authorization"
env_var = "SIM_FLOW_EMBED_API_KEY"
value_prefix = "Bearer "           # defaults to "Bearer "
```

The orchestrator reads `$SIM_FLOW_EMBED_API_KEY` at startup; the
file itself does not contain the secret, so it's safe to commit to
version control. Empty or unset env value with `[auth]` present is
a hard config error.

## Remote vLLM server

Point at the vLLM server's OpenAI-compat endpoint:

```toml
schema_version = 1
provider = "openai-compat"
base_url = "https://embed.internal.example.com/v1"
model = "BAAI/bge-m3"
dimension = 1024

[performance]
batch_size = 64           # vLLM batches well on A100
timeout_secs = 60         # cross-network adds latency

[auth]
env_var = "SIM_FLOW_EMBED_API_KEY"
```

## Hosted provider (Voyage / OpenAI)

Same shape; just point at the provider's base URL and use the
provider's API key. Note that **every embedded chunk's text is
sent to the provider** (Chapter 5 §5.10 -- the data-egress
implication). For Voyage:

```toml
schema_version = 1
provider = "openai-compat"
base_url = "https://api.voyageai.com/v1"
model = "voyage-3"
dimension = 1024

[auth]
env_var = "SIM_FLOW_VOYAGE_API_KEY"
```

For OpenAI:

```toml
schema_version = 1
provider = "openai-compat"
base_url = "https://api.openai.com/v1"
model = "text-embedding-3-small"
dimension = 1536

[auth]
env_var = "OPENAI_API_KEY"
```

## Verifying the configuration

`sim-flow embedder check` resolves the config, opens an HTTP
connection to the provider, and issues a probe-embed to confirm
the returned vector dimension matches `embedder.toml.dimension`:

```bash
sim-flow embedder check
# embedder check: resolving config from /path/to/embedder.toml
# embedder check: ok
#   provider:  openai-compat
#   base_url:  http://localhost:11434/v1
#   model:     nomic-embed-text
#   dimension: 768 (matches config)
#   elapsed:   124 ms
```

`--config <path>` bypasses the priority order and reads one
explicit file (handy when staging a new config without disturbing
the active one). `--verbose` also prints the auth-block presence
and retry / timeout policy.

`sim-flow embedder check` exits 0 on success and non-zero on any
of:

- Config file not found at any priority-order path.
- Config file present but unparseable.
- `[auth]` set but the named env var is missing or empty.
- Provider unreachable / authentication failure / network timeout.
- Returned vector dimension doesn't match the configured one.

This is the recommended diagnostic when a new project's embedder
fails to bootstrap or a retrieval tool returns "embedder
unreachable" mid-session.
