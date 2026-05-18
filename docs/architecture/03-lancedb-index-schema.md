# Chapter 3: LanceDB Index Schema

This chapter specifies the on-disk LanceDB tables sim-flow uses
to back the three retrieval tools (Chapter 4 §4.2-4.4) and the L6 cross-
spec metadata. It defines the table schemas, the embedder
manifest, the index-build / refresh operations, and the storage
layout.

## 3.1 Purpose

LanceDB is sim-flow's structured + vector retrieval store. It
holds three kinds of content:

- **Framework chunks** — embeddings + metadata over
  `foundation-framework`'s `fw:api/` pages and `fw:src/`
  source. Indexed once per framework version; shared across
  all projects.
- **Spec chunks** — embeddings + metadata over the primary
  source spec plus any registered peer specs. Indexed
  per-project after the ingest pipeline (Chapter 1) produces
  the corpus.
- **Signal-table rows** — structured rows extracted from
  signal tables in both the source spec (via Chapter 1) and
  spec.md (via the parser in Chapter 2). Per-project. Mostly
  scalar; no vector column at v1.
- **Cross-spec references** — pure scalar relational table
  recording which spec.md / source-chunk references which peer
  spec. Per-project.

The index is embedded (single-process, file-backed). The
orchestrator opens the lance directory directly; there is no
server, no daemon, no separate process.

## 3.2 Storage Layout

Two distinct trees, one shared and one per-project:

```
~/.sim-flow/lance-index/api/                     -- shared, framework-level
  manifest.toml
  embedder.toml
  framework_chunks.lance/

<project>/.sim-flow/lance-index/                 -- per-project, spec-level
  manifest.toml
  embedder.toml
  spec_chunks.lance/
  signal_table_rows.lance/
  cross_spec_refs.lance/
```

The `~/.sim-flow/lance-index/api/` location is the default
shared root. An environment variable `SIM_FLOW_API_INDEX_ROOT`
overrides it for multi-user shared installations (e.g. CI). The
per-project tree always lives under the project's
`.sim-flow/lance-index/`.

Each `*.lance/` is a Lance dataset directory (versioned
manifests + per-version data fragments). Lance owns the
internal layout; sim-flow treats it as opaque.

## 3.3 Embedder Manifest

Both trees carry an `embedder.toml` recording the embedder
identity used at index time. Queries refuse to run when the
orchestrator's configured embedder does not match the
manifest.

```toml
# embedder.toml
schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
indexed_at = "2026-05-17T10:23:45Z"

[auth]
# Optional. When absent, no auth header is sent.
header_name = "Authorization"
env_var = "SIM_FLOW_EMBED_API_KEY"
value_prefix = "Bearer "
```

Match rules: provider, model, and dimension must match exactly.
base_url is informational (recorded for diagnostics; the
orchestrator's runtime base_url wins for queries — useful when
the index was built against a local Ollama and queries are
now run against a remote vLLM serving the same model).

The `openai-compat` provider is the v1 default. Both Ollama
(macOS local on M5 Max) and vLLM (remote on A100) expose
OpenAI-compat embedding endpoints, which lets a single wire
format span the two deployment contexts.

## 3.4 framework_chunks Table

The L1 framework retrieval index. Shared across projects;
rebuilt when foundation-framework's version stamp changes.

Schema:

```
id                  string                              not null  primary key
source_path         string                              not null
kind                string                              not null    -- enum
name                string                              not null    -- "" when unknown
text                string                              not null
text_sha256         string                              not null
vector              fixed_size_list<float32, DIM>       not null
framework_version   string                              not null
chunk_byte_start    uint64                              not null
chunk_byte_end      uint64                              not null
```

`id` format: `<source_path>::<symbol-or-chunk-N>` (e.g.
`fw:src/model/dataflow/mod.rs::HasLogic`,
`fw:api/pages/foundation_framework/prelude/index.md::chunk-0`).

`kind` enumerates to: `api-page | src-fn | src-impl | src-trait
| src-mod-doc | src-other`.

`framework_version` is the version recorded at build time
(crate version + optional commit hash). Queries filter by this
to ignore stale rows after a rebuild against a new framework
version (the table is then atomically replaced; old rows do
not coexist with new).

Indices:

