# DM3b - Testbench Implementation (work session)

You are executing step DM3b (Testbench Implementation) of the
Direct Modeling Flow. Prerequisite: DM3a gate passed.

## Goal

Implement the UVM-lite testbench scaffolding specified by DM3a's
test plan -- the Sequencers, Drivers, Monitors, Scoreboards, and
the `SimEnvBuilder` wiring helper that DM3c will use to author
and run individual tests. **You do NOT write any test cases here**
beyond the basic data-flow smoke test in the final testbench
milestone; everything else is DM3c.

DM3b is milestone-driven: DM3a wrote one or more
`docs/test-plan/tb-milestone-NN-<name>.md` files, each with up to
10 tasks describing one slice of the testbench. You walk these in
order, completing one milestone at a time and stopping for the
paired critique after each. Do NOT chain milestones.

## Inputs

Read these before starting:

- `docs/impl-plan/plan-management.md` -- the plan-file conventions
  (task states, order, deferring, the 10-task cap).
- `docs/test-plan/test-plan.md` -- index. Read first to orient on
  testbench architecture and to see the TOC of milestone files.
- `docs/test-plan/tb-milestone-NN-<name>.md` -- DM3b's per-milestone
  task lists. The first file with at least one `- [ ]` row is your
  current milestone.
- `docs/testbench.md` -- DM1's verification strategy. Specifically
  the `## Implementation Baseline` section names the
  `lib:examples/<NN-name>/test/` directory whose layout this
  testbench mirrors. DM3b copies that example's `test/` file
  structure as a starting point and adapts component bodies.
- `docs/spec.md`, `docs/analysis/data-movement.md`,
  `docs/analysis/pipeline-mapping.md` -- payload widths and port
  names you'll wire to.
- `src/` -- the model under test; modules and payload types DM2d
  landed.

Reference material (read on demand):

- **The named baseline** under `lib:examples/<NN-name>/test/`
  from `docs/testbench.md`. Read its files end-to-end before you
  write anything in a new milestone -- the structure stays the
  same across all DM3b milestones.
- **Modeling guide -- testing chapter**:
  `lib:docs/modeling-guide/04-testing-models.md`. Consult for
  invariants the baseline isn't already demonstrating (flow
  control, idle cycles).
- **Foundation framework** public API via the `fw:` prefix.
  `fw:api/toc.md` -> the specific `fw:api/pages/.../*.md` for
  exact signatures of `SimEnv`, `SimEnvBuilder`, `Sequencer`,
  `Driver`, `Monitor`, `Scoreboard`, `Port`. Last resort.

## Procedure

1. **Orient**. Read `docs/test-plan/test-plan.md` (index) and
   `docs/testbench.md`'s `## Implementation Baseline`. If the
   `## Implementation Baseline` section is missing or names an
   example that doesn't exist, stop and flag the issue (DM1's
   gate should have caught this); don't guess a substitute.

2. **Pick the current milestone**. Walk
   `docs/test-plan/tb-milestone-NN-<name>.md` files in numeric
   order. The first file with at least one open `- [ ]` row is
   your current milestone. Read ONLY that milestone file and any
   supporting files its tasks reference -- do NOT bulk-read
   later milestones; that defeats the small-context-per-review
   point.

3. **Read the baseline `test/` directory** if you haven't yet
   (`lib:examples/<NN-name>/test/`). It's your structural
   template. The first DM3b milestone typically copies the
   baseline's file structure into `tests/`; later milestones
   adapt the bodies of files that already exist.

4. **Walk the tasks in this milestone IN ORDER**. For each
   `- [ ]` row:
   - Implement the artifact named by the task (a Rust file or
     symbol under `tests/` -- never `src/` unless the task
     explicitly says so).
   - Mirror the baseline's shape; rename types and helpers to
     match this design's payloads and ports per the plan.
   - Run `run_cargo({"command": "build"})` or
     `run_cargo({"command": "check"})` after each task or two
     to keep the compile error surface small.
   - When the task lands cleanly, flip its `- [ ]` to `- [x]`
     in the milestone file. ONLY change the checkbox; do NOT
     modify the task text, reorder rows, or add new ones.
   - If a task turns out to be wrong or impossible, leave the
     box `- [ ]` and document the discrepancy in your final
     summary instead of silently editing the plan.

5. **Final task of the final tb milestone is the smoke test**.
   The very last `tb-milestone-NN-*.md` (typically named
   `tb-milestone-05-simenv-and-smoke.md`) ends with the basic
   data-flow smoke test as a `#[test]`-annotated function. This
   is the ONE `#[test]` DM3b writes; DM3c writes the rest.
   Confirm it passes with `run_cargo({"command": "test"})`.

