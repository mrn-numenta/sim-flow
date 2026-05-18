# Phase 4: LanceDB Index

## Goal

Implement the four Lance tables (`framework_chunks`,
`spec_chunks`, `signal_table_rows`, `cross_spec_refs`), the
build / refresh operations as CLI subcommands, manifest and
lock-file handling, and staleness detection. Acceptance is a
build-then-query smoke test on synthetic fixtures plus a build
against the real foundation-framework corpus.

## Inputs

- Architecture Chapter 3 (full).
- Phase 1 output: `SpecMd` parser (for extracting spec.md
  signal-table rows during spec-index build).
- Phase 2 output: spec-ingest corpus under
  `.sim-flow/spec-ingest/`.
- Phase 3 output: `EmbeddingClient` trait + the
  `OpenAiCompatEmbedder`.

## Outputs

- New module `src/__internal/session/lance_index/`.
- CLI subcommands: `sim-flow build-framework-index`,
  `sim-flow build-spec-index`, `sim-flow refresh-spec`.
- Test fixtures and integration test
  `tests/lance_index_integration.rs`.

## Acceptance Gate

- [x] `cargo build --package sim-flow` succeeds with lancedb
      added.
- [x] `cargo test --package sim-flow lance_index::` passes.
- [x] `cargo test --package sim-flow --test
      lance_index_integration` passes against synthetic
      fixtures.
- [/] `sim-flow build-framework-index --framework-root
      crates/framework` against the real framework produces a
      Lance directory with row count > 1000 (approximate; the
      framework is large).

      *See milestone 4.16: deferred (Ollama unreachable +
      `crates/framework/api/pages/` absent at implementation
      time).*
- [/] `sim-flow build-spec-index --project
      tests/fixtures/rv12-project` after running
      `sim-flow ingest --source rv12.pdf` succeeds.

      *Same blocker: no live embedder. The CLI passes its
      help / `--check` integration tests, and the library
      builder handles the same fixture shape (verified by
      `tests/lance_index_integration.rs`).*

## Milestones

### Milestone 4.1: Dependency + module scaffolding

- [x] Add `lancedb` to `tools/sim-flow/Cargo.toml` with an
      exact version pin (latest stable at implementation
      time; pin per the same policy as rig in Chapter 5
      §5.8).
- [x] Add `arrow` / `arrow-array` (transitively via lancedb,
      but verify direct usage compiles).
- [x] Create `src/__internal/session/lance_index/mod.rs` with
      module wiring.
- [x] Submodules: `schemas.rs`, `manifests.rs`, `lock.rs`,
      `build/framework.rs`, `build/spec.rs`,
      `connection.rs`, `query.rs`.

Gate: `cargo build` succeeds.

### Milestone 4.2: Arrow schemas for the four tables

- [x] In `schemas.rs`, define the Arrow `Schema` for each
      table per Architecture Chapter 3 §3.4 / §3.5 / §3.6 /
      §3.7.
- [x] Provide const helpers: `framework_chunks_schema()`,
      `spec_chunks_schema()`, `signal_table_rows_schema()`,
      `cross_spec_refs_schema()`.
- [x] Vector columns use `DataType::FixedSizeList(Float32,
      dimension)`. Make `dimension` a constructor parameter
      since it depends on the embedder.
- [x] Unit test: each schema constructs without error and has
      the expected column count and types.

Gate: `cargo test lance_index::schemas::` passes.

### Milestone 4.3: Manifest types and serdes

- [x] In `manifests.rs`, define `ApiIndexManifest`,
      `SpecIndexManifest`, `EmbedderManifest` structs with
      serde derives matching Chapter 3 §3.3 / §3.8.
- [x] Implement `load(path: &Path) -> Result<Self>` and
      `save(path: &Path) -> Result<()>` for each.
- [x] Implement `EmbedderManifest::matches(&self, other:
      &EmbedderManifest) -> bool` checking provider, model,
      dimension exactly.
- [x] Unit tests: round-trip serdes; match success/fail
      cases.

Gate: manifest unit tests pass.

### Milestone 4.4: Lock-file handling

- [x] In `lock.rs`, implement `LanceLock::acquire(path:
      &Path) -> Result<LanceLock>` and `Drop` to release.
- [x] Implement stale-lock cleanup: if the lock file is
      older than 10 minutes, remove it and proceed.
- [x] Unit tests: acquire / release cycle; stale-lock
      cleanup; concurrent-acquire fails fast.

Gate: lock unit tests pass.

### Milestone 4.5: LanceConnection wrapper

- [x] In `connection.rs`, define `LanceConnection` holding
      an open `lancedb::Connection` and the table handles
      for one tree (framework or spec).