- Scalar BTree on `kind`, `name`, `framework_version`.
- Vector IVF_FLAT on `vector` for v1. IVF_PQ becomes an option
  if the table grows past ~50k rows; HNSW remains a
  possibility for low-latency requirements. The choice is
  configurable per-build via `manifest.toml.vector_index_type`.

Estimated size: foundation-framework's current corpus is ~864
api-pages + ~2000-3000 src items. At 768-dim float32 vectors,
that's roughly 3000 × 768 × 4 = ~9 MB of vectors plus text +
metadata; well within laptop disk budgets.

## 3.5 spec_chunks Table

The L2 source-spec retrieval index. Per-project. Rebuilt when
the spec ingest pipeline re-runs (detected via
`manifest.toml.source_sha256` change).

Schema:

```
id                  string                              not null  primary key
source_id           string                              not null    -- "primary" | "<peer_id>"
breadcrumb          list<string>                        not null
section_heading     string                              not null
source_page_start   uint32                              not null
source_page_end     uint32                              not null
kind                string                              not null    -- enum
text                string                              not null
text_sha256         string                              not null
vector              fixed_size_list<float32, DIM>       not null
contained_signal_tables   list<string>                  not null    -- table TOML paths
contained_figures         list<string>                  not null    -- raster paths
```

`id` matches the `chunk_id` from Chapter 1's
`chunks/NNN-<slug>.md` front matter — the SHA-256 of
`breadcrumb || source_page_range || body`.

`source_id` is `"primary"` for the primary spec or the peer's
id (matching `manifest.toml.peers[].id`) for chunks from
inherited / referenced specs.

`kind` enumerates to: `prose | table | stub | mixed`. Stub
chunks are still embedded so the agent can discover them via
search ("find the section about HTM" → returns the stub
chunk).

Indices:

- Scalar BTree on `source_id`, `kind`, `section_heading`.
- Vector IVF_FLAT on `vector`.

## 3.6 signal_table_rows Table

The L7 structured signal-table query index. Per-project.
Holds rows from BOTH the source spec (extracted by Chapter 1's
pipeline into `tables/signals/*.toml`) and spec.md (parsed by
Chapter 2's parser from Block subsections). Provenance is
recorded per row.

Schema:

```
row_id              string                              not null  primary key
source_kind         string                              not null    -- "source-spec" | "spec-md"
source_id           string                              not null    -- "primary" | "<peer_id>" | "spec.md"
chunk_id            string                              not null    -- source chunk (for source-spec) or spec.md anchor
stage               string                              not null    -- breadcrumb leaf
breadcrumb          list<string>                        not null
signal_name         string                              not null
direction           string                              not null    -- "in" | "out" | "inout"
peer                string                              not null
description         string                              not null
```

`row_id` format: SHA-256 of
`source_kind || source_id || stage || signal_name`.

Indices:

- Scalar BTree on `signal_name`, `stage`, `peer`, `direction`,
  `source_kind`, `source_id`.
- No vector column in v1. A future revision may add an
  embedding over `signal_name || " " || description` for
  semantic queries on signal names; deferred.

Use cases this table supports (specified in detail in Chapter
4):

- "Every signal driven from PD."
- "Every input to the ALU."
- "Where is `ex_nxt_pc` consumed?"
- "Does spec.md's signal table for IF match the source spec's
  signal table for IF?" (join on `(stage, signal_name)` across
  `source_kind`.)

## 3.7 cross_spec_refs Table

The L6 cross-spec references metadata. Per-project. Pure
scalar; no vector column.

Schema:

```
ref_id                  string                          not null  primary key
source_chunk_id         string                          not null    -- chunk that contains the reference
peer_id                 string                          not null    -- peer being referenced (matches manifest.toml.peers[].id)
peer_chunk_id           string                          not null    -- "" if unresolved
reference_text          string                          not null
referenced_breadcrumbs  list<string>                    not null
```

`ref_id` format: SHA-256 of `source_chunk_id || peer_id ||
reference_text`.

Indices: scalar BTree on `peer_id`, `source_chunk_id`.

This table powers the L6 use case "find every spec.md /
source-chunk that depends on the TM spec" and the inverse
"which references in the primary refer to TM section X."

## 3.8 manifest.toml (per-tree)

Both index trees carry a `manifest.toml` distinct from the
ingest pipeline's `manifest.toml`:

