# Phase 2 - Project Templates And Model-Project Bootstrap

Phase dependency: Phase 1 (orchestrator core).

## Problem Statement

Users need a one-command way to create an AI-flow-managed project that is
pre-wired for sim-foundation, a chosen AI client, and the orchestrator's
state and experiments layout. This phase delivers the `model-project`
cargo-generate template (the DMF-only template), the `sim-flow new model`
subcommand, post-generation initialization, and the `--run-id` CLI contract
that every generated model binary must honor. The study and candidate
templates are deferred to Phase 5.

## Milestone 1 - Template Layout And Resolution

- [x] Create `sim-foundation/templates/` as the canonical template root.
- [x] Document the expected on-disk layout: `templates/model-project/`,
  `templates/study-project/`, `templates/candidate-project/` (latter two
  empty stubs with READMEs pointing to Phase 5).
- [x] Implement template root resolution via walk-up, `SIM_FOUNDATION_ROOT`,
  and `--foundation-root` (already added in Phase 1) -- extend to surface
  a clear error when templates are not found.
- [/] Decide and document how `sim-flow` is delivered to users (global
  `cargo install`, checked-out source build, or both) and how it finds
  sim-foundation templates in each case. For v1, `sim-flow` runs from the
  local sim-foundation checkout; full distribution story is a Phase 7
  documentation task.

## Milestone 2 - model-project Template Contents

- [x] Author `templates/model-project/` with the directory structure in
  [05-templates.md](../../architecture/ai-flow/05-templates.md#template-model-project).
- [x] Include a `Cargo.toml` template with `foundation-framework`
  dependency and commented-out path deps for `sim-models/library/` crates.
- [x] Include `src/lib.rs`, `src/main.rs`, `src/sim.rs`,
  `src/model/mod.rs`, `src/model/top.rs` starter files with minimal
  working content (compiles; the real elaboration harness arrives in
  DM2c).
- [x] Include `tests/elaboration.rs` smoke test.
- [x] Include `CLAUDE.md` and `AGENTS.md` with equivalent content.
- [x] Include `.claude/settings.json`, `.github/copilot-instructions.md`.
- [x] Include `.sim-flow/state.toml` initialized at DM0, and an empty
  `.sim-flow/config.toml` with the defaults from doc 02.
- [x] Include `.gitignore` covering `target/`, `.obsv` binaries, SQLite
  transients, and `.claude/scratchpad/*`.
- [/] Add a `cargo-generate.toml` with template placeholders
  (`project-name`, `top_module_name`). Replaced with `template.toml` and
  an internal template engine (see Milestone 4) so no external
  `cargo-generate` dependency is required.

## Milestone 3 - Model Binary --run-id Contract

- [x] Define the `--run-id <id>` CLI flag contract for model binaries
  generated from the template. Documented in the template's `CLAUDE.md`
  and `AGENTS.md` as a framework invariant.
- [/] Implement a thin runtime helper in `foundation-framework` that
  accepts the run id and threads it into `RunManifest::new` and
  `ObservabilityRunWriter::new`. `RunManifest::new(run_id)` already
  exists; a dedicated helper layer is deferred to Phase 4 where tracking
  actually consumes it.
- [x] Update the template's `src/main.rs` to parse `--run-id`.
- [/] Add a test that runs the generated binary with `--run-id test-001`
  and verifies the produced `run.obsv.manifest.json` carries that id.
  Deferred until Phase 4 wires run recording to `ObservabilityRunWriter`.

## Milestone 4 - sim-flow new Subcommand

- [x] Implement `sim-flow new model <name>` with `--destination` and
  `--library-path` flags; resolves foundation root, expands the template,
  and runs `cargo check` unless `--skip-cargo-check`.
- [x] `sim-flow new study` and `sim-flow new candidate` registered as CLI
  subcommands that return a clear "not yet implemented (Phase 5)" error.
- [x] Implement an internal template expansion engine that substitutes
  `{{placeholder}}` tokens in file contents and path segments; unknown
  tokens are left intact.
- [x] Post-generation: timestamp substitution into `.sim-flow/state.toml`
  is handled by the expansion engine.
- [/] Initialize `.sim-flow/experiments.db` during post-generation. Schema
  lives in Phase 4; for Phase 2 the database is intentionally not
  created (no consumer yet).
- [x] Add an integration test that runs `sim-flow new model` in a
  tempdir and asserts the resulting project contains every expected file
  with placeholders fully resolved.

## Milestone 5 - Multi-Client AI Configuration

- [x] Verify `CLAUDE.md` / `AGENTS.md` content equivalence with a
  side-by-side comparison check exposed via
  `sim_flow::new_project::verify_client_file_equivalence`.
- [x] Document in the template that editing one requires editing the
  other (HTML comment at the top of each file).
- [x] Ensure `.claude/settings.json` grants the tool set listed in
  `config.toml` so interactive Claude Code sessions also work.

## Milestone 6 - Template Validation In CI

- [/] Add a CI job that runs `sim-flow new model smoke-model` end-to-end
  (generate, build, test) on every PR that touches the template or the
  orchestrator. Phase 2 ships the test as a cargo integration test; a
  dedicated CI job definition is deferred until the project's CI
  configuration lands ai-flow paths.
- [x] Add a regression test that regenerates the template into a tempdir
  and fails if any expected file is missing or any placeholder remains
  unresolved.

## Status

Complete. `sim-flow new model <name>` generates a buildable Rust project
from `templates/model-project/`. 32 unit tests and 8 integration tests
(3 new-project + 5 smoke) pass. Remaining `[/]` items are deferred to
phases where their consumers land (tracking to Phase 4, dedicated CI job
to Phase 7).
