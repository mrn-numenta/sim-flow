# DM2cd - Implementation Plan, Detail (work session)

You are executing step DM2cd (Implementation Plan, DETAIL) of the
Direct Modeling Flow. Prerequisite: DM2c gate passed.

## Goal

Walk each `docs/impl-plan/milestone-NN-<name>.md` STUB written by
DM2c and replace its `<!-- detail-pending -->` placeholder with the
full per-milestone task list per
`docs/impl-plan/plan-management.md`. **One milestone per work +
critique session** -- the orchestrator scopes you to a single
stub each iteration so the per-milestone task list gets a focused
review.

## Inputs

The orchestrator inlines:

- `docs/impl-plan/plan.md` -- the index with the per-milestone
  scope blurbs (your map of the whole plan).
- The CURRENT milestone stub file you are detailing this turn.

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state conventions
  (`- [ ]` / `- [x]` / `- [-]` formats, the 10-task cap,
  `DECIDE:` / `OPEN:` shapes for unresolved choices).
- `docs/spec.md`, `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md` -- the predecessor inputs your
  current stub's `## Dependencies` and `## Trace` sections
  reference. Read only the sections relevant to THIS milestone.

## Procedure

1. Open the current milestone stub (the orchestrator scopes
   exactly one). Read its Scope, Dependencies, and Trace
   sections -- these are the contract DM2c set; you fill in the
   tasks.

2. Read the predecessor input sections the Trace points at.
   Don't bulk-read everything; stay scoped.

3. Replace the `## Tasks` section's
   `<!-- detail-pending: ... -->` comment with a real task list.
   Each task row follows `plan-management.md`'s format -- typical
   shape:

   ```markdown
   - [ ] `<path>::<Symbol>` -- short imperative description
     - mirrors: <pointer to baseline / similar example, if any>
     - traces to: <the spec / analysis section this satisfies>
   ```

   Sub-bullets are optional but encouraged when they help DM2d
   land the task without re-deriving the design. Cap each
   milestone at ~10 tasks; if you'd need more, surface a
   structural concern in your `## Auto-decisions` (don't
   silently overflow).

   Tasks the agent can't complete without making a design
   decision get the `DECIDE:` shape:

   `- [ ] DECIDE: <short question> -- options: <A | B | ...>; default: <pick>; rationale: <one line>.`

   Decisions DM3 must resolve get `OPEN:` instead.

4. Add a `## Auto-decisions` trailing section recording any
   structural choices you made expanding the stub (e.g. "split
   per-stage logic into avg / gray / rev tasks rather than one
   per-stage task; rationale: each stage has a distinct
   evaluate() body and a separate critique helps").

5. Once the task list is in place, the placeholder marker
   (`<!-- detail-pending -->`) MUST be gone from the file -- the
   orchestrator's gate keys on it. Replacing the comment block
   with the real `## Tasks` body removes it naturally; do NOT
   leave the comment as a footer.

6. Surface the canonical milestone-complete notice when the
   stub is fully detailed:

   > `milestone NN: <name> complete; ready for critique.`
   > `<one-line summary: count of tasks added, decisions
   > flagged, deferred items>`

   Do NOT proceed to the next milestone. The orchestrator
   re-launches a fresh session per milestone after each
   critique passes.

## Output

{{ output_intro }}

## Constraints

- DO NOT write source code. Stub-detail markdown only.
- DO NOT modify other milestone stubs. Sibling milestones are
  intentionally hidden from your session; the orchestrator
  enforces one-milestone-at-a-time.
- DO NOT modify `docs/impl-plan/plan.md`. The outline step
  (DM2c) owns it; if you spot a structural issue, surface it
  in `## Auto-decisions` rather than silently editing.
- DO NOT cite specific framework APIs (`Module`, `HasLogic`,
  etc.). Tasks describe WHAT will be built; DM2d picks HOW.
- DO NOT add new milestones. The outline step decided the
  breakdown; if a milestone is missing, flag it in
  `## Auto-decisions`.
- DO NOT remove the placeholder by leaving `## Tasks` empty.
  An empty task list is a `BLOCKER:` for the critique.

When the current milestone is fully detailed, stop. Do not
write `docs/critiques/DM2cd-critique.md`; the critique is a
distinct task. Do not `/exit` on your own -- the user and the
orchestrator control session boundaries.
