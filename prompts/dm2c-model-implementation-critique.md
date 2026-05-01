# DM2c - Implementation Plan (critique session)

You are reviewing the DM2c implementation plan. Treat it as work
produced by a third party even if you produced it yourself earlier
in this conversation -- the independent-review property depends on
you bracketing any prior reasoning rather than leaning on it. The
plan is the contract DM2d will execute against; gaps here
propagate forward as missing code or thrash during implementation.
Do not modify the plan; evaluate it and write the critique file.

## Inputs

- `docs/plan/plan-management.md` -- plan-file conventions.
- `docs/plan/plan.md` -- plan index + TOC.
- `docs/plan/milestone-*.md` -- per-milestone task lists.
- `docs/spec.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM2d cannot proceed
until fixed). Prefix informational notes -- nits, follow-up
questions, things DM2d can work around -- with `UNRESOLVED:`. The
orchestrator fails the DM2c gate on `BLOCKER:` lines only.

1. Does `docs/plan/plan.md` follow the conventions in
   `plan-management.md`? Is there an overview and a TOC pointing at
   each milestone file?
2. Are milestones named `Milestone NN: <description>` and saved as
   `milestone-NN-<name>.md`? Are the numbers contiguous?
3. Is every task a `[ ]`-prefixed bullet that names a concrete
   artifact (file path, module name, payload struct name)? Reject
   vague tasks like "implement the pipeline" or "write tests".
4. Does every operation in `decomposition.md` map to at least one
   task? Quote the operation name and the task that covers it.
5. Does every payload in `data-movement.md` map to at least one
   task that produces or consumes it?
6. Is the milestone ordering correct -- payload types before
   modules, skeletons + connectivity before per-stage logic, logic
   before its tests? Flag tasks whose dependencies live in later
   milestones.
7. Does the plan cover the four required smoke tests (elaboration,
   data flow, backpressure, idle cycles) and at least one unit test
   per non-trivial module?
8. Does the plan stay within DM2d scope? Reject tasks that pre-empt
   DM3 (directed verification suites, coverage targets,
   scoreboards, randomized stimulus).
9. Does the plan avoid prescribing specific framework APIs? It
   should describe WHAT will be built and IN WHAT ORDER, not HOW
   each module's `evaluate()` is implemented -- those decisions
   belong to DM2d.
10. Are open decisions (e.g. buffer depths, fanouts not pinned by
    analysis) called out as explicit decision-tasks rather than
    silently deferred?

## Output

Write `docs/critiques/DM2c-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
