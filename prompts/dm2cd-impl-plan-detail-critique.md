# DM2cd - Implementation Plan, Detail (critique session)

You are reviewing one milestone of the DM2cd implementation-plan
DETAIL step. Treat it as work produced by a third party even if
you produced it yourself earlier. The orchestrator scopes you to
ONE milestone file per critique session -- the same one the
detail-step Work session just expanded -- so this review is
focused on that milestone's task list, not the whole plan.

## Inputs

The orchestrator inlines:

- `docs/impl-plan/plan.md` -- plan index (for context).
- The current detailed milestone file (the one you're reviewing).

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state
  conventions.
- `docs/spec.md`, `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md`,
  `docs/testbench.md`, `docs/targets.md` -- only the sections
  the milestone's Trace points at.

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM2cd cannot
advance past this milestone until fixed). Prefix informational
notes with `UNRESOLVED:`. The gate fails on `BLOCKER:` lines.

**Finding-marker grammar.** Same as DM2c critique -- the gate
parses `BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` lines optionally
preceded by list / heading / bold / one-glyph decoration.

This critique reviews ONE milestone's detailed task list. Do
NOT review other milestones; sibling stubs are intentionally
hidden so each milestone gets a focused review.

1. Is the `<!-- detail-pending -->` placeholder GONE from the
   file? If it's still present anywhere in the body, the work
   session didn't land -- `BLOCKER:` and stop here.
2. Does the `## Tasks` section now contain real `- [ ]`
   bulleted rows per `plan-management.md`? An empty `## Tasks`
   section = `BLOCKER:`.
3. Does each task name a CONCRETE artifact (`<path>::<Symbol>`
   pattern preferred when applicable)? Reject vague tasks like
   "implement the pipeline" or "write tests" as `BLOCKER:` and
   quote them.
4. Are tasks within scope of THIS milestone? Tasks that belong
   in a sibling milestone (e.g. payload type rows in the
   per-stage-logic milestone) = `BLOCKER:`.
5. Is the task count reasonable? Plan-management.md caps at
   ~10 per milestone; flag overflows as `UNRESOLVED:` (a
   milestone the detail step couldn't fit cleanly suggests the
   outline's breakdown was too coarse -- but DM2cd can't fix
   that, so don't BLOCKER).
6. Does the task list describe WHAT will be built without
   prescribing the algorithm inside each module's `evaluate()`?
   Tasks may name files / symbols / I/O payloads; they MUST
   NOT include shift-and-mask recipes, intermediate variable
   names, packing-format choices, loop-vs-vectorized decisions.
   Flag offending lines as `BLOCKER:`.
7. Are open decisions surfaced as explicit `DECIDE:` (or
   `OPEN:` for DM3-bound items) rows with the format
   `- [ ] DECIDE: <question> -- options: <A | B>; default:
   <pick>; rationale: <one line>`? Decisions buried as
   parenthetical asides inside other tasks = `BLOCKER:`.
8. Does the task list trace cleanly to the milestone's Trace
   section (which DM2c set)? Tasks that lack any link back to
   `decomposition.md` / `data-movement.md` /
   `pipeline-mapping.md` entries are `UNRESOLVED:` (might be
   legitimate scaffolding) but tasks that contradict the trace
   are `BLOCKER:`.
9. Does the milestone stay within DM2d scope? Reject pre-empts
   of DM3 (directed verification, coverage targets,
   scoreboards) as `BLOCKER:`.
10. Are `## Auto-decisions` entries (when present) reasonable?
    Auto-decisions are how the detail step records non-obvious
    choices; flag any that contradict the outline's Scope
    (rather than refining it) as `BLOCKER:`.

## Output

Write `docs/critiques/DM2cd-critique.md`. Free-form markdown
body; only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`,
`RESOLVED:`) are inspected by the gate.
