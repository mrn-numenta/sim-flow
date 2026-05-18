# Phase 7: DMF Flow Integration -- DM1 through DM3

## Goal

Update the work and critique prompts for DM1, DM2a, DM2b, DM2c,
DM2d, DM3a, and DM3b to consume the structured `SpecMd`, advertise
and recommend the three new retrieval tools at appropriate
moments, and add the DM2d signal-table-consistency advisory
diagnostic. Update the gate-check logic for DM2 steps where
relevant. Add observability metrics for tool usage and
signal-table conflicts.

## Inputs

- Architecture Chapter 6 (sections 6.5 through 6.10).
- Phase 1 output: `SpecMd` parser.
- Phase 5 output: `RetrievalService` plus the three tools.
- Phase 6 output: DM0 produces structured spec.md.

## Outputs

- Updated prompt files under `prompts/`:
  - `dm1-...` (the analysis step prompts).
  - `dm2a-...`, `dm2b-...`, `dm2c-...`, `dm2d-...`.
  - `dm3a-...`, `dm3b-...`.
  - Their corresponding `*-critique.md` variants where relevant.
- Updated universal-tools system message.
- Gate-check augmentations for DM2 steps.
- Observability metric events.
- Integration tests.

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds.
- [ ] `cargo test --package sim-flow dm*::` passes.
- [ ] Integration: a synthetic project advancing through DM0
      → DM2c (via mock LLM responses) succeeds with the new
      prompts and tool catalog.
- [ ] On a real RV12 project, DM2d's
      `signal_table_conflict` diagnostic surfaces at least
      one entry when an intentional mismatch is introduced
      between spec.md and the source spec (manual
      verification).

## Milestones

### Milestone 7.1: Inventory existing prompts

- [ ] List all prompts under `prompts/` that correspond to
      DM1, DM2*, DM3*. Note each file's current line count
      and last-modified date.
- [ ] Record the inventory in a comment-block at the top of
      this milestone or in a tracker file
      `prompts/_dm1-plus-rewrite-tracker.md` so the rewrite
      can proceed in order.

Gate: tracker file exists; lists every target prompt.

### Milestone 7.2: Universal-tools system message update

- [ ] Locate the universal-tools system message (the standing
      prompt injected on every step; see
      `src/__internal/session/orchestrator.rs` or a related
      module for where it is composed).
- [ ] Add a one-paragraph description for each of the four
      new tools:
  - `api_semantic_search`: when to use, expected followup
    (`api_hover`).
  - `spec_semantic_search`: when to use, how `chunk_path`
    chains into `read_file`.
  - `signal_table_query`: when to use, `conflicts_only`
    semantics.
  - `ask_user`: when to use (blocking unknowns), the
    turn-boundary discipline (call it LAST in the turn),
    the auto→manual flip side effect, AND the chaining
    discipline (pass the `thread_id` from a prior call to
    clarify; use `record_as = "none"` on intermediate
    calls; only the closing call sets `record_as` to a
    persisting value). Include the explicit "do NOT use it
    for framework-symbol questions or retrievable spec
    detail" guard. Include the "close threads after ≤ 3
    rounds when possible; warning at 5" cap.