```toml
# ~/.sim-flow/lance-index/api/manifest.toml
schema_version = 1
indexed_at = "2026-05-17T10:23:45Z"
framework_version = "<crate version>"
framework_workspace_hash = "<hex digest>"   -- of crates/framework Cargo.lock + selected source files
vector_index_type = "ivf_flat"               -- "ivf_flat" | "ivf_pq" | "hnsw"
row_count = 2734
```

```toml
# <project>/.sim-flow/lance-index/manifest.toml
schema_version = 1
indexed_at = "2026-05-17T10:23:45Z"
spec_ingest_manifest = "<absolute path>"     -- the manifest.toml from Chapter 1
spec_ingest_source_sha256 = "<hex digest>"   -- copied from spec-ingest manifest for staleness check
spec_md_sha256 = "<hex digest>"              -- of the spec.md at build time, "" if not yet authored

[counts]
spec_chunks = 87
signal_table_rows = 142
cross_spec_refs = 3
```

Staleness detection compares `spec_ingest_source_sha256` and
`spec_md_sha256` against current values. Mismatch → the index
is stale and must be rebuilt (or partially refreshed; see
§3.10).

## 3.9 Build Operations

Three explicit build paths.

### 3.9.1 Framework index build

```
sim-flow build-framework-index \
    [--framework-root <path>] \      -- default: discovered foundation-framework
    [--embedder <config>] \          -- default: ~/.sim-flow/embedder.toml
    [--out <path>] \                 -- default: ~/.sim-flow/lance-index/api/
    [--force]                        -- rebuild even if not stale
```

Pipeline:

1. Resolve the embedder config (from `--embedder` or the
   default path). Validate it produces vectors of the
   expected dimension by a smoke call.
2. Walk `fw:api/pages/**/*.md`. Each file is one chunk.
3. Walk `fw:src/**/*.rs`. Use `syn` to parse and produce one
   chunk per top-level item (`fn`, `impl` block, `trait` def,
   module doc-comment).
4. For each chunk, compute `text_sha256` and check against
   any existing row with the same `id`. Skip embedding if
   sha256 matches an existing row (incremental rebuild).
5. Batch-embed missing chunks. Batch size is embedder-
   dependent (configurable in `embedder.toml`'s
   `[performance]` block; default 32).
6. Write all rows to a new Lance dataset at `<out>.tmp/`.
7. Atomically rename `<out>.tmp/` over `<out>/`.
8. Write the updated `manifest.toml` and `embedder.toml`.

Re-runs are idempotent and incremental: the SHA-256 check
avoids re-embedding unchanged chunks. A `--force` flag
re-embeds everything (used when the embedder model changes).

### 3.9.2 Per-project spec index build

```
sim-flow build-spec-index \
    [--project <root>] \             -- default: cwd
    [--embedder <config>] \          -- default: ~/.sim-flow/embedder.toml
    [--force]
```

Pipeline:

1. Read `<project>/.sim-flow/spec-ingest/manifest.toml`. Hard
   error if absent (ingestion must precede indexing).
2. Resolve and validate the embedder.
3. Walk `<project>/.sim-flow/spec-ingest/primary/chunks/*.md`
   plus every `peers/<peer>/chunks/*.md`. Each is one row in
   `spec_chunks`. SHA-256 staleness check against existing
   rows; skip unchanged.
4. Walk every signal-table TOML under
   `<project>/.sim-flow/spec-ingest/**/tables/signals/*.toml`.
   Emit one `signal_table_rows` row per `[[rows]]` entry,
   with `source_kind = "source-spec"`.
5. If `<project>/docs/spec.md` exists and parses cleanly:
   - Parse via the spec.md parser (Chapter 2).
   - Emit `signal_table_rows` rows from every Block's signal
     table, with `source_kind = "spec-md"`.
   - Record `spec_md_sha256` in the index manifest.
6. Walk every `references.toml` under
   `<project>/.sim-flow/spec-ingest/**/references.toml`. Emit
   one `cross_spec_refs` row per `[[references]]` entry.
7. Atomic-rename each `*.lance/` directory into place.
8. Write the updated `manifest.toml` and `embedder.toml`.

### 3.9.3 Combined "ingest + index" convenience

```
sim-flow refresh-spec [--project <root>]
```

Equivalent to running `sim-flow ingest --rebuild` followed by
`sim-flow build-spec-index`. The orchestrator's DM0 step calls
this programmatically when it detects a stale source-spec
hash.

