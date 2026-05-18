# Phase 8: Migration and End-to-End Validation

## Goal

Provide a migration path for projects with `spec.md` files
written against the old template, run an end-to-end validation
of the new pipeline against rgb_toy and against a fresh
no-source project, and produce the documentation updates that
ship with the new system. Mark the implementation plan
complete after this phase's gate passes.

## Inputs

- All previous phases (1 through 7).
- Existing projects with old-template `spec.md`:
  - `sim-models/users/mneilly/rgb_toy/`
  - any others discovered.
- The Phase 7 measurement output (invented-API rate
  comparison).

## Outputs

- Migration tool / subcommand for old-template spec.md.
- End-to-end test results recorded under
  `tests/fixtures/end-to-end-snapshots/`.
- Updated user-facing documentation under `docs/` (sim-flow
  README, embedder.md, ingest.md, spec.md authoring guide).
- Final implementation status recorded.

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds.
- [ ] `cargo test --package sim-flow` passes (all
      previous-phase tests still green).
- [ ] `cargo clippy --package sim-flow -- -D warnings`
      passes.
- [ ] End-to-end: a fresh project from `sim-flow new model`
      with no source spec runs through DM0 (no-source mode)
      and reaches DM1 cleanly using only scripted user
      answers.
- [ ] End-to-end: the rv12 project's ingested corpus runs
      through DM0 → DM1 → DM2a → DM2b → DM2c → DM2d using
      the new tools. DM2d completes without `work-no-artifact`
      cap.
- [ ] rgb_toy migration: the existing rgb_toy spec.md
      migrates to the new schema via the migration tool;
      validation passes.
- [ ] Phase 7's invented-API rate measurement shows
      improvement OR a documented reason it didn't.

## Milestones

### Milestone 8.1: Migration tool scaffolding

- [ ] Create `src/__internal/session/spec_md/migrate.rs`.
- [ ] Define `migrate_old_spec_md(old: &str) ->
      Result<SpecMd, MigrationError>`.
- [ ] Wire to a CLI subcommand: `sim-flow migrate-spec
      [--project <root>] [--dry-run]`.
- [ ] Output:
  - `--dry-run`: prints a diff of old vs new and exits
    without writing.
  - default: backs up old at
    `docs/spec.md.legacy.<timestamp>` and writes the new
    spec.md.

Gate: CLI registers; scaffold compiles.

### Milestone 8.2: Old-template parser (best-effort)

- [ ] In `migrate.rs`, implement a best-effort parser for
      the OLD template structure (the prose-heavy 218-line
      template).
- [ ] Map old sections to new sections:
  - Metadata → Metadata (direct).
  - Purpose And Scope → Purpose + Scope + Non-goals.
  - Assumptions And Constraints → Assumptions and
    Constraints (quantitative table where regex-detectable).
  - External Interfaces → External Interfaces (extract
    signal tables where present).
  - Internal Interfaces → Blocks (each old "Internal
    interface" becomes a Block stub; the agent / user fills
    in details).
  - Parameters / Open Questions / Auto-decisions / Worked
    Examples → direct.
- [ ] Sections without a clean mapping become empty in the
      new spec.md; their old content is preserved as a
      comment block at the bottom for manual review.
- [ ] Unit tests on a fixture old-spec.md (use rgb_toy as
      the reference).

Gate: migration unit tests pass.

### Milestone 8.3: rgb_toy migration

- [ ] Run `sim-flow migrate-spec --project
      <rgb_toy-copy> --dry-run` against a copy of rgb_toy.
- [ ] Review the diff manually; document any missed mappings.
- [ ] Run without `--dry-run` to produce the migrated
      spec.md.
- [ ] Run the new DM0 gate-check against the migrated
      spec.md.
- [ ] If gate fails, iterate on the migration tool until
      the rgb_toy migration passes.
- [ ] Record the outcome in
      `tests/fixtures/migration-snapshots/rgb_toy.md`.

Gate: rgb_toy migrated spec.md passes the new DM0 gate.

### Milestone 8.4: End-to-end test -- fresh no-source project

- [ ] Author a test harness under
      `tests/end_to_end_no_source.rs` that:
  - Bootstraps a project via `sim-flow new model`.
  - Skips the ingest step (no source spec).
  - Invokes DM0 with a scripted-answer mock `AutoPresenter`
    that supplies plausible answers for every
    MissingField.
  - Verifies DM0 produces a passing spec.md.
  - Continues into DM1; verifies DM1 reads the structured
    spec.md and produces decomposition.md.
  - Stops after DM1 (per the acceptance gate; full DMF
    is not required for the no-source case in v1).
