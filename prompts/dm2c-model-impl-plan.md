# DM2c - Implementation Plan (work session)

You are executing step DM2c (Implementation Plan) of the Direct Modeling
Flow. Prerequisite: DM2b gate passed.

## Goal

Produce a written implementation plan for the cycle-accurate
sim-foundation model that DM2d will build. **You do NOT write any
code in this step.** The plan is a sequenced set of milestones and
tasks, scoped from the prior decomposition / pipeline-mapping /
data-movement analysis, that DM2d can work through deterministically.
A clear plan keeps DM2d focused: each task corresponds to a
verifiable artifact (a payload type, a module, a wiring, a smoke
test).

## Inputs

Read these before writing the plan:

- `docs/plan/plan-management.md` -- the plan-file conventions
  (`plan.md` index + per-milestone files, milestone / task numbering,
  `[ ]` checkbox format).
- `docs/spec.md` -- the specification.
- `docs/targets.md` -- modeling targets, including the gate-budget target
  and any target-sensitive architectural constraints.
- `docs/testbench.md` -- verification strategy and planned testbench
  architecture from DM1.
- `docs/analysis/decomposition.md` -- operations and the
  decomposition into modules.
- `docs/analysis/pipeline-mapping.md` -- the pipeline topology.
- `docs/analysis/data-movement.md` -- payload widths and dataflow.

You do NOT need to read framework or library reference material here.
That belongs primarily in DM2d when the agent actually writes code.
If you need to sanity-check that a planned artifact maps onto an
existing public framework surface, consult `fw:api/toc.md` and only
the specific `fw:api/pages/...` files you need. Do not bulk-read the
framework API and do not turn DM2c into an implementation-spelunking
step.

## Procedure

1. Read each input above.
2. Decide the milestone breakdown. A milestone is a coherent slice of
   work that lands a self-contained capability:
   - **payload types** -- one milestone covering all `src/model/*`
     payload structs derived from `data-movement.md`.
   - **module skeletons + connectivity** -- modules from the
     decomposition stubbed out and wired per `pipeline-mapping.md`,
     elaboration succeeds.
   - **per-stage logic** -- one milestone per pipeline stage (or one
     per cluster of related stages) that fills in `evaluate()` for
     the modules in that stage.
   - **smoke + unit tests** -- elaboration test, basic data-flow
     test, backpressure test, idle-cycle test, and per-module unit
     tests for representative inputs.

   Do not include exhaustive verification (directed sequences,
   coverage targets, scoreboards) -- that belongs in DM3, NOT here.

3. For each milestone, list its tasks as a `[ ]`-prefixed bullet
   list. Each task should be one focused unit of work that DM2d can
   tick off when complete. Tasks should reference concrete
   artifacts -- file paths, module names, payload struct names --
   not vague phrases like "implement the pipeline".
4. Trace every operation in `decomposition.md` and every payload in
   `data-movement.md` to at least one task. Tasks the agent can't
   complete without making decisions outside the analysis -- e.g.
   "decide the buffer depth for stage X" -- belong in the plan as
   explicit decision tasks (or are flagged `OPEN:` for DM3 to
   resolve).
5. Make sure the plan accounts for target- and verification-sensitive
   implementation work where it materially affects DM2d:
   - gate-budget-sensitive stage structure or buffering decisions
   - smoke tests that exercise the critical liveness / backpressure /
     idle behavior implied by DM1
   - any observability or structural hooks DM3 will rely on later,
     when those hooks must be designed in during implementation
   Do not pre-empt full DM3 verification planning, but do not ignore
   DM1's strategy artifacts either.
6. Order milestones so that each one's tasks have all their
   dependencies in earlier milestones (payload types before modules,
   skeletons + connectivity before per-stage logic, logic before
   tests).

## Output

Per `docs/plan/plan-management.md`:

- `docs/plan/plan.md` -- the index. Brief overview, then a TOC
  pointing at each `milestone-NN-<name>.md`.
- `docs/plan/milestone-NN-<name>.md` -- one file per milestone with
  the milestone's task list (`[ ]` bullets).

Use two-digit milestone numbers (`milestone-01-payload-types.md`,
`milestone-02-skeletons.md`, etc.) so the directory sorts in plan
order.

## Constraints

- DO NOT write any source code. No `src/model/`, no `tests/`, no
  `Cargo.toml` edits. Plan files only.
- DO NOT cite specific framework APIs (`Module`, `HasLogic`,
  `ConnectivityPlan`, etc.). Those are DM2d's concern; here we
  describe WHAT will be built and IN WHAT ORDER, not HOW each piece
  is implemented.
- DO NOT pre-empt DV scope. If the analysis suggests a verification
  concern, leave it for DM3 (a single bullet noting "covered in DM3"
  is fine).
- Use `docs/targets.md` and `docs/testbench.md` to shape the plan where
  they affect implementation structure, but do not turn this into a
  full verification-plan step.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2c-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
