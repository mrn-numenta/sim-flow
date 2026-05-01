# DM3c - Test Execution and Coverage (work session)

You are executing step DM3c (Test Execution and Coverage) of the
Direct Modeling Flow. Prerequisite: DM3b gate passed.

## Goal

Implement and run every test enumerated in `docs/plan/test-plan.md`
across all four categories (smoke, edge, stress, random) using the
UVM-lite testbench DM3b landed. Then measure coverage with
`cargo-tarpaulin` and meet the threshold the plan specified
(default 90% line coverage on `src/`, or whatever the plan
declared).

## Inputs

- `docs/plan/test-plan.md` -- the test enumeration; you tick off
  each `- [ ]` row as you implement and pass that test.
- `tests/` -- the testbench scaffolding DM3b produced. You add
  test bodies but do NOT modify the scaffolding (Sequencers,
  Drivers, Monitors, Scoreboards, `SimEnvBuilder` helper). If
  the scaffolding is wrong, flag it and stop -- DM3b's gate
  failed.
- `src/` -- the model under test, for understanding behavior
  when a test fails.

## Procedure

1. Read `docs/plan/test-plan.md`. Implement tests in the order
   they appear, category by category: `## Smoke` first
   (scaffolding sanity), then `## Edge`, then `## Stress`, then
   `## Random`. The basic data-flow smoke test is already passing
   from DM3b -- start with the next smoke entry.

   Treat each `## <Category>` as a milestone. When every `- [ ]`
   row in a category is resolved (`- [x]` done OR `- [-]`
   deferred), **STOP**. Surface a clear notice -- `<Category>
   tests complete; ready for critique.` followed by a one-line
   summary (count of new tests, design fixes that landed, any
   deferred items) -- and wait for user input before starting the
   paired critique session before starting the next category. Do
   NOT chain categories. The critique is the primary correctness
   check for each category; user review may happen around it, but
   you should assume advancement depends on the critique passing.

   See "Order, jumping, and deferring" below for what to do when
   a test row depends on another row's fix, when you discover a
   test that wasn't in the plan, or when a test should be
   deferred.
2. For each test row:
   - Write the test body using the testbench helpers DM3b
     defined; do not invent new components.
   - Run `cargo test <test_name>` (via `run_cargo` if the agent
     has it; otherwise the equivalent shell invocation).
   - If the test fails because of a design bug in `src/`, fix
     the model and re-run; record the fix in the test row's
     "fix:" sub-bullet.
   - If the test fails because of a test bug, fix the test.
   - Once passing, flip the row's `- [ ]` to `- [x]` in
     `docs/plan/test-plan.md`.
3. **Random tests**: every random test must pin a seed in its
   name (`<test>_seed_<N>`) per the plan. Failures must be
   reproducible from the seed alone -- never use uncontrolled
   randomness.
4. After every category's `- [ ]` is `- [x]`, run the full suite
   once more to confirm nothing regressed:
   `run_cargo({"command": "test"})`.
5. **Coverage with `cargo-tarpaulin`**:
   - Install if missing: `cargo install cargo-tarpaulin`
     (one-time per environment).
   - Run the command from the plan (typically
     `cargo tarpaulin --out Html --out Lcov --output-dir target/coverage`).
   - Read the line-coverage percentage from the output.
   - If at or above the plan's threshold, write the report path
     and the measured percentage into the `## Coverage` section
     of `docs/plan/test-plan.md` (e.g. `coverage report:
     target/coverage/tarpaulin-report.html (line: 92.4%)`).
   - If below threshold, identify uncovered lines (open the HTML
     report) and either:
     - add tests to cover them (preferred -- adds new
       `- [x]` rows to the appropriate category), or
     - mark them as intentional exclusions in the plan's
       `## Coverage > Exclusions` list, with a one-line reason.
       The reason must be specific (e.g. "platform-gated
       Windows path under `cfg(windows)`"), not vague
       ("unimportant").

## Order, jumping, and deferring

`docs/plan/plan-management.md` is the source of truth: task
states (`- [ ]` / `- [x]` / `- [-]` with `defer reason:`),
out-of-order work (`order swap:` sub-bullet), and additions
(`added:` sub-bullet). Read it before starting; the conventions
apply to test rows in `docs/plan/test-plan.md` exactly as they do
to milestone tasks in DM2c's plan.

DM3c-specific note: deferred (`- [-]`) rows count as resolved for
the category-completion check, but they do NOT contribute to the
coverage threshold's pass criterion. A category where every row
was deferred is a `BLOCKER:` for the critique to flag.

## Re-entry

If the suite is large, DM3c may run across multiple work +
critique sessions. The gate stays open until every plan row is
`- [x]` or `- [-]` (with a documented `defer reason:`) and the
coverage threshold is met. Restart from the highest-numbered
category that still has open rows.

## Output

- `cargo test` passes (every implemented test).
- `cargo tarpaulin` reports line coverage at or above the plan's
  threshold.
- Every row in `docs/plan/test-plan.md` is `- [x]` or `- [-]`
  with a specific `defer reason:`.
- The coverage report path + measured percentage is recorded in
  the plan's `## Coverage` section.
- Any uncovered lines that are intentionally left below direct test
  coverage are recorded in `## Coverage > Exclusions` with a
  concrete reason.

Category completion and step completion are different:

- After each category is complete, stop and wait for the paired
  category critique before starting the next category.
- After the final category, the full-suite rerun, and coverage work are
  complete, stop for the final DM3c critique. That final critique is the
  end-to-end regression/coverage pass, not the first time the tests are
  being reviewed.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM3c-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
