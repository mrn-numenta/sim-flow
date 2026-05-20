# Implementation Plan: Spec Ingest, Structured spec.md, and LanceDB Retrieval

This plan implements the architecture specified in
[../architecture/architecture.md](../architecture/architecture.md)
(Chapters 1-6). It is structured for execution by an LLM agent
working through milestones one at a time, with concrete file
paths, function names, and gate criteria per milestone.

## Overview

The architecture defines six interlocking components:

1. A rewritten spec-ingest pipeline.
2. A structured spec.md schema with parser / writer.
3. A LanceDB index over framework chunks, spec chunks, and
   signal-table rows.
4. Four new agent tools wired into the universal tool catalog
   — three retrieval tools and one user-interaction tool
   (`ask_user`) with turn-boundary scheduling and an
   auto→manual step-mode flip.
5. Rig as an embedding-client only.
6. DMF flow integration: DM0 authoring loop (built on
   `ask_user`), DM1+ tool usage, gate-check and critique
   changes.

The plan delivers these in eight phases ordered by dependency.
Each phase is self-contained in a per-phase file with
milestones and tasks. A phase is "done" when its acceptance
gate passes; the gate is specific to that phase and recorded
in the phase file.

The plan deliberately favors small, gate-driven milestones over
large open-ended ones. The robustness study tells us LLM
implementers stall when milestones are too broad; each
milestone here has at most a few concrete tasks plus a
verifiable gate (compile, test passes, command runs cleanly,
round-trip identity).

## Dependency Order

```
Phase 1: Schema + Parser           (foundation)
  |
  +-------> Phase 2: Spec Ingest Pipeline
  |              (consumes parser for spec.md re-emit;
  |               otherwise independent)
  |
  +-------> Phase 6: DMF -- DM0
  |              (needs parser + ingest)
  |
Phase 3: Embedder Abstraction       (independent; rig dep)
  |
  +-------> Phase 4: LanceDB Index
                 (needs embedder)
                 |
                 +-------> Phase 5: Agent Tools
                                |   (3 retrieval tools + ask_user)
                                |
                                +-------> Phase 7: DMF -- DM1+
                                                  (needs tools)

Phase 8: Migration + End-to-End Validation
   (needs everything above)
```

Phases 1, 2, 3 can be executed in parallel by separate agents.
Phases 4 follows 3; Phase 5 follows 4; Phases 6 follows 1+2;
Phase 7 follows 5+6. Phase 8 is sequencing-dependent on all
others. The plan order below reflects a single-agent execution
order; an agent that wants to parallelize can read the
dependency diagram above.

## Phases

- [ ] [01-phase-schema-and-parser.md](01-phase-schema-and-parser.md)
  - Defines Rust types for `SpecMd` and the structured-table
    sub-types; implements the markdown parser and writer; ships
    a new `spec.md.tmpl`; round-trip identity tests.
- [ ] [02-phase-spec-ingest-pipeline.md](02-phase-spec-ingest-pipeline.md)
  - Rewrites `spec_ingest.rs` as the seven-stage pipeline from
    Architecture Chapter 1; produces the on-disk corpus under
    `.sim-flow/spec-ingest/`; CLI subcommand; integration
    tests against the four sample specs.
- [ ] [03-phase-embedder.md](03-phase-embedder.md)
  - `EmbeddingClient` trait; `OpenAiCompatEmbedder` wrapping
    rig's openai-compat provider; `embedder.toml` loader;
    `sim-flow embedder check` CLI; smoke tests against Ollama.
- [ ] [04-phase-lancedb-index.md](04-phase-lancedb-index.md)
  - Four Lance tables; build / refresh CLI subcommands;
    manifest + lock handling; staleness detection.
- [ ] [05-phase-agent-tools.md](05-phase-agent-tools.md)
  - `RetrievalService` sync/async bridge; the three retrieval
    tools (`api_semantic_search`, `spec_semantic_search`,
    `signal_table_query`); the `ask_user` tool with
    turn-boundary scheduling and auto→manual step-mode flip;
    tool registration; native function-call schemas;
    observability metrics.
- [/] [06-phase-dmf-dm0.md](06-phase-dmf-dm0.md)
  - DM0 auto-populate logic (source-driven mode); Q&A loop
    driver (no-source mode); required-field traversal; DM0
    prompt rewrite; gate-check changes. 13 of 14 milestones
    complete; only the live RV12 outcome snapshot (6.14) is
    outstanding (operational verification, not code work).
