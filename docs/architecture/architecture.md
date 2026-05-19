# Architecture: Spec Ingest, Structured spec.md, and LanceDB Retrieval

This document is the architecture specification for the spec-side
improvements and lance-backed retrieval work described in the
brainstorm collection:

- [../brainstorming/lancedb-rig-incorporation-plan.md](../brainstorming/lancedb-rig-incorporation-plan.md)
- [../brainstorming/spec-ingest-figure-extraction.md](../brainstorming/spec-ingest-figure-extraction.md)
- [../brainstorming/spec-md-restructure.md](../brainstorming/spec-md-restructure.md)

It defines the components and their contracts in enough detail
that an implementation plan can be derived. It does NOT contain
calendar sequencing, task breakdowns, or code; those belong in
the implementation plan that follows.

## Overview

Today's pipeline ingests a source spec into per-page markdown,
fills a prose-heavy [`spec.md.tmpl`](../../templates/model-project/docs/spec.md.tmpl)
during DM0, and threads that spec.md plus a static 864-page
rustdoc snapshot of `foundation-framework` through DMF. Four
problems with this pipeline have been validated against real
specs (Apical NoC, Numenta SoC, Spatial Pooler 2.1, RV12):

1. **Source-spec ingestion drops critical content.** Tables get
   sliced across page boundaries. Composite figures (raster
   shape-sheet + vector text overlay) extract as shapes only —
   the entire RV12 IF next-PC mux and EX bypass-network topology
   are lost. Heading-only stub sections in skeleton specs become
   invisible. Cross-spec inheritance (SP→TM) is unsupported.
2. **spec.md is prose-heavy where it should be structured.**
   Every hardware-spec carries the same structured forms — per-
   block signal tables, FSMs, encodings, address maps, error
   tables, parameter tables — and the current template flattens
   them all to prose. Downstream steps re-parse the prose every
   time.
3. **Framework discovery is name-anchored.** The live LSP tools
   (`api_search` / `api_hover` / `api_impls` / ...) answer "I
   know the name, tell me the truth" but not "I don't know the
   name, find me candidates." The DM2d invented-API failure
   (rgb_toy fabricating `take_input`) is the canonical
   consequence.
4. **Spec.md and source spec duplicate prose.** With no
   retrieval over the source spec, DM0 must copy everything into
   spec.md; with no structured spec.md, DM1+ must re-read prose
   to extract any structured fact.

This architecture addresses all four with a coherent set of
components:

- A **rewritten spec ingest pipeline** that produces
  section-anchored chunks (not page-anchored), structured
  tables (signal tables, parameter tables, etc.), faithful
  figure rasters (via page-region rendering, not embedded-image
  extraction), and explicit stub / TBD / cross-spec metadata.
- A **structured spec.md template** that carries the typed
  artifacts — block subsections with signal tables, FSM tables,
  encoding tables, error tables, parameter tables, figure
  entries, source-spec anchors — replacing prose duplication of
  the source.
- A **LanceDB index** (embedded, single library dependency) with
  three logical tables: framework chunks for L1 (semantic search
  over `fw:api` + `fw:src`), spec chunks for L2 (semantic
  search over the source spec — primary plus any inherited
  peers), and signal-table rows for L7 (structured query over
  extracted signal-table content).
- A **rig integration** strictly as an embedding client. Rig is
  used to call provider embedding endpoints; sim-flow never
  adopts rig's Agent abstraction, never routes completion
  requests through rig, and the orchestrator's main loop stays
  synchronous. Async leakage is bounded to the embed call and
  the lance query path.
- **Four new agent tools** wired into the universal tool
  catalog alongside the existing `api_*` LSP tools. Three are
  retrieval tools — `api_semantic_search` for L1,
  `spec_semantic_search` for L2, `signal_table_query` for L7.
  The fourth is `ask_user`, a turn-boundary user-interaction
  tool the agent invokes when a TBD or design choice blocks
  forward progress: the orchestrator surfaces the question to
  the user via the chat panel, packages the reply as the tool
  result, and flips automated step-mode to manual when invoked
  during an automated run (preserving the "automated = no
  human in the loop" policy by exiting that mode the moment
  one is needed).
- **DMF flow integration** that surfaces the new artifacts to
  DM0 (authoring spec.md against the structured template), DM1
  / DM2 / DM3 (consuming structured artifacts and using the new
  tools), and the gate / critique layer (validating structured
  artifacts as foreign-key-checked TOML rather than regexing
  prose).

The state machine, gate model, protocol-event wire format, and
subprocess CLI clients (Claude Code, codex, copilot) are
unchanged. This work extends the orchestrator's data inputs and
tool catalog; it does not change how the orchestrator
orchestrates.

