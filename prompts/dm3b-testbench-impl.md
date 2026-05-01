# DM3b - Testbench Implementation (work session)

You are executing step DM3b (Testbench Implementation) of the Direct
Modeling Flow. Prerequisite: DM3a gate passed.

## Goal

Implement the UVM-lite testbench scaffolding specified by DM3a's
test plan in `docs/plan/test-plan.md` -- the Sequencers, Drivers,
Monitors, Scoreboards, and the `SimEnvBuilder` wiring that DM3c
will use to author and run individual tests. **You do NOT write
any test cases here**; that's DM3c. Your job is to make the
scaffolding compile and provide a working "basic data flow" smoke
test that proves the wiring is sound.

## Inputs

Read these before starting:

- `docs/plan/test-plan.md` -- specifically the `## Testbench`
  section. This names every Sequencer / Driver / Monitor /
  Scoreboard you must implement, and the `SimEnvBuilder` wiring
  helper they expect.
- `docs/spec.md`, `docs/analysis/data-movement.md`,
  `docs/analysis/pipeline-mapping.md` -- payload widths and port
  names you'll wire to.
- `src/` -- the model under test; especially the modules and
  payload types DM2d landed.

Reference material (read on demand; the test plan should already
cite specific chapters / examples to follow):

- **Modeling guide -- testing chapter**:
  `lib:docs/modeling-guide/04-testing-models.md`. Authoritative
  source for UVM-lite component conventions and `SimEnvBuilder`
  patterns.
- **Worked examples** with testbenches: each `lib:examples/`
  directory's `tests/` is a concrete reference; pick the
  example(s) the test plan named.
- **Foundation framework** public API via the `fw:` prefix. Start with
  `fw:api/toc.md`, then read only the specific
  `fw:api/pages/.../*.md` files you need for exact types like
  `SimEnv`, `SimEnvBuilder`, `Sequencer`, `Driver`, `Monitor`,
  `Scoreboard`, and `Port`. Use `fw:src/prelude.rs` only as a
  secondary source when you need exact source-level signatures or
  examples.

## Procedure

1. Read `docs/plan/test-plan.md`'s `## Testbench` section first.
   Treat the component list as a contract; if a component named
   in the plan can't be implemented as written, stop and flag
   the issue (the plan was wrong) rather than silently deviating.
2. Implement each component named in the plan under `tests/` (or
   a dedicated test module per the example you're mirroring):
   - **Sequencer** per stimulus class.
   - **Driver** per external interface.
   - **Monitor** per observable signal / port.
   - **Scoreboard** per correctness invariant.
3. Wire the testbench via `SimEnvBuilder`. Expose a helper
   function (name from the test plan) that returns a fully
   assembled `SimEnv` ready to consume stimulus.
4. Add the "basic data flow" smoke test from the plan's
   `## Smoke` section as a `#[test]`-annotated function so
   `cargo build && cargo test --test <name>` succeeds. This is
   the ONLY test you implement here; the rest of the plan's
   tests are DM3c's responsibility.
5. Confirm with `run_cargo`:
   - `run_cargo({"command": "build"})` -- compiles cleanly.
   - `run_cargo({"command": "test"})` -- the basic data flow
     smoke test passes.
6. Read `run_cargo` output; do NOT guess at build errors from
   source -- always confirm with `run_cargo` output.

## Constraints

- UVM-lite invariants: Sequencer -> Driver -> DUT -> Monitor ->
  Scoreboard. No Scoreboard reaches into internal model state;
  observation goes through Monitors only.
- Do not author edge / stress / random tests here -- that's
  DM3c. The only test you write at this step is the basic
  data-flow smoke test that proves the scaffolding works.
- Do not modify `docs/plan/test-plan.md`. If the plan is wrong,
  flag it (DM3a failed); don't silently fix it forward.

## Output

- Testbench sources under `tests/` (or the appropriate test
  module).
- The basic data flow smoke test from the plan implemented and
  passing.
- `cargo build` and `cargo test` both succeed.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM3b-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
