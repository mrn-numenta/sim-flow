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

- [ ] Create `sim-foundation/templates/` as the canonical template root.
- [ ] Document the expected on-disk layout: `templates/model-project/`,
  `templates/study-project/`, `templates/candidate-project/` (latter two
  empty stubs with READMEs pointing to Phase 5).
- [ ] Implement template root resolution via walk-up, `SIM_FOUNDATION_ROOT`,
  and `--foundation-root` (already added in Phase 1) -- extend to surface
  a clear error when templates are not found.
- [ ] Decide and document how `sim-flow` is delivered to users (global
  `cargo install`, checked-out source build, or both) and how it finds
  sim-foundation templates in each case.

## Milestone 2 - model-project Template Contents

- [ ] Author `templates/model-project/` with the directory structure in
  [05-templates.md](../../architecture/ai-flow/05-templates.md#template-model-project).
- [ ] Include a `Cargo.toml` template with `foundation-framework`
  dependency and commented-out path deps for `sim-models/library/` crates.
- [ ] Include `src/lib.rs`, `src/main.rs`, `src/sim.rs`,
  `src/model/mod.rs`, `src/model/top.rs` starter files with minimal
  working content (elaboration-only harness).
- [ ] Include `tests/elaboration.rs` smoke test.
- [ ] Include `CLAUDE.md` and `AGENTS.md` with equivalent content.
- [ ] Include `.claude/settings.json`, `.github/copilot-instructions.md`.
- [ ] Include `.sim-flow/state.toml` initialized at DM0, and an empty
  `.sim-flow/config.toml` with the defaults from doc 02.
- [ ] Include `.gitignore` covering `target/`, `.obsv` binaries, SQLite
  transients, and `.claude/scratchpad/*`.
- [ ] Add a `cargo-generate.toml` with template placeholders
  (`project-name`, `top_module_name`).

## Milestone 3 - Model Binary --run-id Contract

- [ ] Define the `--run-id <id>` CLI flag contract for model binaries
  generated from the template. Document in the template's `CLAUDE.md` /
  `AGENTS.md` as a framework invariant.
- [ ] Implement a thin runtime helper in `foundation-framework` that
  accepts the run id and threads it into `RunManifest::new` and
  `ObservabilityRunWriter::new`.
- [ ] Update the template's `src/main.rs` to parse `--run-id` and call
  the helper.
- [ ] Add a test that runs the generated binary with `--run-id test-001`
  and verifies the produced `run.obsv.manifest.json` carries that id.

## Milestone 4 - sim-flow new model Subcommand

- [ ] Implement `sim-flow new model <name>`:
  - resolve username from git config or `.sim-flow/user`
  - locate the `model-project` template
  - invoke `cargo generate` with the right destination
    (`users/<username>/models/<name>/`)
  - compute the relative `library_path` placeholder
- [ ] Implement post-generation init:
  - initialize `.sim-flow/experiments.db` with the Phase 4 schema
    (or an empty-schema placeholder if Phase 4 has not landed yet)
  - set the `started` timestamp in `state.toml`
  - run `cargo build` to validate the generated project
  - print the next-action hint (`sim-flow run DM0`)
- [ ] Add an integration test that runs `sim-flow new model` in a
  tempdir and asserts the resulting project builds.

## Milestone 5 - Multi-Client AI Configuration

- [ ] Verify `CLAUDE.md` / `AGENTS.md` content equivalence with a
  side-by-side diff check enforced in CI (lint script).
- [ ] Document in the template that editing one requires editing the
  other, and add a comment at the top of each file to that effect.
- [ ] Ensure `.claude/settings.json` grants the tool set listed in
  `config.toml` so interactive Claude Code sessions also work.

## Milestone 6 - Template Validation In CI

- [ ] Add a CI job that runs `sim-flow new model smoke-model` end-to-end
  (generate, build, test) on every PR that touches the template or the
  orchestrator.
- [ ] Add a regression test that re-generates the template into a golden
  directory and fails if anything unexpected changes.

## Status

Not started.