## Core Axioms

The chapters that follow rest on these axioms. Any deviation
must be argued explicitly in the chapter that deviates.

1. **The orchestrator core does not change.** State machine,
   gate engine, step ordering, protocol-event wire format,
   per-step write scoping, and subprocess CLI client support
   remain unmodified. New components are additive.
2. **Spec ingest is rewritten, not patched.** The current
   `spec_ingest.rs` produces per-page markdown + extracted
   embedded images. The new pipeline produces section-anchored
   chunks + page-rendered figures + structured-table extracts.
   The output schema is different; downstream consumers
   change with it.
3. **spec.md is the structured normalization of the source
   spec, not a copy.** Prose duplicating the source is
   redundant once L2 retrieval exists. spec.md carries
   structured tables, anchors back to source-spec sections,
   and the cross-cutting concerns that don't have a natural
   home in the source.
4. **LanceDB is embedded, single-process, file-backed.** No
   server, no separate daemon. The orchestrator opens the
   lance directory in-process. Index builds are explicit
   operations (CLI subcommand or DM0 step), not implicit at
   query time.
5. **Rig is an embedding client only.** Provider-level access
   to embedding endpoints; nothing else. Completion requests,
   tool dispatch, prompt caching, and extended-thinking
   controls continue to route through sim-flow's existing
   adapters. The agent layer in rig is not used.
6. **Embedders are versioned.** Every index records the
   embedder identity (provider + model + dimension) in a
   manifest. Queries refuse to run against an index built with
   a different embedder. Re-indexing on embedder change is an
   explicit operation.
7. **Page-region rendering replaces embedded-image extraction
   for figures.** The current extract-image path silently
   loses composite-figure content; the new path renders the
   PDF page region as the agent / human reader would see it.
   Figure captioning (turning a faithful raster into
   structured text) is a separate concern with hooks but no
   v1 implementation.
8. **Universal tool catalog stays universal.** The three new
   tools (`api_semantic_search`, `spec_semantic_search`,
   `signal_table_query`) are advertised on every step. The
   prior decision to remove per-step tool gating stands.
9. **Subprocess CLI agents must continue to work.** Claude
   Code, codex, and copilot subprocess paths see no changes
   to their input. New tools and structured artifacts are
   threaded through the same protocol channels they already
   consume.
10. **The bundled VS Code extension stays a thin renderer.**
    All ingestion, indexing, retrieval, and tool-dispatch
    logic lives in the sim-flow orchestrator crate. The
    extension forwards events and renders state.

## Component Map

The architecture is six interlocking components. Each chapter
specifies one component's contracts (inputs, outputs, schema,
invariants) in enough detail to drive an implementation plan.

```
                       +---------------------+
                       |  source spec PDFs   |
                       +----------+----------+
                                  |
                                  v
              +-----------+--------+-------+----------+
              | Ch. 1: Spec Ingest Pipeline           |
              |  - section chunking                   |
              |  - page-chrome stripping              |
              |  - signal-table extraction            |
              |  - figure page-rendering              |
              |  - stub / TBD / xref detection        |
              +-----------+----------------+----------+
                          |                |
                          v                v
        +-----------+-----+----+   +-------+------+
        | Ch. 2: spec.md       |   | Ch. 3: Lance |
        |   template + schema  |   |   Index      |
        |   (DM0 fills this)   |   |   (built by  |
        |                      |   |    CLI)      |
        +-----------+----------+   +-------+------+
                    |                      |
                    +----------+-----------+
                               |
                               v
                  +------------+---------+
                  | Ch. 4: Agent Tools   |
                  |   (api_semantic_     |
                  |    search,           |
                  |    spec_semantic_    |
                  |    search,           |
                  |    signal_table_     |
                  |    query,            |
                  |    ask_user)         |
                  +------------+---------+
                               |
                               v
            +------------------+----------------+
            | Ch. 5: Rig (embedding client)     |
            +------------------+----------------+
                               |
                               v
            +------------------+----------------+
            | Ch. 6: DMF Flow Integration       |
            |  - DM0 (author against template)  |
            |  - DM1+ (consume artifacts)       |
            |  - gates (validate structure)     |
            |  - critique (check artifacts)     |
            +-----------------------------------+
```

## Chapters

1. [01-spec-ingest-pipeline.md](01-spec-ingest-pipeline.md) —
   The replacement for `spec_ingest.rs`. Specifies inputs (PDF
   / markdown / text source specs), outputs (section-anchored
   chunks, structured tables, rendered figures, stub / TBD /
   cross-spec metadata), the on-disk layout under
   `.sim-flow/spec-ingest/`, and the per-stage contracts.
