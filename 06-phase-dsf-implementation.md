# Phase 6 - Design Study Flow Step Implementation

Phase dependency: Phase 3 (DMF patterns), Phase 4 (experiment tracking),
Phase 5 (DSF templates, per-candidate state, DS9 flip plumbing).

## Problem Statement

Phase 5 delivers the DSF project layout and the orchestrator support for
per-candidate execution. This phase authors the DS0-DS9 instruction files,
registers the per-step gate descriptors, and validates the DSF end-to-end
against a multi-candidate reference study. It also exercises the in-place
DS9 -> DM0 transition so the same project continues into the DMF without a
second `cargo generate`.

## Milestone 1 - Instruction Authoring (DS0-DS4)

- [ ] Author `instructions/ds0-specification.md` and `ds0-critique.md`
  per doc 03, emphasizing requirements (not design).
- [ ] Author `ds1-study-setup.md` and `ds1-critique.md`.
- [ ] Author `ds2-decomposition.md` and `ds2-critique.md` with the
  architectural-freedom framing from doc 03.
- [ ] Author `ds3-pipeline-mapping.md` and `ds3-critique.md` covering
  generation of 3-5 candidate mappings.
- [ ] Author `ds4-analytical-screening.md` and `ds4-critique.md`.
- [ ] Each critique file includes the `UNRESOLVED:` / `BLOCKER:` line
  convention header.

## Milestone 2 - Instruction Authoring (DS5-DS9)

- [ ] Author `ds5a-candidate-prototyping.md` and `ds5a-critique.md`
  parameterized by `<candidate-name>`.
- [ ] Author `ds5b-candidate-validation.md` and `ds5b-critique.md`.
- [ ] Author `ds6-comparison.md` and `ds6-critique.md`.
- [ ] Author `ds7-deep-analysis.md` and `ds7-critique.md`.
- [ ] Author `ds8-decision.md` and `ds8-critique.md`.
- [ ] Author `ds9-formalize.md` and `ds9-critique.md`, making clear
  that DS9 populates `final-model/` in place and does not call
  `sim-flow new model`.

## Milestone 3 - Step Descriptors And Gate Checks (DS0-DS4)

- [ ] Register DS0-DS4 step descriptors in
  `crates/sim-flow/src/steps/ds.rs`.
- [ ] DS0 gate: `spec.md` exists with frequency and node, critique
  scan, critique flags no premature design decisions (heuristic check
  -- look for `BLOCKER:` lines specifically about closed design space).
- [ ] DS1 gate: `study.md`, `targets.md`, at least one workload
  definition in `workloads/`, `testbench.md`, critique scan.
- [ ] DS2 gate: `analysis/decomposition.md` and
  `analysis/data-movement.md`, critique scan.
- [ ] DS3 gate: `analysis/pipeline-mapping.md` with multiple
  candidates, `candidates/` has >= 2 subdirectories, critique scan.
- [ ] DS4 gate: `analysis/screening-results.md`,
  `analysis/screening-decision.md`, at least one surviving candidate,
  critique scan.

## Milestone 4 - Step Descriptors And Gate Checks (DS5a-DS5b)

- [ ] Register DS5a and DS5b as per-candidate steps (see Phase 5
  Milestone 5).
- [ ] DS5a per-candidate gate: `candidates/<name>/src/model/`
  populated, `candidates/<name>/Cargo.toml` exists, `cargo build` and
  `cargo test` succeed in the candidate directory, per-candidate
  critique scan.
- [ ] DS5b per-candidate gate: at least one experiment row exists with
  `candidate = '<name>'` and `study = '<study>'`, workload results
  summary exists under `candidates/<name>/analysis/`, per-candidate
  critique scan.

## Milestone 5 - Step Descriptors And Gate Checks (DS6-DS9)

- [ ] DS6 gate: `comparisons/round-1-comparison.md` exists with the
  comparison matrix, at least one candidate identified as surviving,
  critique scan.
- [ ] DS7 gate: per-candidate analysis reports exist under
  `candidates/<name>/analysis/`,
  `comparisons/deep-analysis-summary.md` exists, critique scan.
- [ ] DS8 gate: `comparisons/final-decision.md` exists, a winning
  candidate is identified (or explicit "no winner" with path forward),
  critique scan.
- [ ] DS9 gate: updated `spec.md` contains a detailed architecture,
  `formalization-inputs.md` exists, `final-model/` exists and
  `cargo build` succeeds, critique scan.
- [ ] Wire the DS9 post-gate action from Phase 5 Milestone 6 so the
  flow flips to `direct-modeling` on DS9 pass.

## Milestone 6 - End-To-End DSF Validation

- [ ] Create a small reference study under
  `sim-models/users/_testflow/studies/reference-noc/` with 3 candidates
  (e.g., ring, mesh, crossbar stubs) to exercise DS0-DS9.
- [ ] Drive the full flow with the mock AI client and canned
  responses, asserting each gate's pass/fail behavior.
- [ ] Add a real-client validation gated behind an env-var opt-in.
- [ ] Exercise the DS9 -> DM0 transition and confirm `sim-flow run
  DM0` proceeds against `final-model/` in the same project.
- [ ] Exercise re-entry: reset DS3, add a new candidate, re-run DS4
  through DS9.

## Milestone 7 - DSF Documentation And Handoff

- [ ] Add a `docs/getting-started/ai-flow-dsf.md` walkthrough from
  `sim-flow new study` through `sim-flow run DM4`.
- [ ] Document the decision-matrix format and rationale for DS6 and
  DS8 so study outputs are reviewable by others.
- [ ] Update `CHANGELOG.md` when the DSF is usable end-to-end.

## Status

Not started. Gated on Phase 5 and Phase 4.
