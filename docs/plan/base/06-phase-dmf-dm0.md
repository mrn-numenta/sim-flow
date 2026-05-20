# Phase 6: DMF Flow Integration -- DM0

## Goal

Integrate the new components into DM0: auto-populate spec.md
from the ingest corpus in source-driven mode, drive an
interactive Q&A loop in no-source mode, rewrite the DM0 work
and critique prompts, and update the DM0 gate-check to validate
against the new schema. The acceptance gate is DM0 producing a
gate-passing spec.md on both a source-spec project (rv12) and a
no-source project.

## Inputs

- Architecture Chapter 6 (sections 6.2, 6.3, 6.4, **6.5** —
  the `ask_user` integration that DM0's Q&A loop is built
  on).
- Phase 1 output: `SpecMd` parser, writer, traversal.
- Phase 2 output: spec-ingest corpus.
- Phase 5 output: agent tools (`api_semantic_search`,
  `spec_semantic_search`, `signal_table_query`, **`ask_user`
  with suspend/resume protocol and mode flip**). DM0's no-
  source Q&A loop is implemented as a thin driver on top of
  `ask_user` — it does not implement its own user-prompting
  machinery.

## Outputs

- Rewritten DM0 work prompt at
  `prompts/dm0-specification.md`.
- Rewritten DM0 critique prompt at
  `prompts/dm0-specification-critique.md`.
- New auto-populate logic under
  `src/__internal/session/dm0/auto_populate.rs`.
- New Q&A loop driver under
  `src/__internal/session/dm0/qa_loop.rs`.
- DM0 gate-check replacement under
  `src/__internal/session/dm0/gate.rs`.
- Unit + integration tests.

## Acceptance Gate

- [x] `cargo build --package sim-flow` succeeds.
- [x] `cargo test --package sim-flow dm0::` passes.
- [x] Integration: running DM0 (via `e2e_manual` or a
      synthetic driver) against an ingested RV12 fixture
      produces a spec.md that passes the new gate.
- [x] Integration: running DM0 against a no-source fixture
      with scripted Q&A answers produces a spec.md that
      passes the new gate.

## Milestones

### Milestone 6.1: DM0 module scaffolding

- [x] Create `src/__internal/session/dm0/mod.rs` and
      submodules: `auto_populate.rs`, `qa_loop.rs`,
      `gate.rs`, `prompts.rs`.
- [x] Wire from the existing DM0 step dispatch site.
- [x] Define the public entry: `run_dm0_work(opts:
      &DmOpts, host: &mut AutoPresenter, llm: &mut dyn
      LlmAdapter) -> Result<DmOutcome>`.
- [x] Stub each helper so the wiring compiles.

Gate: `cargo build` succeeds.

### Milestone 6.2: Mode detection

- [x] Read the project's `.sim-flow/spec-ingest/manifest.toml`.
- [x] Branch on `source_kind`:
  - `pdf | markdown | text` → source-driven mode.
  - `none` → interactive mode.
  - Missing manifest → emit a Diagnostic suggesting `sim-flow
    ingest` first, and either run interactive mode or fail
    depending on a flag.
- [x] Unit test: mode detection on three synthetic
      manifests.

Gate: mode-detection unit tests pass.

### Milestone 6.3: Auto-populate -- Metadata + Quantitative

- [x] In `auto_populate.rs`, implement
      `populate_metadata(manifest: &IngestManifest,
      spec: &mut SpecMd)` filling
      `SpecMd.metadata.source_documents` from
      `manifest.peers[]` plus the primary entry.
- [x] Implement
      `populate_assumptions(corpus_root: &Path, spec: &mut
      SpecMd)` reading any quantitative facts the ingest
      pipeline detected (clock frequency, technology node)
      from a designated source. v1: scan parameter tables
      and chunks for known patterns.
- [x] Unit test on a synthetic ingest corpus.

Gate: metadata + quantitative auto-populate unit test
passes.

### Milestone 6.4: Auto-populate -- Parameters, Encodings, Errors, FSMs

- [x] Implement
      `populate_parameters(corpus_root: &Path, spec: &mut
      SpecMd)` reading
      `<root>/primary/tables/parameters/*.toml` and emitting
      `SpecMd.parameters[]`.
- [x] Implement `populate_encodings`, `populate_errors`,
      `populate_fsms` analogously.
- [x] Unit tests for each.

Gate: per-section auto-populate unit tests pass.

### Milestone 6.5: Auto-populate -- Blocks (the hard one)

- [x] Implement `populate_blocks(corpus_root: &Path, spec:
      &mut SpecMd)`:
  - Read every
    `<root>/primary/tables/signals/NNN-<stage>.toml`.
  - For each, create a `Block` entry with `name = stage`,
    `parent = "(none -- top-level)"` (heuristic; the agent
    refines in the LLM-completion step), `signals =
    <rows>`.
  - Source anchors point at the source chunks the signal
    table came from.
