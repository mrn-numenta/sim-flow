# DM2d - Model Implementation (work session)

You are executing step DM2d (Model Implementation) of the Direct
Modeling Flow. Prerequisite: DM2c gate passed.

## Goal

Execute the implementation plan written in DM2c to produce a
cycle-accurate sim-foundation model that elaborates and passes smoke
tests. Exhaustive verification is DM3, not here.

## Inputs

Read these before starting:

- `docs/plan/plan.md` -- the milestone index. Read this first; it
  tells you which milestone files to read in what order.
- `docs/plan/milestone-*.md` -- per-milestone task lists. The plan
  is your source of truth for what to build and in what order;
  follow it task by task.
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

Reference material (read on demand; do NOT bulk-read upfront):

- **sim-models** repo via the `lib:` prefix (modeling guide, worked
  examples, library models, prior user projects).
- **foundation framework** public API via the `fw:` prefix. Start with
  `fw:api/toc.md`, then read only the specific
  `fw:api/pages/.../*.md` files you need. Use `fw:src/prelude.rs` or
  other `fw:src/...` files only as a secondary source when you need
  exact signatures or source-level examples. Do not browse internal
  helpers; treat anything outside the curated public API surface as
  implementation detail.

Each top-level `lib:` directory has a `README.md` that indexes its
contents -- start there before diving into individual files. Consult
`lib:` material in this order of authority:

- **Modeling guide** (canonical): start with
  `lib:docs/modeling-guide/README.md`, then read the numbered
  chapters it points to (01-quickstart, 02-rust-quickstart,
  03-building-models, 04-testing-models,
  05-observability-and-analysis, 06-design-patterns).
- **Worked examples** (canonical, minimal): start with
  `lib:examples/README.md` for the index, then read the README
  inside each example directory before its source. Pick a few
  examples whose topology matches your design.
- **Library models** (canonical, production-grade reusable building
  blocks): start with `lib:library/README.md` if present, and the
  per-module READMEs (e.g. `lib:library/micron-lpddr5x/README.md`).
  Reference for non-trivial structures (memory subsystems, NoC
  fabrics, SoC composition).
- **User projects** (NON-canonical, illustrative only):
  `lib:users/<name>/` -- prior-art models from other users in this
  org. Read any `README.md` first. Useful for ideas but they may
  use older idioms or non-standard patterns; prefer the modeling
  guide / examples / library when they conflict.

You MAY consult the foundation framework's public API via `fw:`
(start with `fw:api/toc.md`); do NOT browse other sim-foundation
crates or the framework crate's internal helper modules. Use `fw:` and
`lib:` to answer "how do I express this in the framework?", not
"what should I build?" The DM2c plan remains the source of truth for
scope and structure.

## Procedure

1. Read `docs/plan/plan.md` to orient. Then process each milestone
   in order from `docs/plan/milestone-*.md`.
2. Read `docs/targets.md` and `docs/testbench.md` before starting
   implementation.
   - Use `docs/targets.md` to preserve target-sensitive structural
     choices such as gate-budget-driven stage boundaries, buffering, or
     other performance-sensitive implementation decisions already baked
     into the plan and mapping.
   - Use `docs/testbench.md` to notice observability or smoke-test needs
     that must be supported structurally during implementation.
   - Do not turn DM2d into a full verification-implementation step;
     DM3 still owns the full testbench and verification suite.
3. For each milestone:
   - Read its `milestone-NN-<name>.md` file.
   - Work through tasks in the order they're listed. See "Order,
     jumping, and deferring" below before deviating.
   - As you complete each task, edit the milestone file to flip
     its `- [ ]` to `- [x]`.
   - Run `run_cargo` as appropriate (see step 6 below) to confirm
     compilation / tests as you land artifacts.
   - When the LAST `- [ ]` in the milestone is resolved (`- [x]`
     done OR `- [-]` deferred), **STOP**. Surface a clear notice
     -- `Milestone NN: <name> complete; ready for review.`
     followed by a one-line summary of what landed (and any
     deferred items, with a single sentence each) -- and wait for
     user input before starting the next milestone. Do NOT chain
     milestones. The user reviews the code / tests / build state
     between milestones.

