# DM2cd - Implementation Plan, Detail (work session)

You are executing step DM2cd (Implementation Plan, DETAIL) of the
Direct Modeling Flow. Prerequisite: DM2c gate passed.

## Goal

Walk each `docs/impl-plan/milestone-NN-<name>.md` STUB written by
DM2c and replace its `<!-- detail-pending -->` placeholder with the
full per-milestone task list per
`docs/plan-management.md`. **One milestone per work +
critique session** -- the orchestrator scopes you to a single
stub each iteration so the per-milestone task list gets a focused
review.

## Inputs

The orchestrator inlines:

- `docs/impl-plan/plan.md` -- the index with the per-milestone
  scope blurbs (your map of the whole plan).
- The CURRENT milestone stub file you are detailing this turn.

Read on demand:

- `docs/plan-management.md` -- task / state conventions
  (`- [ ]` / `- [x]` / `- [-]` formats, the 10-task cap,
  `DECIDE:` / `OPEN:` shapes for unresolved choices).
- `docs/spec.md` -- use `read_markdown(path: "docs/spec.md",
  section: "Block: <name>")` to fetch only the sections the
  current milestone's `## Trace` block references. The full
  spec.md is large (40-60 KB on real designs); pulling it whole
  wastes context. Each block's `#### Retrieval hints` lists
  `spec_semantic_search` queries for source-spec context beyond
  what spec.md inlines.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md` -- predecessor inputs your
  current stub's `## Dependencies` and `## Trace` sections
  reference. Use `read_markdown` per-section here too; do NOT
  bulk-read.

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

   **Decision discipline (BLOCKER-class if violated).** A task
   body that prose-embeds an unresolved choice is wrong. Every
   pattern below means an unmade design decision is hiding inside
   another task:

   - "X (could be A or B)" / "either A or B" / "A vs B"
   - "TBD" / "to be decided" / "pending review"
   - "decide later" / "we'll figure this out in DM2d"
   - parenthetical asides naming multiple options
   - "default is A, but could be B"

   When you find yourself writing any of those, STOP. Lift the
   choice into a sibling `DECIDE:` row with the format above and
   reference it from the original task as
   `- depends on: DECIDE row above`. The critique BLOCKS on
   buried decisions; surfacing them up-front keeps the milestone
   reviewable.

   **Trace coverage (UNRESOLVED-class if violated).** Before you
   stop, re-read the milestone stub's `## Trace` section and
   verify EVERY operation, payload, and analysis line referenced
   there appears as the `traces to:` target of at least one task
   you wrote. If an operation has no covering task, either:

   - add a task for it, OR
   - record the deliberate omission in `## Auto-decisions`
     (`- decided to fold X into Y; rationale: ...`) so the
     critique can audit the choice rather than flag it as a
     gap.

   The critique runs both directions of the trace check — tasks
   without traces AND operations without tasks — so silent
   omissions don't pass.

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
write `docs/critiques/DM2cd-critique.json`; the critique is a
distinct task. Do not `/exit` on your own -- the user and the
orchestrator control session boundaries.