- [ ] Add a one-line "heuristic" guide: when to reach for
      semantic search vs scalar query vs LSP `api_hover` vs
      `ask_user` (and when to chain a follow-up `ask_user`
      vs accept the user's answer as final).
- [ ] Unit test (if there's an existing prompt-snapshot
      test): the system message contains the expected
      strings.

Gate: system-message update lands; tests pass.

### Milestone 7.3: DM2d work prompt update

- [ ] Edit the DM2d work prompt to add the
      `api_semantic_search` → `api_hover` discipline per
      Architecture §6.7.
- [ ] Add the `signal_table_query` nudge for I/O contracts
      per §6.7.
- [ ] Add the `ask_user` nudge per Architecture §6.7 (the
      "TBD / unresolvable ambiguity / design choice" rule;
      call as LAST tool call of the turn; do NOT use for
      framework-symbol questions or retrievable spec
      detail).
- [ ] Add the chaining nudge per Architecture §4.5: when
      the user's reply is partial / ambiguous, the agent
      passes the returned `thread_id` on a follow-up
      clarification call with `record_as = "none"`; the
      thread is closed by a final call with the resolved
      `record_as` value.
- [ ] Keep the existing milestone-walk + write-allowlist
      content intact.
- [ ] Move the old prompt to `prompts/legacy/`.

Gate: new prompt file exists.

### Milestone 7.4: DM2d signal-table-consistency diagnostic

- [ ] In the DM2d gate-check or post-work hook, invoke
      `RetrievalService::query_signal_table_sync` with
      `conflicts_only=true`.
- [ ] For each conflict, emit a `Diagnostic::Warning` with
      message describing the stage, signal_name, and
      `differs_on` fields.
- [ ] Emit a `tracing::info!` metrics event
      (`event = "signal_table_conflict"`).
- [ ] Conflicts are advisory; do NOT fail the gate.
- [ ] Unit test: synthesize a project with one deliberate
      conflict; verify the diagnostic fires once with the
      expected fields.

Gate: signal-table-consistency unit test passes.

### Milestone 7.5: DM1, DM2a, DM2b, DM2c work-prompt updates

- [ ] For each prompt, add a "Tools you should use" section
      per Architecture §6.6.
- [ ] DM1 / DM2a / DM2b emphasize `signal_table_query` and
      `spec_semantic_search`.
- [ ] DM2c additionally calls out `api_semantic_search` for
      milestones that reference framework symbols.
- [ ] Each prompt gains the `ask_user` nudge for blocking
      design decisions arising during analysis (e.g.
      decomposition choices the spec doesn't make),
      including the chaining convention (pass `thread_id`
      to clarify; `record_as = "none"` on intermediate
      calls; close with the persisting `record_as`).
- [ ] Update the prompts to load `spec.md` via the new
      parser pattern (the prompt mentions the structured
      format and instructs the agent to consult
      `SpecMd.blocks[]` etc.).
- [ ] Move old prompts to `prompts/legacy/`.

Gate: new prompt files exist.

### Milestone 7.6: DM3a, DM3b work-prompt updates

- [ ] Update DM3a (test plan) to consume
      `SpecMd.worked_examples[]` and use
      `spec_semantic_search` for source-spec expansion when
      needed.
- [ ] Update DM3b (test impl) to use `signal_table_query`
      for confirming test driver/monitor I/O shapes.
- [ ] Both prompts gain the `ask_user` nudge for blocking
      verification choices (e.g. coverage thresholds the
      spec doesn't pin down), including the chaining
      convention.

Gate: new prompt files exist.

### Milestone 7.7: Critique-prompt updates

- [ ] Update each step's critique prompt to:
  - Parse spec.md via the structured parser.
  - Run the same `validate()` cross-ref checks as DM0's
    critique.
  - Use `signal_table_query` and `spec_semantic_search`
    where relevant to verify the work session's outputs
    against the spec.
- [ ] Specific additions per critique:
  - DM2d critique surfaces signal-table conflicts as
    findings.
  - DM2c critique verifies each milestone's named files
    correspond to a block or operation in `SpecMd`.

Gate: critique prompt files updated.

### Milestone 7.8: Gate-check refactor -- DM2a / DM2b / DM2c

- [ ] Replace existing regex-on-prose gate checks for
      DM2a/b/c with parser-based checks per Architecture
      §6.5:
  - DM2a: every operation in `decomposition.md` exists in
    `SpecMd.functional_behavior.operations[].id`.
  - DM2b: every pipeline-mapping stage references a
    `SpecMd.blocks[].name`.
  - DM2c: every milestone names exactly one Block or
    Operation.
- [ ] Keep the old regex-based gates as a fallback when
      `docs/spec.md` cannot be parsed (e.g. an old-template
      project that hasn't been migrated).
- [ ] Unit tests for each gate variant.

Gate: gate-check unit tests pass.

### Milestone 7.9: Observability metrics for DM-side tool usage

- [ ] Each step's work session emits a metrics summary at
      `step_end`:
  - Count of `api_semantic_search` calls.
  - Count of `spec_semantic_search` calls.
  - Count of `signal_table_query` calls.
  - Count of `ask_user` calls.
  - Count of distinct `ask_user` threads opened during
    the step.
  - Count of multi-turn threads (turn_count > 1) closed
    during the step.
  - Mean turn_count across closed threads (a coarse
    "ambiguity index" for the step).
  - Count of auto→manual flips triggered by `ask_user`
    during the step.
  - Count of cancelled-thread events during the step.
  - Per-tool average latency (for retrieval tools).
  - Per-`ask_user` average user wait time.
- [ ] Add the corresponding fields to the existing
      `metrics.jsonl` capture format (verify they appear
      under `target = "sim_flow::metrics"`).
- [ ] Emit the `ask_user_in_dm` event per Architecture
      §6.14 on every `ask_user` invocation from a DM step.
- [ ] Unit test: a synthetic session with N tool calls
      including one `ask_user` emits the expected counts
      and mode-flip event.

Gate: metrics test passes.

### Milestone 7.10: Per-step DM0-to-DM2c integration test

- [ ] Author `tests/dm_pipeline_integration.rs` that walks
      a synthetic project through DM0 → DM1 → DM2a → DM2b →
      DM2c using a mock LLM that returns canned responses
      verifying tool-catalog availability and gate-check
      paths.
- [ ] Verify each step's gate passes with the new
      structured artifacts.

Gate: integration test passes.

### Milestone 7.11: Live DM2d run against RV12

- [ ] Run a live DM2d session against the ingested RV12
      project (after DM0/1/2a/b/c have completed) with a
      real LLM.
- [ ] Verify the agent calls `api_semantic_search` at least
      once before writing a framework-symbol reference.
- [ ] Verify the signal-table-consistency diagnostic does
      NOT fire on a clean run (no conflicts expected).
- [ ] Introduce a deliberate spec.md edit changing one
      signal's direction; re-run DM2d's gate; verify the
      conflict fires with the expected fields.
- [ ] Record outcomes in
      `tests/fixtures/dm2d-snapshots/rv12.md`.

Gate: manual verification; recorded.

### Milestone 7.12: Tooling-effectiveness measurement

- [ ] Capture a baseline rgb_toy DM2d session BEFORE this
      phase (or use the existing captured session from the
      robustness study).
- [ ] Run the same DM2d session AFTER this phase with the
      new tools available.
- [ ] Count the invented-API rate in each (using the
      anomaly classifier from the robustness study).
- [ ] Record the comparison in
      `docs/brainstorming/model-robustness-study.md` as a
      new "Phase 7 results" section.

Gate: measurement recorded; observable improvement noted
(target: invented-API rate drops by >50%; if no improvement
flag for follow-up).

## Out of Scope (deferred to later phases)

- **Migrating existing projects' old-template spec.md.**
  Phase 8.
- **End-to-end flow validation against a full DMF run.**
  Phase 8.
- **DM4 / DM5 prompt updates.** Out of scope; per
  Architecture §6.8, DM4 sees the tools advertised but
  rarely uses them.
- **New flow shapes (DSF, SVF).** Out of scope.
- **Subprocess-CLI-agent prompt updates** (Claude Code,
  codex, copilot). They use the same universal-tools system
  message; no further changes required.