6. **Verify and stop**. When every `- [ ]` row in the current
   milestone is resolved (`- [x]` done OR `- [-]` deferred with
   a `- defer reason:` sub-bullet):
   - `run_cargo({"command": "build"})` -- confirm the milestone
     compiles before you stop. Cheap to run; do NOT skip.
   - On the smoke milestone only:
     `run_cargo({"command": "test"})` -- the smoke test passes.

   {{ pre_stop_hygiene }}

   Then **STOP**. Surface a clear notice:

   > `tb-milestone NN: <name> complete; ready for critique.`
   > `<one-line summary: count of files added, design tweaks
   > that landed, deferred items>`

   Do NOT roll into the next milestone. The paired critique is
   the gate; a clean critique advances DM3b to the next
   milestone, a critique with `BLOCKER:` items sends you back
   into the same milestone with focused feedback.

### Order, jumping, and deferring

{{ order_jumping_deferring }}

## Coding Requirements

{{ coding_requirements }}

DM3b split axis: one component class per file under
`tests/testbench/` (see `## File Layout` below).

## File Layout

DM3b writes most of the testbench under `tests/testbench/` -- a
subdirectory, not a single `tests/testbench.rs` file -- and splits
concerns across multiple files. The ONE exception is the
`SimEnvBuilder` helper, which lives under `src/model/test/`
because it is shared scaffolding reused by unit tests (in `src/`)
AND integration tests (in `tests/`). Do NOT duplicate the helper
in `tests/`; integration tests import the canonical helper from
the crate.

- `src/model/test/env.rs` -- **CANONICAL** location for the
  `SimEnvBuilder` wiring helper function
  (`make_env(...)` or per the test plan) plus the `OrderedModules`
  type alias and any shared port-name constants. Expose
  publicly (`pub mod test` under `src/model/mod.rs`, no
  `#[cfg(test)]` gate) so integration tests can import via
  `use <crate_name>::model::test::env::make_env;`. Do NOT write
  a `tests/testbench/env.rs` -- a duplicate copy is a
  blocker-grade violation that gets flagged in critique.
- `tests/testbench/mod.rs` -- module root that re-exports the
  per-component files. The env helper is imported from the
  crate, not declared here.
- `tests/testbench/payloads.rs` -- payload structs (or just
  re-exports of the `src/model/` types if those already cover
  the testbench's needs).
- `tests/testbench/sequencers.rs` -- one Sequencer per stimulus
  class.
- `tests/testbench/drivers.rs` -- one Driver per external
  interface.
- `tests/testbench/monitors.rs` -- one Monitor per observable
  port.
- `tests/testbench/scoreboards.rs` -- one Scoreboard per
  correctness invariant + the reference model `compute_expected`
  helpers.
- `tests/smoke/basic_data_flow.rs` -- the basic data-flow smoke
  test (the ONE `#[test]` DM3b authors). Lives outside
  `tests/testbench/` because it's a TEST, not scaffolding;
  same `tests/<category>/<test>.rs` pattern DM3c uses.

The 400-line cap applies to each individual file under
`tests/testbench/`. Adapt the split (e.g. one Scoreboard per file
under `tests/testbench/scoreboards/`) if any single file would
otherwise exceed the cap.

## Constraints

- UVM-lite invariants: Sequencer -> Driver -> DUT -> Monitor ->
  Scoreboard. No Scoreboard reaches into internal model state;
  observation goes through Monitors only.
- Do not modify `docs/test-plan/test-plan.md` or any
  `tb-milestone-*.md` STRUCTURE. Only flip `- [ ]` to `- [x]` /
  `- [-]` (with a defer-reason sub-bullet) on individual rows.
- Do not pick a different baseline than DM1's
  `## Implementation Baseline`. If it's wrong, flag it (DM1
  failed); don't silently substitute.
- Do not work multiple milestones at once. One milestone, one
  critique, repeat.
- Do not author edge / stress / random tests -- those are DM3c's
  responsibility (the `test-milestone-NN-*.md` files).
- Outside the framework's public API surface (reachable via
  `fw:`), do not read or cite sim-foundation source files.

## Re-entry

If DM3b runs across multiple work + critique sessions, restart by
walking the `tb-milestone-NN-*.md` files in numeric order. The
first one with at least one `- [ ]` row -- or any task whose
named artifact is missing from `tests/` -- is your current
milestone, and you start at the first such row in that file. Do
NOT skip a milestone just because its rows are all `[x]`; if
`cargo build` fails on first run, the prior milestone's claim of
completeness was wrong, so reopen the failing tasks first.

## Output

{{ output_intro }}

When the artifacts in the current milestone are complete and
verified, stop with the milestone-complete notice. Do not write
`docs/critiques/DM3b-critique.json`; the critique is a distinct
task. Do not `/exit` on your own -- the user and the orchestrator
control session boundaries.

Final output, after all milestones are complete and the final
critique has passed:

- Testbench sources under `tests/` mirroring the named baseline.
- The basic data-flow smoke test from the final tb-milestone
  implemented and passing.
- Every `tb-milestone-NN-*.md` task `- [x]` (or `- [-]` with a
  documented `defer reason:`).
- `cargo build` and `cargo test` both succeed.