## 3.10 Staleness Detection and Partial Refresh

Three staleness signals:

1. **Source spec changed**: `spec_ingest/manifest.toml`'s
   `source_sha256` differs from the lance index manifest's
   `spec_ingest_source_sha256`. Triggers a full re-ingest +
   re-index. Detected by `sim-flow build-spec-index --check`.
2. **spec.md changed**: SHA-256 of `docs/spec.md` differs
   from `spec_md_sha256`. Triggers a partial rebuild that
   replaces only the `signal_table_rows` rows with
   `source_kind = "spec-md"`.
3. **Embedder config changed**: `embedder.toml` provider /
   model / dimension differs from the index manifest's
   `embedder.toml`. Triggers a full re-embed (with `--force`).

Partial refresh paths exist for case 2 only. Case 1 conceptually
allows partial (only re-embed chunks whose `text_sha256`
changed), and the build pipeline implements this incrementally
by default — but the overall corpus structure (chunk IDs,
breadcrumbs) may shift on re-ingest, so the safe default is
full rebuild for case 1.

## 3.11 Query Surface

This chapter defines the storage; Chapter 4 defines the tools.
For completeness, the read-side operations Chapter 3 exposes
to Chapter 4's tools are:

- **Vector search with optional scalar filter** on
  `framework_chunks` and `spec_chunks`. Returns top-K rows
  ordered by L2 distance to the query vector.
- **Scalar query** on `signal_table_rows` and `cross_spec_refs`
  (filter by signal_name / stage / peer / direction / etc.).
- **Hybrid query**: scalar filter + vector top-K on
  `framework_chunks` and `spec_chunks` (e.g. "search `kind =
  'src-fn'` rows by vector similarity to query Q").
- **Foreign-key lookups**: resolve a `chunk_id` to its
  containing `spec_chunks` row; resolve a `peer_id` to its
  `cross_spec_refs` rows.

All read operations are async (Lance Rust API is async-only).
Chapter 4 specifies the sync-orchestrator bridge.

## 3.12 Concurrency and Locking

Lance handles concurrent reads natively. Writes are
single-writer; the build operations (§3.9) hold an exclusive
file lock on the dataset directory for the duration of the
rebuild. Lock file: `<dataset>.lock` next to the lance
directory. Stale lock cleanup: a lock older than 10 minutes is
removed on next build attempt.

The orchestrator's runtime querying never holds writer locks.
Build operations are invoked via CLI or from DM0; concurrent
build attempts serialize via the lock.

## 3.13 Versioning and Compatibility

- **Lance dataset format**: Lance carries its own forward/
  backward compatibility guarantees. sim-flow targets Lance
  format version 2.0 (current stable). Migration to format 3.x
  if/when it lands is an implementation-plan concern.
- **sim-flow schema_version**: every TOML manifest and every
  Lance table carries a `schema_version` integer. Schema
  evolution is by incrementing the integer; downstream readers
  check it and refuse to read newer versions.
- **Embedder migration**: changing the embedder is a full
  rebuild (see §3.10 case 3). There is no "convert old vectors
  to new" path; vectors are tied to the embedder.

## 3.14 Backups and Disposability

The lance index is **disposable**: any project's lance tree
can be deleted and rebuilt from the spec-ingest output plus
spec.md plus the embedder. The shared framework index is
likewise rebuildable from `fw:api/` + `fw:src/`. No critical
state lives only in lance; lance is a cache.

Backups are unnecessary. CI environments can build the index
fresh on each run; the cost is dominated by embedding API
calls, not lance writes.

## 3.15 What This Chapter Does Not Specify

- The exact lance Rust API calls. Implementation concern;
  contract is the schemas + operations specified here.
- The embedding model choice (Ollama nomic-embed-text vs vLLM
  bge-m3 vs voyage-3 etc.). Operational concern; Chapter 5
  specifies the rig provider interface that consumes whichever
  is configured.
- Performance tuning (batch sizes, IVF nlist / nprobe
  parameters, HNSW ef_construction). Implementation /
  operations concern. Defaults that work at our scale are
  acceptable for v1.
- Multi-user / multi-process semantics beyond the
  single-writer file lock. Out of scope.
- A web / RPC API for the index. Out of scope; lance is opened
  in-process by the orchestrator only.
