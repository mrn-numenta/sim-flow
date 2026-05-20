# Phase 3 - Direct Modeling Flow Step Implementation

Phase dependency: Phase 1 (orchestrator), Phase 2 (model-project template).

## Problem Statement

Phase 1 delivers the generic step-running machinery; Phase 2 delivers a
project to run it against. This phase wires those together for the DMF by
authoring the per-step instruction files, defining the per-step gate
descriptors, and validating the full flow end-to-end on a small reference
model. DM5 (external PPA) is scoped out and deferred to Phase 7 because its
session structure is still TBD with the PPA engineer.

Phase 4 (experiment tracking) must land before DM4 can be completed, but
DM0-DM3 do not depend on tracking. This phase implements DM0-DM3 first, then
stubs DM4 with tracking-dependent checks marked as blocked until Phase 4.

## Milestone 1 - Instruction Authoring (DM0-DM3)

- [x] Author `instructions/dm0-specification.md` (work) and
  `dm0-specification-critique.md` (critique) per the prompts in doc 02.
- [x] Author `dm1-modeling-setup.md` and `dm1-modeling-setup-critique.md`.
- [x] Author `dm2a-decomposition.md` and `dm2a-decomposition-critique.md`.
- [x] Author `dm2b-pipeline-mapping.md` and
  `dm2b-pipeline-mapping-critique.md`.
- [x] Author `dm2c-model-impl-plan.md` and
  `dm2c-model-impl-plan-critique.md`, including references to
  Foundation patterns (`Module`, `HasLogic`, `HasInstances`,
  `ConnectivityPlan`) and `sim-models/examples`.
- [x] Author `dm3a-testbench-impl.md` and `dm3a-testbench-impl-critique.md`
  referencing [uvm-lite.md](../../architecture/uvm-lite.md).
- [x] Author `dm3b-test-plan.md` and `dm3b-test-plan-critique.md`.
- [x] Author `dm3c-test-execution.md` and `dm3c-test-execution-critique.md`,
  including the re-entry idiom for partial coverage completion.
- [x] For every critique file, include the `UNRESOLVED:` / `BLOCKER:`
  line-prefix convention at the top.

## Milestone 2 - Step Descriptors And Gate Checks (DM0-DM3)

- [x] Register DM0-DM3 step descriptors in
  `tools/sim-flow/src/steps/dm.rs`.
- [x] Implement DM0 gate: `spec.md` exists, frequency + node regex
  checks, critique file scan.
- [x] Implement DM1 gate: `targets.md` and `testbench.md` exist with
  quantitative content and UVM-lite component mentions, critique file
  scan.
- [x] Implement DM2a gate: `analysis/decomposition.md` and
  `analysis/data-movement.md` exist, at least one `## Operation:` heading
  in the decomposition, critique file scan.
- [x] Implement DM2b gate: `analysis/pipeline-mapping.md` exists and
  mentions stages, critique file scan.
- [/] Cross-reference check ("every operation name from decomposition
  appears in pipeline-mapping") is delegated to the DM2b critique prompt
  rather than being a structural gate. Revisit if the AI's semantic
  check proves unreliable in practice.
- [x] Implement DM2c gate: `Cargo.toml` with `foundation-framework` dep,
  `cargo build` succeeds, elaboration test passes, ConnectivityPlan and
  HasLogic references present, critique file scan.
- [x] Implement DM3a gate: testbench sources reference UVM-lite
  components, `cargo build` succeeds, critique file scan.
- [x] Implement DM3b gate: `docs/test-plan.md` exists with markdown
  checklist entries and references spec.md / targets.md, critique scan.
- [x] Implement DM3c gate: full `cargo test` passes, `docs/test-plan.md`
  still present, critique scan.
- [/] Coverage >= 90% structural check is delegated to DM3c critique;
  Phase 7 can add a dedicated structural check once a coverage tool is
  standardized.
- [/] DM3c re-entry path (detect an incomplete checklist and keep the
  gate open) is implemented in the state machine's back-transition
  semantics from Phase 1; no additional DM3c-specific logic is required
  beyond running the step again.

## Milestone 4 - Step Descriptors And Gate Checks (DM4)

- [x] Author `instructions/dm4-performance-analysis.md` and
  `dm4-performance-analysis-critique.md`.
- [x] Register DM4 step descriptor in `steps/dm.rs`.
- [x] Implement DM4 structural checks:
  - `docs/analysis/` contains at least one report
  - throughput / latency metrics appear in the report
  - critique file scan
- [x] DM4 critique prompt instructs the AI to emit `BLOCKER: experiment
  tracking unavailable (Phase 4 pending)` when tracking is absent, so
  the gate remains closed until Phase 4 lands.

## Milestone 5 - End-To-End DMF Validation

- [ ] Create a small reference spec (e.g., a 4-stage pipeline) under
  `sim-models/users/_testflow/models/reference-pipeline/` suitable for
  exercising DM0-DM3c. Deferred: sim-models is a separate repo and this
  reference exercise belongs there. Phase 3 ships staged-artifact gate
  integration tests in `tools/sim-flow/tests/dm_gates.rs` which
  exercise every DM0-DM4 gate's pass and fail paths without consuming
  real LLM turns.
- [ ] Run `sim-flow run DM0` through `sim-flow run DM3c` against the
  reference model using the mock AI client and canned responses. Phase
  1's `tests/smoke.rs` already exercises the full work+critique runner
  path with the mock; per-step canned fixtures for DM0-DM3 are deferred
  to Phase 5 adoption work.
- [ ] Real-client env-gated end-to-end validation. Deferred to user
  validation in sim-models.
- [ ] Canonical DMF smoke walkthrough under `docs/analysis/ai-flow/`.
  Deferred to Phase 7 documentation.

## Milestone 6 - DMF Re-Entry And Reset

- [x] Verify `sim-flow reset DM2a` cascades to DM2b/DM2c/DM3a/.../DM4
  correctly (`tests/dm_gates.rs::reset_cascades_across_dm_order` runs
  the full DM order through the state machine's reset path).
- [/] Verify a re-entered DM2a preserves artifact files on disk so the
  work session can iterate rather than starting from scratch. The
  orchestrator does not delete artifacts on reset; confirmation in a
  real project run belongs to Phase 5 adoption.
- [/] Document the re-entry UX in the generated `CLAUDE.md`. Current
  template explains state + critiques; a dedicated re-entry section is
  a Phase 7 doc polish.

## Milestone 7 - DMF Documentation And Handoff

- [ ] Add a `docs/getting-started/ai-flow-dmf.md` walkthrough that a
  user can follow to create a new model project and drive it from DM0
  to DM3c. Deferred to Phase 7.
- [ ] Update `CHANGELOG.md` when the DMF is usable end-to-end. Pending
  Phase 4 landing.

## Status

DM0-DM4 instruction files, step descriptors, and gate checks are in
place. 14 new DM-gate integration tests pass in addition to the 40
prior tests (total 57 tests across sim-flow). DM5 remains explicitly
deferred. Real end-to-end validation against a reference project in
sim-models is left for adoption; the gate descriptors are now the
authoritative contract between orchestrator and AI.

Prompt and contract refinements since the initial implementation:

- DM0 is now template-driven via `docs/spec.md.tmpl` and is explicitly
  judged on whether it is model-ready for a competent frontier LLM, not
  whether it is exhaustively specified down to every minute detail.
- DM0 and DM1 now treat a gate-budget-per-cycle target or derivable
  estimate as a hard prerequisite for DM2 decomposition and pipeline
  mapping.
- DM1 is now framed as modeling targets plus verification strategy,
  rather than detailed testbench or test-plan authoring.