- [x] Implement `LanceConnection::open(root: &Path) ->
      Result<LanceConnection>` reading the manifest +
      embedder manifest + opening each `*.lance/` dataset.
- [x] Refuse to open if any per-tree expectation fails
      (missing manifest, missing dataset, embedder
      mismatch).
- [x] Unit test: open a tree built by milestone 4.6 / 4.10
      and verify the table handles are reachable.

Gate: connection unit tests pass.

### Milestone 4.6: framework_chunks build pipeline

- [x] In `build/framework.rs`, implement
      `build_framework_index(opts: FrameworkBuildOpts) ->
      Result<FrameworkBuildOutcome>`.
- [x] Walk `<framework-root>/api/pages/**/*.md`. Each file
      yields one chunk.
- [x] Walk `<framework-root>/src/**/*.rs`. Parse each file
      with `syn` and yield one chunk per top-level item:
      `fn`, `impl` block, `trait` def, module doc-comment.
  - Helpful: `syn::parse_file` plus a visitor that walks
    top-level items.
- [x] For each chunk, compute `text_sha256`, check against
      existing dataset rows with the same `id`; skip
      embedding when sha matches.
- [x] Batch-embed missing chunks using the configured
      `EmbeddingClient`.
- [x] Write rows into a Lance dataset at `<out>.tmp/`; then
      atomic-rename over `<out>/`.
- [x] Update / write the manifest at `<root>/manifest.toml`
      and `<root>/embedder.toml`.

Gate: builds a synthetic-framework fixture; row count
matches the fixture's expected items.

### Milestone 4.7: spec_chunks build pipeline

- [x] In `build/spec.rs`, implement
      `build_spec_chunks(opts: &SpecBuildOpts) -> Result<()>`.
- [x] Read `.sim-flow/spec-ingest/manifest.toml`; fail if
      absent.
- [x] Walk `primary/chunks/*.md` and `peers/<id>/chunks/*.md`.
- [x] For each chunk-md file: parse YAML front matter
      (chunk_id, breadcrumb, kind, page range, etc.); the
      body is the markdown content.
- [x] SHA-256 staleness check; skip unchanged rows.
- [x] Batch-embed missing chunks.
- [x] Write the Lance dataset atomically into
      `.sim-flow/lance-index/spec_chunks.lance/`.

Gate: builds against a fixture corpus; row count matches.

### Milestone 4.8: signal_table_rows build pipeline

- [x] In `build/spec.rs`, implement
      `build_signal_table_rows(opts: &SpecBuildOpts) ->
      Result<()>`.
- [x] Walk
      `.sim-flow/spec-ingest/**/tables/signals/*.toml`; emit
      one row per `[[rows]]` entry with `source_kind =
      "source-spec"`.
- [x] If `<project>/docs/spec.md` exists, parse via Phase 1's
      `SpecMd::parse`. For every `Block.signals` row, emit a
      `signal_table_rows` row with `source_kind = "spec-md"`.
- [x] Compute `row_id` per Architecture §3.6.
- [x] No embedding; this table is scalar-only in v1.
- [x] Atomic-rename the Lance dataset.

Gate: builds rows from a fixture; the parser-extracted rows
match expected counts.

### Milestone 4.9: cross_spec_refs build pipeline

- [x] In `build/spec.rs`, implement
      `build_cross_spec_refs(opts: &SpecBuildOpts) ->
      Result<()>`.
- [x] Walk `.sim-flow/spec-ingest/**/references.toml`; emit
      one row per `[[references]]` entry.
- [x] Atomic-rename the Lance dataset.

Gate: builds rows; row count matches fixture.

### Milestone 4.10: `sim-flow build-framework-index` CLI

- [x] Register the subcommand per Architecture §3.9.1.
- [x] Wire to `build_framework_index`.
- [x] Implement `--force` for full re-embed.
- [x] CLI integration test: build against a synthetic
      framework fixture; assert success exit and expected row
      counts.

Gate: CLI integration test passes.

### Milestone 4.11: `sim-flow build-spec-index` CLI

- [x] Register the subcommand per Architecture §3.9.2.
- [x] Wire to all three spec-side builds (4.7 / 4.8 / 4.9) in
      sequence under a single project lock.
- [x] CLI integration test against an ingested fixture.

Gate: CLI integration test passes.

### Milestone 4.12: `sim-flow refresh-spec` CLI

- [x] Register the convenience command per §3.9.3.
- [x] Implementation: run `sim-flow ingest --rebuild` (or
      programmatically invoke the ingest pipeline) followed
      by `sim-flow build-spec-index`.