- [ ] [07-phase-dmf-dm1-plus.md](07-phase-dmf-dm1-plus.md)
  - DM1 / DM2a / DM2b / DM2c / DM2d / DM3a / DM3b prompt
    updates; tool-usage nudges; DM2d signal-table-consistency
    diagnostic; gate-check changes for DM2*; observability
    metrics.
- [ ] [08-phase-migration-and-validation.md](08-phase-migration-and-validation.md)
  - Migration tool for existing projects' spec.md; end-to-end
    smoke against rgb_toy; replay validation; documentation
    updates.
- [/] [09-phase-format-discovery.md](09-phase-format-discovery.md)
  - Structured-spans pipeline driven by `pdf_oxide`'s structured
    API + a `format.json` semantic descriptor; spec_md schema
    extensions (CSRs, glossary, layer/role/domain tags, PMU
    events); DM0 auto-populate consumes `format.json` directly.
    Code-complete: 16 of 16 implementation milestones done; only
    the live RV12 outcome snapshot (9.16) remains as operational
    verification.

## Conventions for Phase Files

Each phase file follows the same shape:

- **Goal**: one paragraph stating what "done" looks like.
- **Inputs**: what artifacts the phase consumes (previous-
  phase outputs, architecture chapter sections).
- **Outputs**: what artifacts the phase produces.
- **Acceptance gate**: the specific check that signals "phase
  complete" (e.g. "all milestones marked [x] and `cargo test
  --package sim-flow ingest::` passes").
- **Milestones**: ordered list, each with tasks and a gate.
- **Out of scope**: what's deferred to later phases or to
  follow-up work.

Tasks are sized for a single LLM work session each. Where a
task requires file changes, the phase file names the target
files. Where a task requires tests, the phase file names the
test module or fixture.

## Conventions for Task Granularity

A task is a single concrete change. Examples of well-sized
tasks:

- "Add `SignalTable` struct to `src/__internal/session/spec_md/types.rs`."
- "Write parser logic for the `## Blocks` section in `src/__internal/session/spec_md/parser/blocks.rs`."
- "Add `signal_table_query` to the universal tool catalog in `src/__internal/steps/mod.rs`."

Examples of tasks that are too large and should be split:

- "Implement the spec.md parser" (split per-section).
- "Build the Lance index" (split into schema, build operations, refresh).
- "Wire up DM0" (split: auto-populate, Q&A loop, prompt, gate).

## Conventions for Gates

Every milestone has a gate. Gates are mechanically verifiable.
Examples:

- "cargo build succeeds."
- "cargo test --package sim-flow spec_md::parser:: passes."
- "`sim-flow ingest --source <fixture>.pdf --out <tmp>` produces a valid manifest.toml."
- "Round-trip: parse `tests/fixtures/specs/<name>.md` and re-emit; byte-equality with input."
- "End-to-end: running `sim-flow auto --project rgb_toy` reaches DM2c without `work-no-artifact` cap."

Gates that are too loose:

- "It works." (no verification)
- "Tests pass." (which tests?)
- "Looks reasonable." (subjective)

## What This Plan Does NOT Specify

- **Calendar time, deadlines, person-days.** This is an LLM-
  execution plan; the cost dimensions are wall-clock embed +
  cargo-test cycles, not human time.
- **Implementation language details** (which crate version,
  exact macro choices). Implementer chooses within the
  constraints stated in the architecture.
- **Code review process.** Each milestone's gate is the
  reviewable artifact. Review of generated code is the human's
  responsibility outside this plan.
- **Rollback strategy.** Phases are independent enough that a
  failed phase can be reverted in isolation. The repo's git
  history is the rollback mechanism.

## What "Done" Means for the Plan as a Whole

All eight phases marked [x]. Phase 8's acceptance gate is:

- The rgb_toy DM2d replay (Phase 8 milestone) runs cleanly
  with the new pipeline.
- The invented-API rate drops measurably on that replay
  versus a pre-implementation baseline run.
- A new project initialized via `sim-flow new model` and
  authored with no source spec via the Q&A loop produces a
  valid spec.md, advances through DM0, and reaches DM1
  without gate failures.
- All cargo tests pass; all clippy lints pass.

These are the acceptance signals the human reviewer keys on
to mark the plan complete.