- [x] Unit test with an RV12-like fixture (six signal tables
      → six blocks).

Gate: blocks auto-populate unit test passes.

### Milestone 6.6: Auto-populate -- Figures, Anchors, Open Questions

- [x] Implement `populate_figures(corpus_root: &Path, spec:
      &mut SpecMd)` emitting one FigureEntry per
      `figures/page-NNN.png` with source page + raster
      path; caption is left empty.
- [x] Implement `populate_anchors(spec: &mut SpecMd)`
      building the `Source-Spec Anchors` index by walking
      already-populated sections.
- [x] Implement
      `populate_open_questions_from_tbds(corpus_root: &Path,
      spec: &mut SpecMd)` reading
      `<root>/primary/tbds.toml` and turning each entry into
      an OpenQuestion with breadcrumb context.
- [x] Unit tests.

Gate: figures / anchors / TBDs auto-populate unit tests
pass.

### Milestone 6.7: Source-driven DM0 work-prompt rewrite

- [x] Replace `prompts/dm0-specification.md` with a version
      that:
  - Describes the new structured template.
  - Lists the auto-populated artifacts the agent will see on
    arrival.
  - Tells the agent which prose subsections it is
    responsible for completing (Purpose / Scope / Non-goals /
    per-Block Behavior summary / Functional Behavior prose).
  - Tells the agent which Open Questions to resolve (TBDs).
  - Includes the prompt nudges for `spec_semantic_search`
    when the agent needs source-spec detail and
    `signal_table_query` for spot-checking.
  - Includes the `ask_user` nudge (Chapter 4 §4.5
    prompt-nudge text) telling the agent to invoke
    `ask_user` for blocking TBDs / unanswered Open
    Questions, as the LAST tool call of the turn.
  - Specifies the final `write_file docs/spec.md` step.
- [x] Move the old prompt to `prompts/legacy/`.
- [x] Unit test: the new prompt parses cleanly (some prompts
      have syntactic structure verified by tests).

Gate: new prompt file exists; tests pass.

### Milestone 6.8: Q&A loop -- MissingField iteration via ask_user

The DM0 no-source Q&A loop is implemented as a thin driver on
top of the `ask_user` tool from Phase 5. It does NOT
implement its own user-prompting machinery; every question
goes through `ask_user`'s suspend/resume protocol.

- [x] In `qa_loop.rs`, implement
      `drive_qa_loop(spec: &mut SpecMd, llm: &mut dyn
      LlmAdapter) -> Result<()>`. Note: no direct
      `AutoPresenter` parameter — the loop emits `ask_user`
      tool calls and the orchestrator's existing
      `RequestUserInput` machinery surfaces them.
- [x] Loop, one thread per MissingField:
  - Compute `spec.missing_required_fields()` via Phase 1's
    traversal.
  - If empty, exit (proceed to validation + write).
  - Else pick the next MissingField and open a new
    ask_user thread (no `thread_id` on the first call):
    - `question` = the field's `prompt_template`.
    - `context` = which section is being filled, why it
      matters, plus optional source-anchor context when
      relevant.
    - `kind` mapped from `MissingFieldKind`:
      `Scalar` → `free-form`,
      `ConstrainedScalar { regex }` → `value` (regex
      validated post-reply),
      `Prose` → `free-form`,
      `TableRow { columns }` → `free-form` with the column
      schema described in `context`,
      `SectionApplicability` → `yes-no`.
    - `choices` populated when applicable.
    - `default` populated when the field has a sensible
      default.
    - **First call uses `record_as = "none"`** (persistence
      deferred to thread close).
  - Emit the tool call. The agent's LLM turn ends here
    cleanly (per Architecture §6.5.1). The orchestrator
    returns the generated `thread_id` on the answer.
  - On the next turn the agent receives the user's reply
    and validates against the field's `kind`:
    - **Valid and unambiguous**: close the thread by
      emitting a final `ask_user` call with the SAME
      `thread_id`, `record_as = "auto-decision"`, and a
      brief confirmation question (`kind = "yes-no"`,
      `default = "yes"`). Persistence happens at this
      close call.
    - **Invalid OR ambiguous**: emit a follow-up `ask_user`
      with the SAME `thread_id`, `record_as = "none"`, and
      a more focused clarification per Architecture §4.5
      chaining guidance. Loop until valid OR the turn cap
      is reached OR the user cancels the thread.
  - On thread close: commit the resolved answer to the
    in-memory `SpecMd` struct. Persist `docs/spec.md` every
    N fields (configurable; default 5).
  - On thread cancel / abandon: record a TBD entry for the
    field in `Open Questions`, advance to the next field.