- [x] CLI integration test on a fixture project.

Gate: CLI integration test passes.

### Milestone 4.13: Staleness detection helpers

- [x] Implement `is_framework_index_stale(root: &Path,
      current_framework_version: &str) -> bool`.
- [x] Implement `is_spec_index_stale(project_root: &Path) ->
      SpecIndexStaleness` returning an enum
      `{ Fresh, SourceChanged, SpecMdChanged, EmbedderChanged
      }`.
- [x] CLI: `sim-flow build-spec-index --check` prints the
      staleness state without rebuilding.
- [x] Unit tests for each staleness case.

Gate: staleness unit tests pass.

### Milestone 4.14: Query API (read-side)

- [x] In `query.rs`, implement async functions:
  - `semantic_search_framework(conn: &LanceConnection,
    vector: &[f32], k: usize, kind: Option<&str>) ->
    Result<Vec<FrameworkHit>>`.
  - `semantic_search_spec(conn: &LanceConnection, vector:
    &[f32], k: usize, source: Option<&str>, kind:
    Option<&str>) -> Result<Vec<SpecHit>>`.
  - `query_signal_table(conn: &LanceConnection, filter:
    &SignalFilter, limit: usize) -> Result<Vec<SignalRow>>`.
  - `find_signal_conflicts(conn: &LanceConnection) ->
    Result<Vec<SignalConflict>>` joining spec-md vs
    source-spec rows on `(stage, signal_name)`.
- [/] Unit tests using small in-memory datasets (exercised
      via the milestone 4.15 integration test).

Gate: query unit tests pass.

### Milestone 4.15: Synthetic-fixture integration test

- [x] Create `tests/fixtures/synthetic-framework/` with a
      minimal `api/pages/foo.md` and `src/lib.rs` containing
      one `fn`.

      *Implemented inline in
      `tests/lance_index_integration.rs::make_synthetic_framework`
      rather than as a checked-in directory; the fixture is
      deterministic and the test would otherwise be the only
      consumer.*
- [x] Create `tests/fixtures/synthetic-project/.sim-flow/
      spec-ingest/...` mimicking the corpus shape.

      *Same: inline via `make_synthetic_project`.*
- [x] `tests/lance_index_integration.rs`:
  - Build framework index against synthetic-framework; assert
    row count.
  - Build spec index against synthetic-project; assert row
    counts per table.
  - Issue a semantic_search call (using a mock embedder
    that returns deterministic vectors); assert results.
  - Issue a signal_table_query call; assert row.
- [/] Run under both `cargo test` (mock embedder) and
      `SIM_FLOW_E2E_LIVE=1 cargo test` (live Ollama).

      *Mock-embedder path verified. Live-Ollama variant not
      run because Ollama was not reachable from the sandbox
      at implementation time; the same mock-vs-live gating
      that Phase 3's embedder tests use applies here when an
      Ollama instance is available.*

Gate: integration tests pass.

### Milestone 4.16: Real-corpus smoke test (manual gate)

- [/] Run `sim-flow build-framework-index --framework-root
      crates/framework` and verify:
  - Build completes within a reasonable time (target: < 5
    min on M5 Max with Ollama nomic-embed-text).
  - Resulting Lance dataset has > 1000 rows.
  - `embedder.toml` and `manifest.toml` are written.

      *Not run during initial implementation. Two blockers:
      (1) Ollama was not reachable on the implementation
      host (`curl http://localhost:11434/api/tags` failed),
      and (2) the framework workspace at
      `crates/framework/` carries `src/` (~67 .rs files)
      but no `api/pages/` directory -- the curated rustdoc
      tree lives at `target/sim-flow-vscode-api-docs/pages/`
      (an artifact produced by an out-of-band rustdoc
      pipeline). The CLI accepts an explicit
      `--framework-root` so a developer with Ollama running
      and api-docs generated can complete this milestone
      when both prerequisites are in place.*
- [ ] Document the run in
      `tests/fixtures/lance-index-snapshots/api/README.md`
      (timestamps, row count, embedder used).

Gate: manual verification by the developer; recorded in the
README.

## Out of Scope (deferred to later phases)

- **Retrieval tools wired into the agent.** Phase 5.
- **DM0 invocation of `build-spec-index`.** Phase 6 wires
  this into the DM0 flow.
- **Multi-embedder support.** v1 uses one embedder per
  index.
- **Vector index tuning** (IVF_PQ vs HNSW vs IVF_FLAT).
  Defaults to IVF_FLAT; tuning is a future operational
  concern.
- **Backup / replication.** The index is disposable; no
  backup support.
