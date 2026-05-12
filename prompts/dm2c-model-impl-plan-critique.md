# DM2c - Implementation Plan, Outline (critique session)

You are reviewing the DM2c implementation-plan OUTLINE. Treat it
as work produced by a third party even if you produced it
yourself earlier -- the independent-review property depends on
you bracketing any prior reasoning. The outline is the contract
DM2cd will detail against; gaps here propagate forward as
missing or mis-shaped milestones. Do not modify the plan;
evaluate it and write the critique file.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions.
- `docs/impl-plan/plan.md` -- plan index + per-milestone scope
  blurbs.
- `docs/impl-plan/milestone-*.md` -- one stub file per
  milestone (Scope / Dependencies / Trace +
  `<!-- detail-pending -->` placeholder).
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM2cd cannot
proceed until fixed). Prefix informational notes -- nits,
follow-up questions, things downstream can work around -- with
`UNRESOLVED:`. The orchestrator fails the DM2c gate on
both `BLOCKER:` and `UNRESOLVED:` lines.

{{ critique_kinds }}

This critique reviews the OUTLINE, not the per-milestone task
lists -- those are DM2cd's responsibility. Resist reviewing
content that lives in `<!-- detail-pending -->` placeholders;
flag the ABSENCE of one as a `BLOCKER:` (the gate keys on it),
but don't critique the missing tasks themselves.

1. Does `docs/impl-plan/plan.md` follow `plan-management.md`?
   Is there a design summary, a TOC pointing at every
   `milestone-NN-*.md` stub, and a 1-2 sentence scope blurb
   per milestone in the index?
2. Are milestones named `Milestone NN: <description>` in
   `milestone-NN-<name>.md`? Are the numbers contiguous (no
   gaps, no duplicates)? Is the directory order lexicographic?
3. Does each stub file contain Scope / Dependencies / Trace
   sections + the literal comment marker
   `<!-- detail-pending -->`? Missing marker = `BLOCKER:` (the
   orchestrator's detail-step gate keys on it).
4. Does each stub's Scope blurb name a coherent slice of work
   with a clear acceptance criterion (NOT a task list)? A scope
   that just lists files is a `BLOCKER:`; a scope that says
   "compile passes" without naming what's being compiled is a
   `BLOCKER:`.
5. Does each stub's Dependencies section list the predecessor
   milestones AND the predecessor input docs the detail step
   needs? Missing predecessors = `BLOCKER:`.
6. Does each stub's Trace section point at SPECIFIC entries in
   `decomposition.md` / `data-movement.md` /
   `pipeline-mapping.md` / `testbench.md`? Vague trace ("see
   decomposition.md") = `UNRESOLVED:`; missing trace entirely
   for a milestone = `BLOCKER:`.
7. Does every operation in `decomposition.md` and every payload
   in `data-movement.md` map to at least one milestone (via
   some stub's Trace section)? Quote any unmapped operation /
   payload.
8. Is the milestone ordering correct? Payload types before
   modules that use them, skeletons + connectivity before
   per-stage logic, logic before its tests. Flag any milestone
   whose Dependencies list a successor milestone.
9. Does the outline cover the elaboration smoke test, basic
   data-flow smoke test, AND any flow-control / idle-cycle
   tests **explicitly required by `docs/testbench.md`**? When
   the design has no flow-control surface, the absence of
   backpressure / idle tests is `RESOLVED:` provided some stub
   has a one-line note acknowledging the choice.
10. Does the outline stay within DM2d scope? A stub that
    pre-empts DM3 (directed verification suites, coverage
    targets, scoreboards, randomized stimulus) is `BLOCKER:`.
11. Does ANY stub file leak per-task content (concrete `- [ ]`
    rows, algorithm details, shift-and-mask recipes) into its
    Scope or Trace section? That's the detail step's job;
    stubs should describe WHAT, not HOW. Flag offending lines
    as `BLOCKER:` -- they pre-empt DM2cd.

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM2c-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM2c-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}