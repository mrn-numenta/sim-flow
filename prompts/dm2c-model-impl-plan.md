# DM2c - Implementation Plan, Outline (work session)

You are executing step DM2c (Implementation Plan, OUTLINE) of the
Direct Modeling Flow. Prerequisite: DM2b gate passed.

## Goal

Produce the IMPLEMENTATION PLAN OUTLINE for the cycle-accurate
sim-foundation model that DM2d will build. **You write the index
(`plan.md`) plus one STUB file per milestone.** You do NOT write
any code, and you do NOT yet write the per-milestone task list --
the per-milestone task list is DM2cd's job, walking each stub one
at a time so each task list gets a focused critique.

The outline step exists because for non-trivial designs a single
session can't fit "the spec + decomposition + targets + per-
milestone task lists for 25 milestones" in one prompt without
truncation. The outline names the milestones; the detail step
expands each.

## Inputs

Read these before writing the outline:

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  (index + per-milestone files, milestone / task numbering,
  `[ ]` checkbox format).
- `docs/spec.md` -- the specification.
- `docs/targets.md` -- modeling targets.
- `docs/testbench.md` -- verification strategy and planned
  testbench architecture from DM1.
- `docs/analysis/decomposition.md` -- operations and module
  decomposition.
- `docs/analysis/pipeline-mapping.md` -- pipeline topology.
- `docs/analysis/data-movement.md` -- payload widths and
  dataflow.

Do NOT consult framework or library reference material here.
That belongs in DM2d when the agent writes code.

## Procedure

1. Read each input above.
2. Decide the milestone breakdown. A milestone is a coherent
   slice of work that lands a self-contained capability. Typical
   shapes for the model implementation:
   - **payload types** -- one milestone covering all
     `src/model/*` payload structs from `data-movement.md`.
   - **module skeletons + connectivity** -- modules from the
     decomposition stubbed out and wired per
     `pipeline-mapping.md`; elaboration succeeds.
   - **per-stage logic** -- one milestone per pipeline stage
     (or per cluster of related stages).
   - **smoke + unit tests** -- elaboration, basic data-flow,
     and any flow-control / idle-cycle tests that
     `docs/testbench.md` explicitly defines.

   Order milestones so each one's dependencies are in earlier
   milestones (payloads before modules, skeletons before logic,
   logic before tests).

3. Write `docs/impl-plan/plan.md` (the index). It contains:
   - A 1-2 paragraph design summary (what the model does).
   - A TOC mapping each milestone to its file.
   - A per-milestone 1-2 sentence SCOPE blurb (NOT a task
     list -- the blurb tells DM2cd what the milestone
     covers so it can write the task list without re-reading
     every input).
   - Trace links from each milestone scope back to the
     specific entries in `decomposition.md` /
     `pipeline-mapping.md` / `data-movement.md` that drive it.

4. Write one stub file per milestone at
   `docs/impl-plan/milestone-NN-<name>.md`. Two-digit zero-padded
   number (`milestone-01-payload-types.md`,
   `milestone-02-skeletons-and-connectivity.md`, ...) so the
   directory sorts in plan order.

   Each stub file uses this exact template:

   ```markdown
   # Milestone NN: <Name>

   ## Scope

   <1-paragraph description: which modules / components /
   behaviors this milestone delivers, ending with the
   acceptance criterion (e.g. "elaboration succeeds with the
   declared module set").>

   ## Dependencies

   - Predecessor milestones: <list of milestone-NN entries that
     must complete first, or "none" for milestone 01>.
   - Predecessor inputs: <which docs / src directories the
     detail step needs to read (e.g.
     `docs/analysis/data-movement.md`, `docs/spec.md`)>.

   ## Trace

   - decomposition.md: <which operations / modules>
   - data-movement.md: <which payloads / edges>
   - testbench.md: <observability hooks, if any>

   ## Tasks

   <!-- detail-pending: DM2cd replaces this section with the full
   task list per `docs/impl-plan/plan-management.md`. The scope +
   dependencies + trace above are the contract this milestone
   delivers; expand into concrete `- [ ]` rows naming files,
   symbols, and acceptance signals. -->
   ```

   The exact comment marker `<!-- detail-pending -->` is
   load-bearing: the orchestrator's gate fails until every
   stub has been detailed (placeholder removed). Keep it
   verbatim. Do NOT add task rows to the stub.

5. Trace every operation in `decomposition.md` and every payload
   in `data-movement.md` to at least one milestone (in some
   stub's `## Trace` section). Decisions the detail step needs
   to surface (`DECIDE:` / `OPEN:` per `plan-management.md`)
   are DM2cd's concern, not yours -- name the milestone where
   the decision will land in your scope blurb so DM2cd has the
   pointer.

## Output

{{ output_intro }}

- `docs/impl-plan/plan.md` -- the index with design summary +
  TOC + per-milestone scope blurbs.
- `docs/impl-plan/milestone-NN-<name>.md` -- one stub file per
  milestone with Scope / Dependencies / Trace sections plus the
  `<!-- detail-pending -->` placeholder.

## Constraints

- DO NOT write source code. Stub files only.
- DO NOT write the per-milestone task list -- DM2cd does that.
- DO NOT remove the `<!-- detail-pending -->` placeholder from
  the stubs; the orchestrator's gate keys on it.
- DO NOT cite specific framework APIs (`Module`, `HasLogic`,
  `ConnectivityPlan`). Those are DM2d's concern.
- DO NOT pre-empt DV scope. Verification concerns belong in
  DM3.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2c-critique.json`; the critique is a distinct
task. Do not `/exit` on your own -- the user and the orchestrator
control session boundaries.
