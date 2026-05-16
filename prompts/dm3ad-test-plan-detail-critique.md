# DM3ad - Test Plan, Detail (critique session)

You are reviewing one milestone of the DM3ad test-plan DETAIL
step. {{ third_party_reviewer_note }} The orchestrator scopes you
to ONE milestone file per critique session -- the same one the
work session just expanded. This review is focused on that
milestone's task list, not the whole plan.

## Inputs

The orchestrator inlines:

- `docs/test-plan/test-plan.md` -- the index (for context).
- The current detailed milestone file (the one you're
  reviewing).

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state
  conventions.
- `docs/test-plan/coverage.md` -- relevant for
  `test-milestone-05-coverage.md` review.
- `docs/spec.md`, `docs/targets.md`, `docs/testbench.md`,
  `docs/analysis/*.md` -- only sections the milestone Trace
  points at.

## Evaluation

{{ critique_kinds }}

This critique reviews ONE milestone's detailed task list. Do
NOT review other milestones; sibling stubs are intentionally
hidden.

1. Is the `<!-- detail-pending -->` placeholder GONE from the
   file? If still present, the work session didn't land --
   `BLOCKER:` and stop here.
2. Does the `## Tasks` section now contain real `- [ ]`
   bulleted rows? Empty `## Tasks` = `BLOCKER:`.
3. Does each task name a CONCRETE artifact?
   - For `tb-milestone-*`: `tests/testbench/<file>.rs::<symbol>`
     pattern (or `tests/smoke/<test>.rs::<test>` for the smoke
     test). Reject vague tasks ("implement the testbench") as
     `BLOCKER:`.
   - For `test-milestone-*`: `tests/<category>/<test>.rs::<test>`
     pattern. The category must match the milestone's category
     (smoke / edge / stress / random / coverage). Tasks landing
     in the wrong category = `BLOCKER:`.
4. Are tasks within scope of THIS milestone? Tasks belonging in
   a sibling milestone = `BLOCKER:`.
5. Is the task count ≤10? Overflows = `UNRESOLVED:` (a sign
   the outline's breakdown was too coarse, but DM3ad can't
   split mid-detail; flag for the outline-step retry).
6. Is the milestone within the right CATEGORY for its file
   name?
   - `test-milestone-01-smoke*.md`: smoke tests only
     (elaboration / basic data flow / backpressure-when-
     applicable / idle-cycle).
   - `test-milestone-02-edge*.md`: boundary / corner-case
     tests only.
   - `test-milestone-03-stress*.md`: sustained traffic; MUST
     exercise targets from `docs/targets.md`.
   - `test-milestone-04-random*.md`: constraint-randomized
     stimulus; tests MUST have seeds in names.
   - `test-milestone-05-coverage.md`: walks the
     `coverage.md` command + records the measurement.
   Mixing categories = `BLOCKER:`.
7. Does the task list trace cleanly to the milestone's Trace
   section? Tasks that lack a trace link to spec / target /
   decomposition entries = `UNRESOLVED:`; tasks that contradict
   the Trace = `BLOCKER:`.
8. For random milestones: does every test pin a seed in its
   name? Missing seed = `BLOCKER:` (failures must be
   reproducible).
9. Does the task list pre-empt DM3b/DM3c implementation
   choices? Naming the file path + symbol + pass criteria is
   in scope; specifying internal struct layouts, function
   bodies, or framework-specific helpers is `BLOCKER:` (the
   plan describes WHAT, not HOW).
10. Are `## Auto-decisions` entries (when present) reasonable?
    Auto-decisions that contradict the outline's Scope (rather
    than refining it) = `BLOCKER:`.

## Output

{{ output_intro }}

{{ critique_output_block }}