### Order, jumping, and deferring

`docs/plan/plan-management.md` is the source of truth for how to
walk a plan: task states (`- [ ]` pending, `- [x]` done, `- [-]`
deferred with `defer reason:` sub-bullet), how to handle
out-of-order work (`order swap:` sub-bullet documenting why),
and how to add work the plan missed (`added:` sub-bullet on a
new `- [ ]` / `- [x]` row). Read it before starting; follow it
strictly.
4. **Payload types**: create Rust structs in `src/model/` derived
   from `data-movement.md`. Payload types live alongside the
   modules that produce and consume them.
5. **Connectivity**: build a `ConnectivityPlan` that wires every
   pipeline stage. Name modules after operations from
   `decomposition.md` so future readers can trace
   spec -> decomposition -> code.
6. **Modules**: implement each module using the Foundation
   `Module`, `HasLogic`, and `HasInstances` traits. Every module
   must respect framework invariants and the structure established by
   the plan and pipeline mapping. For simple modules, implementing logic
   directly in `evaluate()` may be fine; for complex modules, factor
   helpers when that improves clarity. Do not treat those style notes as
   permission to ignore the plan's intended architecture.
7. **Cargo verification**: after each module lands, invoke the
   `run_cargo` tool to verify it compiles / passes:
   - `run_cargo({"command": "check"})` for the cheap type-only
     pass while iterating.
   - `run_cargo({"command": "build"})` once you think a module is
     done.
   - `run_cargo({"command": "test"})` once smoke / unit tests are
     in place.
   Read the returned stdout / stderr; if there are real errors,
   fix them and re-run. Do NOT guess at build errors from source
   -- always confirm with `run_cargo` output.
8. **Tests**: write **only** unit tests and smoke tests at this
   step. Exhaustive verification (directed sequences, coverage
   targets, randomized stimulus, scoreboards) belongs to **DM3** --
   do not pre-empt that scope here. Cover:
   - **Smoke**: elaboration (topology builds without error), basic
     data flow through the pipeline, backpressure propagation,
     idle cycles produce no spurious outputs.
   - **Unit** (small, focused): per-module correctness of the
     `evaluate()` core for a couple of representative inputs --
     enough to catch obvious wiring / payload-type mistakes while
     iterating, not enough to substitute for DV.
   If you find yourself writing scoreboards, sequencers, or a
   directed-test suite, stop -- defer that work to DM3.

## Constraints

- Do not bypass the Foundation port system.
- Do not implement custom scheduling.
- Do not alter the `Module` phase order (evaluate -> settle -> update).
- Outside the framework's public API surface (reachable via `fw:`),
  do not read or cite sim-foundation source files; use the docs and
  examples in `sim-models/` (via `lib:`) as the source of truth.
- Stay inside the plan. If a task in the plan turns out to be wrong
  or impossible, flag the issue (in auto mode, document the
  decision in the milestone file's `## Auto-decisions` section)
  rather than silently deviating.
- References do not override the plan. If examples or public framework
  APIs suggest a different structure than the plan, prefer the plan and
  document the tension rather than drifting silently.

## Output

- `src/model/` populated with modules and payload types.
- Smoke tests + minimal unit tests under `tests/` passing. Full
  verification suite is DM3, not here.
- Every milestone file's tasks marked `[x]`.
- `cargo build` and `cargo test` both succeed.

Milestone completion and step completion are different:

- After each milestone is complete, stop and wait for user review before
  starting the next milestone.
- After the final milestone is complete and the build/tests are green,
  stop the step.

Do not write
`docs/critiques/DM2d-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
