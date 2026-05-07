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

- `docs/impl-plan/plan-management.md` -- the plan-file conventions
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
     per cluster of related stages). The milestone NAMES the modules
     whose `evaluate()` body DM2d will write; it does NOT spell out
     the algorithm. Each task is one line: which module, which file,
     which input/output payloads. DM2d picks the algorithm. Acceptable
     task: `Implement evaluate() in src/model/avg_stage.rs (consumes
     pixel_in_a + pixel_in_b, produces averaged_pixel).` NOT
     acceptable: shift-and-mask recipes, intermediate variable names,
     loop-vs-vectorized choices, packing-format decisions.
   - **smoke + unit tests** -- elaboration test, basic data-flow
     test, plus any flow-control / idle-cycle tests **explicitly
     called out by `docs/testbench.md`**. Per-module unit tests for
     representative inputs. If the design is purely combinational
     with no ready/valid handshake or stall semantics, do NOT add
     backpressure or idle-cycle tests just because they're listed
     here -- write a one-line "RESOLVED: design has no flow-control
     surface, backpressure / idle tests do not apply" entry in the
     plan instead so the critique sees the deliberate choice.

   Do not include exhaustive verification (directed sequences,
   coverage targets, scoreboards) -- that belongs in DM3, NOT here.

3. For each milestone, list its tasks as a `[ ]`-prefixed bullet
   list. Each task should be one focused unit of work that DM2d can
   tick off when complete. Tasks should reference concrete
   artifacts -- file paths, module names, payload struct names --
   not vague phrases like "implement the pipeline".
4. Trace every operation in `decomposition.md` and every payload in
   `data-movement.md` to at least one task. Tasks the agent can't
   complete without making decisions outside the analysis are
   FIRST-CLASS plan entries with this exact shape:

   `- [ ] DECIDE: <short question> -- options: <A | B | ...>; default: <pick>; rationale: <one line>.`

   Example:
   `- [ ] DECIDE: pixel-port type -- options: u32 | PixelRGBA; default: PixelRGBA; rationale: typed payload catches RGBA-vs-ABGR mistakes at the type level.`

   Decisions DM3 must resolve (rather than DM2d) get the same line
   shape with `OPEN:` instead of `DECIDE:`. Do NOT bury ambiguity in
   parenthetical asides like `(u32 or PixelRGBA?)` inside other
   tasks -- the critique will treat that as unresolved.
5. Make sure the plan accounts for target- and verification-sensitive
   implementation work where it materially affects DM2d:
   - gate-budget-sensitive stage structure or buffering decisions
   - flow-control / idle-cycle smoke tests **only when
     `docs/testbench.md` explicitly defines them** for this design;
     otherwise the RESOLVED entry described above is the correct
     output.
   - **internal-signal observability**: read `docs/testbench.md` and
     translate every "observe internal register / signal" requirement
     into a concrete task naming the signal and the module that
     exposes it (e.g. `Expose pipeline-register avg_to_gray on
     AvgStage for testbench observation`). Do NOT specify the
     framework mechanism (`SignalTrace`, test-only output ports,
     etc.) -- that's DM2d's choice. The plan only names WHAT must be
     observable.
   Do not pre-empt full DM3 verification planning, but do not ignore
   DM1's strategy artifacts either.
6. Order milestones so that each one's tasks have all their
   dependencies in earlier milestones (payload types before modules,
   skeletons + connectivity before per-stage logic, logic before
   tests).

## Output

Per `docs/impl-plan/plan-management.md`:

- `docs/impl-plan/plan.md` -- the index. Brief overview, then a TOC
  pointing at each `milestone-NN-<name>.md`.
- `docs/impl-plan/milestone-NN-<name>.md` -- one file per milestone with
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