2. [02-spec-md-schema.md](02-spec-md-schema.md) — The
   structured spec.md template. Specifies the top-level
   section structure, the per-section table schemas (block
   signal tables, FSM tables, encoding tables, parameter
   tables, error tables, figure entries), the source-spec
   anchor format, and the validation rules.
3. [03-lancedb-index-schema.md](03-lancedb-index-schema.md) —
   The on-disk lance index. Specifies the table schemas
   (`framework_chunks`, `spec_chunks`, `signal_table_rows`,
   `cross_spec_refs`), the embedder manifest, the build /
   refresh operations, and the storage layout.
4. [04-agent-tools.md](04-agent-tools.md) — The four new
   agent tools. Three retrieval tools (`api_semantic_search`,
   `spec_semantic_search`, `signal_table_query`) and one
   user-interaction tool (`ask_user`). Specifies each tool's
   signature, argument schema, return shape, prompt nudges in
   step prompts, the bridge between sync orchestrator
   dispatch and async lance queries, and the auto→manual
   step-mode flip semantics for `ask_user`.
5. [05-rig-integration.md](05-rig-integration.md) — Rig as an
   embedding client. Specifies the embedder abstraction
   sim-flow consumes, how rig's provider clients sit behind
   it, the async-to-sync boundary, version pinning, and the
   explicit non-uses (no Agent, no completion routing, no
   transport replacement).
6. [06-dmf-flow-integration.md](06-dmf-flow-integration.md) —
   How DMF changes. Specifies DM0's new authoring loop against
   the structured template, DM1 / DM2 / DM3's consumption of
   structured artifacts and the new tools, gate-check changes
   (TOML-roundtrip and foreign-key validation in addition to
   the existing regex), and critique-pass changes that read
   structured artifacts.
7. [07-spec-format-discovery.md](07-spec-format-discovery.md) —
   The semantic-mapping pre-pass that turns a source PDF into
   `format.json`, a per-spec descriptor of section roles, table
   kinds + column maps, figure kinds, glossary entries, and
   chrome regexes. Specifies the schema, the decision policy
   (deterministic / LLM at discovery / user at DM0), the CLI
   surface, the spec_md extensions for CSRs / glossary / layer
   tags / clock / power / reset domains / security / numerical
   conventions / PMU, and how the descriptor's role tags drive
   both DM0 auto-populate and Chapter 3's lance-index chunk
   tagging.

## Reading order

Chapters are designed to be read top-to-bottom: Chapter 1's
output schema is Chapter 2 and Chapter 3's input. Chapter 4's
tools query Chapter 3's index. Chapter 5 sits behind Chapter
4's query path. Chapter 6 integrates everything into DMF.
Chapter 7 specifies the format-discovery pre-pass that runs
before Chapter 1's classify stage and produces the `format.json`
that drives chunk tagging, table classification, and DM0
auto-populate.

A reader who only cares about one component can skim the
component map above and read only that chapter — each chapter
declares its inputs and outputs at the top so cross-chapter
contracts are explicit.

## What this architecture does NOT cover

- **L3 (previously-rejected pile), L4 (replay-corpus index),
  L5 (project-source index).** Deferred or rejected in the
  brainstorm; not in this architecture.
- **Vision-model figure captioning.** The figure extraction
  produces faithful rasters; the structured caption shape is
  an open question. v1 captures the raster + a free-form
  prose caption (author-supplied or DM0-elicited); the typed
  caption schema lands in a future architecture revision.
- **Migrating existing projects.** rgb_toy and any other
  project with a spec.md written against the old template
  needs a migration path. Treated as an implementation-plan
  concern, not an architecture concern.
- **Calendar sequencing, task breakdowns, effort estimates.**
  The implementation plan that follows this architecture
  doc owns sequencing and effort.
- **New backends, new flows, new step kinds.** The DMF flow
  integration in Chapter 6 modifies the existing steps; it
  does not introduce new ones.

## Conventions

- **No emojis** in any architecture doc (per
  [architecture-format.md](architecture-format.md)).
- **Schemas use TOML** where appropriate (matches existing
  `state.toml`, `config.toml`, and the planned
  `decomposition.toml` / `pipeline-mapping.toml` from the
  prior brainstorm).
- **Schemas use serde-compatible Rust structs** where the
  artifact is consumed by Rust code; the TOML shape is the
  serialized form.
- **Identifiers are kebab-case** for filenames, paths, tool
  names, and CLI arguments; **snake_case** for TOML keys and
  Rust field names; **CamelCase** for Rust types.
- **Paths are project-relative** unless explicitly absolute.
  Source-spec paths use the existing `lib:` / `fw:` prefix
  convention.