- [x] Handle Worked Examples specially: open a thread
      with `kind = "free-form"` and a richer prompt asking
      the user to walk through a representative scenario.
      If the user's first answer is sparse, chain a
      clarification within the same thread (e.g. "Can you
      walk through what each pipeline stage does for the
      first instruction?"). Close the thread with one LLM
      normalization pass to convert the accumulated
      free-text into Chapter 2 §2.3.18 format and persist
      via `record_as = "auto-decision"`.
- [x] Unit tests with a mock host:
  - Single-turn-per-field path: every MissingField in a
    fixture `SpecMd::default()` is answered cleanly on
    the first reply; assert spec.md ends up populated.
  - Multi-turn-per-field path: at least one MissingField
    requires a chained clarification; assert the thread
    coalesces to one Auto-decision row and the field is
    populated correctly.
  - Cancellation path: the user cancels one thread; assert
    the field gets a TBD and the loop continues to the
    next field.

Gate: Q&A-loop-via-ask_user unit tests pass.

### Milestone 6.9: SectionApplicability via ask_user

- [x] For OPTIONAL sections (State Machines, Encodings,
      Memory Map, Connectivity, Error Handling,
      Cycle-Accurate Behavior, Figures), the loop emits an
      `ask_user` call with `kind = "choice"`, `choices =
      ["yes", "no", "skip"]`, `question = "Does this design
      have a <section>?"`. The orchestrator surfaces this
      as a quick-reply chip set in the chat panel.
- [x] On `yes`, drill into that section's MissingFields.
- [x] On `no`, mark the section as "not applicable" in the
      spec.md (a brief comment block + skip the section).
- [x] On `skip`, defer the section to a later pass; surface
      a top-level `ask_user` review at the end.
- [x] Unit test exercising all three branches.

Gate: applicability unit tests pass.

### Milestone 6.10: DM0 critique-prompt rewrite

- [x] Replace `prompts/dm0-specification-critique.md` with a
      version that:
  - Loads `docs/spec.md` and parses it via the new parser.
  - Walks the structured `SpecMd` checking semantic
    consistency (per Architecture §6.3 Step C).
  - Uses `spec_semantic_search` to surface anchors the agent
    may have missed.
  - Writes findings to `docs/critiques/DM0-critique.json` in
    the existing format.
- [x] Move the old critique prompt to `prompts/legacy/`.

Gate: critique-prompt file exists.

### Milestone 6.11: DM0 gate-check replacement

- [x] In `gate.rs`, implement
      `check_dm0_gate(spec_md_path: &Path, manifest: &Path)
      -> GateOutcome` per Architecture §6.2.4.
- [x] Steps:
  - Parse spec.md via Phase 1's parser. Hard fail on parse
    error.
  - Run `spec.validate()` (Phase 1's cross-ref check).
  - Verify `Clock frequency` regex and `Gate budget per
    cycle` regex on the quantitative table.
  - Resolve every source-anchor against the ingest
    manifest's chunk ids.
  - Check Auto-decisions populated in automated mode (read
    the orchestrator's mode flag).
- [x] Replace the old DM0 regex-based gate dispatch with the
      new function.
- [x] Unit test: synthesize a valid spec.md → gate passes;
      synthesize a spec.md missing `Clock frequency` →
      gate fails with the expected error.

Gate: gate unit tests pass.

### Milestone 6.12: Source-driven integration test

- [x] Create `tests/fixtures/rv12-project/` with the
      already-ingested RV12 corpus and an empty spec.md.
- [x] Integration test: invoke `run_dm0_work` programmatically
      with a mock LLM that produces deterministic
      assistant-text responses (mock the LLM completion
      step).
- [x] Assert that auto-populate fills the expected sections
      and that the gate passes after the mock LLM completes
      its turn.

Gate: source-driven integration test passes.

### Milestone 6.13: No-source integration test

- [x] Create `tests/fixtures/empty-project/` (a project
      bootstrapped via `sim-flow new model` with no source
      spec).
- [x] Integration test: invoke `run_dm0_work` with a mock
      `AutoPresenter` that scripts user answers for every
      MissingField.
- [x] Assert the spec.md ends up with all REQUIRED fields
      populated and passes the gate.

Gate: no-source integration test passes.

### Milestone 6.14: Live DM0 against RV12

- [ ] Run `sim-flow auto --project <rv12-tmp>` against an
      ingested RV12 corpus with a real LLM (Claude Opus 4.7
      or Qwen 3.6 via vLLM).
- [ ] Verify DM0 completes within the work-side cap and
      produces a spec.md the new gate accepts.
- [ ] Record outcome in
      `tests/fixtures/dm0-snapshots/rv12.md` (terminator
      reason, turn count, gate result, time).

Gate: manual verification; recorded.

## Out of Scope (deferred to later phases)

- **DM1 / DM2* / DM3* prompt updates.** Phase 7 owns this.
- **DM2d signal-table-consistency diagnostic.** Phase 7.
- **Migration of existing projects' spec.md.** Phase 8.
- **Critique observability** (`signal_table_conflict`
  metrics, etc.). Phase 7 wires these.
- **Tool-call enforcement gates** (e.g. "must call
  api_semantic_search before writing"). Out of scope for
  v1.
