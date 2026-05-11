# DM3ad - Test Plan, Detail (work session)

You are executing step DM3ad (Test Plan, DETAIL) of the Direct
Modeling Flow. Prerequisite: DM3a gate passed.

## Goal

Walk each milestone STUB written by DM3a and replace its
`<!-- detail-pending -->` placeholder with the full task list per
`docs/impl-plan/plan-management.md`. The orchestrator scopes you
to ONE stub per work + critique session, walking
`tb-milestone-NN-*.md` and `test-milestone-NN-*.md` files in
lexicographic order.

## Inputs

The orchestrator inlines:

- `docs/test-plan/test-plan.md` -- the index (testbench
  architecture, traceability, per-milestone scope blurbs).
- The CURRENT milestone stub file you are detailing this turn.

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state
  conventions.
- `docs/test-plan/coverage.md` -- coverage strategy (relevant
  for `test-milestone-05-coverage.md`).
- `docs/spec.md`, `docs/targets.md`, `docs/testbench.md`,
  `docs/analysis/decomposition.md`,
  `docs/analysis/data-movement.md`,
  `docs/analysis/pipeline-mapping.md` -- only the sections the
  current stub's Trace section points at.
- The named baseline from `docs/testbench.md`'s
  `## Implementation Baseline` -- structural reference for
  tb-milestone task paths.

## Procedure

1. Open the current milestone stub. Read its Scope, Components/
   Tests, and Trace sections -- DM3a's contract for this
   milestone.

2. Read the Trace's predecessor sections.

3. Replace the `## Tasks` section's
   `<!-- detail-pending: ... -->` comment with a real task list.

   For **tb-milestone-NN-*.md** (DM3b's slices), each task names
   a concrete artifact path under `tests/testbench/` (or
   `tests/smoke/` for the smoke test):

   ```markdown
   - [ ] `tests/testbench/<file>.rs::<symbol>` -- <one-sentence
     purpose>
     - mirrors: `lib:examples/<NN-name>/test/<file>` (where
       applicable)
     - traces to: <spec section / target row / decomposition op>
   ```

   For **test-milestone-NN-*.md** (DM3c's slices), each task
   names a test file + name (DM3c writes one test per file
   under `tests/<category>/`):

   ```markdown
   - [ ] `tests/<category>/<test_name>.rs::<test_name>` --
     <one-sentence purpose>
     - pass criteria: <specific, measurable>
     - traces to: <spec section / target row / decomposition op>
   ```

   Test names must be identifier-safe (DM3c uses them as Rust
   `#[test]` function names AND filenames). Random tests pin a
   seed in the name AND filename:
   `tests/random/<test>_seed_<N>.rs::<test>_seed_<N>`.

4. **10-task cap per milestone**. If your task list would exceed
   ~10 rows, surface a structural concern in
   `## Auto-decisions` (don't silently overflow). The
   `plan-management.md` cap is enforced; DM3a should have split
   the milestone if it's too big.

5. **Mandatory categorical content** (the critique enforces):
   - `test-milestone-01-smoke*.md`: at minimum elaboration,
     basic data flow, backpressure (when applicable), idle
     cycles produce no spurious outputs. For combinational
     designs with no flow-control surface, write a one-line
     `RESOLVED: design has no flow-control surface, backpressure
     / idle entries do not apply` inside the file.
   - `test-milestone-03-stress*.md` MUST exercise the targets
     in `docs/targets.md`.
   - `test-milestone-04-random*.md` tests pin seeds in their
     names.
   - `test-milestone-05-coverage.md` walks `coverage.md`'s run
     command.

6. Add a `## Auto-decisions` trailing section recording any
   structural choices.

7. Once the task list is in place, the placeholder marker
   (`<!-- detail-pending -->`) MUST be gone from the file.

8. Surface the canonical milestone-complete notice:

   > `<milestone-name> complete; ready for critique.`
   > `<one-line summary: count of tasks added, decisions
   > flagged, deferred items>`

   Do NOT proceed to the next milestone.

## Output

**Use the path as the fence info-string, verbatim.** When you rewrite a
milestone file, the opening fence must be the relative path the
milestone instruction names (e.g.
`​```docs/impl-plan/milestone-NN-<slug>.md`). Opening with a
language tag (`markdown`, `json`, `toml`, `rust`, `yaml`, `text`,
`md`, `rs`, `yml`, `txt`) means the body is **silently dropped** --
the file never updates, the gate fails, and the work session burns
its retry budget. See `_conventions/fenced-blocks.md`
("Language-tag info-strings are SILENTLY DROPPED") for the failure
mode in detail.

## Constraints

- DO NOT write any test code. Stub-detail markdown only.
- DO NOT modify other milestone stubs. Sibling stubs are
  intentionally hidden.
- DO NOT modify `docs/test-plan/test-plan.md` or
  `docs/test-plan/coverage.md`. The outline step (DM3a) owns
  them; if you spot a structural issue, surface it in
  `## Auto-decisions`.
- DO NOT cite internal Foundation helpers; tasks describe WHAT
  will be built, DM3b/DM3c pick HOW.
- DO NOT add new milestone files; if the breakdown is wrong,
  flag it in `## Auto-decisions`.
- DO NOT remove the placeholder by leaving `## Tasks` empty --
  empty task list is a `BLOCKER:`.

When the current milestone is fully detailed, stop. Do not
write `docs/critiques/DM3ad-critique.md`. Do not `/exit` on
your own.