- [ ] Add to CI gated on `SIM_FLOW_E2E_LIVE=1` if it requires
      a live LLM, or use a mock LLM for cheap CI.

Gate: end-to-end-no-source test passes.

### Milestone 8.5: End-to-end test -- RV12 project

- [ ] Author `tests/end_to_end_rv12.rs` that:
  - Copies the RV12 PDF fixture to a tmp project.
  - Runs `sim-flow ingest`.
  - Runs `sim-flow build-spec-index`.
  - Invokes DMF (via `sim-flow auto` or the equivalent
    library API) with a real LLM (gated on
    `SIM_FLOW_E2E_LIVE=1`).
  - Captures the full event stream (using the existing
    `--capture-jsonl` flag from the robustness study).
  - Asserts DM0, DM1, DM2a, DM2b, DM2c, DM2d advance
    cleanly (no `work-no-artifact` cap, no critique-cap on
    structured sections).
- [ ] Save the captured JSONL as a regression baseline
      under `tests/fixtures/end-to-end-snapshots/rv12/`.

Gate: end-to-end-rv12 test reaches at least DM2d on the
default model + embedder configuration.

### Milestone 8.6: Invented-API rate measurement

- [ ] Take the rgb_toy DM2d failure capture from the
      robustness study (Phase 0 of that study).
- [ ] Replay it against the new orchestrator with the
      retrieval tools available (using the
      `MockAgent::from_corpus` mechanism for replayability).
- [ ] Count `api_semantic_search` calls and verify the
      agent now reaches for it before writing framework
      symbols.
- [ ] Diff the resulting code against the baseline: the
      `take_input` fabrication should not recur.
- [ ] Record the comparison in
      [`../brainstorming/model-robustness-study.md`](../brainstorming/model-robustness-study.md)
      as a new section "Phase 8 results".

Gate: measurement documented.

### Milestone 8.7: User-facing documentation

- [ ] Update `tools/sim-flow/README.md` (or create one if
      missing) with:
  - Quick-start for the new flow.
  - How to set up Ollama for the embedder.
  - How to run `sim-flow ingest` + `build-spec-index` for a
    new source-spec project.
  - How to run the no-source authoring flow.
- [ ] Write `docs/embedder.md` covering the embedder choices
      per Architecture Chapter 5.
- [ ] Write `docs/ingest.md` covering the ingest pipeline
      (what it does, what it produces, when to re-run).
- [ ] Write `docs/spec-md-authoring.md` covering the new
      spec.md structure (per-section guide for human authors
      who want to write or edit manually).
- [ ] Update any existing CLAUDE.md / OPENWOLF.md references
      that mention the old template.

Gate: documentation files exist; `cargo doc --no-deps`
succeeds without warnings on touched modules.

### Milestone 8.8: Final clippy + test sweep

- [ ] `cargo clippy --workspace --all-targets -- -D
      warnings` from `tools/sim-flow/`.
- [ ] `cargo test --workspace` from `tools/sim-flow/`.
- [ ] If any failures, fix and re-run.

Gate: clippy clean; all tests green.

### Milestone 8.9: Implementation status note

- [ ] Update [plan.md](plan.md) marking all phases [x].
- [ ] Write a short summary section at the bottom of
      plan.md recording:
  - Acceptance gate outcomes for each phase.
  - Any deferred work explicitly punted to future
    revisions.
  - The state of the invented-API rate measurement.

Gate: plan.md updated; PR can be merged.

## Out of Scope (post-v1 follow-ups)

- **Vision-model captioning of figures.** Hooks exist in
  the architecture (figure caption stubs); the captioning
  pipeline is a future phase.
- **L3 (previously-rejected pile), L4 (replay-corpus
  index), L5 (project-source index)** from the brainstorm.
- **Multi-flow support (DSF, SVF).** Architecture is built
  with these in mind but only DMF is wired in v1.
- **Per-step tool gating.** Universal catalog stays
  universal; no re-introduction of per-step gating in v1.
- **Multi-user / shared-index deployments.** Single-writer
  per project; shared framework index supports concurrent
  readers only.
