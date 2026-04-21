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

- [ ] Author `instructions/dm0-specification.md` (work) and
  `dm0-critique.md` (critique) per the prompts in doc 02.
- [ ] Author `dm1-modeling-setup.md` and `dm1-critique.md`.
- [ ] Author `dm2a-decomposition.md` and `dm2a-critique.md`.
- [ ] Author `dm2b-pipeline-mapping.md` and `dm2b-critique.md`.
- [ ] Author `dm2c-model-implementation.md` and `dm2c-critique.md`,
  including references to Foundation patterns (`Module`, `HasLogic`,
  `HasInstances`, `ConnectivityPlan`) and `sim-models/examples`.
- [ ] Author `dm3a-testbench-impl.md` and `dm3a-critique.md` referencing
  [uvm-lite.md](../../architecture/uvm-lite.md).
- [ ] Author `dm3b-test-plan.md` and `dm3b-critique.md`.
- [ ] Author `dm3c-test-execution.md` and `dm3c-critique.md`, including
  the re-entry idiom for partial coverage completion.
- [ ] For every critique file, include the `UNRESOLVED:` / `BLOCKER:`
  line-prefix convention at the top.

## Milestone 2 - Step Descriptors And Gate Checks (DM0-DM3)

- [ ] Register DM0-DM3 step descriptors in `crates/sim-flow/src/steps/dm.rs`.
- [ ] Implement DM0 gate: `spec.md` exists, frequency + node regex
  checks, critique file scan.
- [ ] Implement DM1 gate: `targets.md` and `testbench.md` exist with
  quantitative content, critique file scan.
- [ ] Implement DM2a gate: `analysis/decomposition.md` and
  `analysis/data-movement.md` exist, critique file scan.
- [ ] Implement DM2b gate: `analysis/pipeline-mapping.md` exists, every
  operation name from `decomposition.md` appears in the mapping,
  critique file scan.
- [ ] Implement DM2c gate: `src/model/` populated, `cargo build`
  succeeds, `cargo test` passes the elaboration test, `ConnectivityPlan`
  and `HasLogic` present via grep, critique file scan.
- [ ] Implement DM3a gate: testbench sources exist, `cargo build`
  succeeds, critique file scan.
- [ ] Implement DM3b gate: `docs/test-plan.md` exists with entries that
  reference spec.md requirements, critique file scan.
- [ ] Implement DM3c gate: `cargo test` passes, coverage report exists
  at >= 90% or has documented exclusions, test plan checklist items
  marked complete, critique file scan.
- [ ] Implement the DM3c re-entry path: detect an incomplete checklist
  and keep the gate open across multiple work/critique pairs.

## Milestone 4 - Step Descriptors And Gate Checks (DM4)

- [ ] Author `instructions/dm4-performance-analysis.md` and
  `dm4-critique.md`.
- [ ] Register DM4 step descriptor in `steps/dm.rs`.
- [ ] Implement DM4 gate with tracking-dependent checks:
  - at least one experiment row exists in `experiments.db` for this
    project
  - `docs/analysis/` contains a report with throughput and latency
    metrics
  - critique file scan
- [ ] Tag DM4 as blocked until Phase 4 Milestone 3 (metrics extraction)
  lands.

## Milestone 5 - End-To-End DMF Validation

- [ ] Create a small reference spec (e.g., a 4-stage pipeline) under
  `sim-models/users/_testflow/models/reference-pipeline/` suitable for
  exercising DM0-DM3c.
- [ ] Run `sim-flow run DM0` through `sim-flow run DM3c` against the
  reference model using the mock AI client and canned responses
  (work-session artifact fixtures) to validate the orchestrator and
  gate checks without consuming real LLM turns.
- [ ] Add a second end-to-end validation using a real AI client
  (Claude by default) gated behind an env-var opt-in so CI does not
  spend tokens.
- [ ] Document the reference-pipeline run in `docs/analysis/ai-flow/`
  as the canonical DMF smoke walkthrough.

## Milestone 6 - DMF Re-Entry And Reset

- [ ] Verify `sim-flow reset DM2a` cascades to DM2b/DM2c/DM3a/.../DM4
  correctly (reuses Phase 1 Milestone 2 logic).
- [ ] Verify a re-entered DM2a preserves artifact files on disk so the
  work session can iterate rather than starting from scratch.
- [ ] Document the re-entry UX in `docs/architecture/ai-flow/` or the
  generated `CLAUDE.md`.

## Milestone 7 - DMF Documentation And Handoff

- [ ] Add a `docs/getting-started/ai-flow-dmf.md` walkthrough that a
  user can follow to create a new model project and drive it from DM0
  to DM3c.
- [ ] Update `CHANGELOG.md` when the DMF is usable end-to-end (DM0-DM3c
  complete, DM4 unblocked by Phase 4).

## Status

Not started. Gated on Phase 1 and Phase 2.
