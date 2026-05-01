# DM4a - Performance Analysis Plan (work session)

You are executing step DM4a (Performance Analysis Plan) of the
Direct Modeling Flow. Prerequisite: DM3c gate passed.

## Goal

Produce a written performance-analysis plan that DM4b will execute.
**You do NOT run any simulations or write any analysis here.** The
plan is a milestone-driven sequence that DM4b walks through, ticking
off each task as the run / sweep / write-up completes. A clear plan
keeps DM4b focused: each task corresponds to a verifiable artifact
(an experiment run with a specific run-id, a metric extraction, a
sweep table, a `docs/analysis/<topic>.md` report).

## Inputs

Read these before writing the plan:

- `docs/plan/plan-management.md` -- the plan-file conventions
  (`plan.md` index + per-milestone files, milestone / task numbering,
  `[ ]` checkbox format).
- `docs/spec.md` -- the specification (workload assumptions,
  parameterization, design intent).
- `docs/targets.md` -- the quantitative targets every milestone
  must trace back to.
- `docs/analysis/decomposition.md` -- module list (for
  per-module utilization + bottleneck reporting).
- `docs/analysis/pipeline-mapping.md` -- pipeline shape
  (for stage-level stall / occupancy reporting).
- `docs/plan/test-plan.md` -- the verification surface; the
  Stress category there names the workloads that already exercise
  the targets and can usually be promoted into perf measurements.
- `src/`, `tests/` -- the model under test and the testbench
  scaffolding.

You don't need to read framework / library reference material here;
that's DM4b's concern when running experiments and writing reports.

## Procedure

1. Read each input above.
2. Decide the milestone breakdown. Use this skeleton; each entry is
   a milestone DM4b will work through in order:
   - **Baseline measurement** -- run the model on the canonical
     workload(s) named in `docs/spec.md`, record into
     `.sim-flow/experiments.db` with stable run-ids, capture the
     core metrics (throughput steady-state + transient, latency
     p50 / p90 / p99, per-module utilization, pipeline bubbles).
   - **Parameter sweep(s)** -- if the design is parameterizable,
     enumerate the parameters that should be swept and the ranges
     to cover. One sweep per parameter (or per coupled-parameter
     group). Use `sim-flow sweep <sweep.toml>`. If the design has
     no parameters, document this milestone as
     "no parameter sweeps -- design is fixed" and skip.
   - **Bottleneck analysis** -- per-module stall counts, queue
     occupancies, link utilization; identify the limiting stage
     and the next optimization lever for any target that's not
     met. Tasks here should reference per-module observations
     against the modules in `docs/analysis/decomposition.md`.
   - **Target verification** -- one task per row of
     `docs/targets.md` to confirm the measurement meets the
     target. A row that is NOT met must be called out as a
     `BLOCKER:`-eligible task so the report names it explicitly.
   - **Reporting** -- write the per-topic markdown reports to
     `docs/analysis/<topic>.md`. One report per major topic
     (throughput, latency, sweeps, bottlenecks). Each report
     references the run-ids that back its numbers so the data
     is reproducible.

   Drop or merge milestones only when the design genuinely doesn't
   need them (e.g. no sweeps); document the rationale in the plan.

3. For each milestone, list its tasks as a `[ ]`-prefixed bullet
   list. Each task names a concrete artifact: a run-id, a sweep
   config, a metric extraction, a report path. Vague tasks like
   "measure performance" are not acceptable.
4. **Trace every target**. Every row of `docs/targets.md` must map
   to at least one task in `Target verification` and to the
   measurement that produced its number (a run-id from
   `Baseline measurement` or a sweep cell from `Parameter sweeps`).
5. **Run-id discipline**. Plan the run-ids upfront so DM4b can use
   them deterministically: one run-id per workload + parameter
   combination, named like `baseline-<workload>` or
   `sweep-<param>-<value>`. Document the run-id naming scheme in
   the plan so each run-recording task in DM4b can reference it.
6. **Stop-points for user review**. Tell DM4b explicitly: after
   completing all tasks in a milestone (every `[ ]` is `[x]`),
   stop and surface a "milestone NN complete; ready for review"
   notice rather than rolling straight into milestone NN+1.

## Output

Per `docs/plan/plan-management.md`:

- `docs/plan/perf-plan.md` -- the index. Brief overview, then a
  TOC pointing at each `perf-milestone-NN-<name>.md`.
- `docs/plan/perf-milestone-NN-<name>.md` -- one file per
  milestone with the milestone's task list (`[ ]` bullets).

The `perf-` prefix on milestone files keeps them distinct from
DM2c's implementation milestones (`milestone-NN-*.md`) when both
trees coexist under `docs/plan/`.

## Constraints

- DO NOT run any simulations, sweeps, or `sim-flow record-run`
  invocations. No `cargo run`, no `sim-flow sweep`, no edits to
  `.sim-flow/experiments.db`. Plan files only.
- DO NOT write any `docs/analysis/<topic>.md` reports here -- that's
  DM4b. The plan describes what reports DM4b will produce and what
  evidence each must cite, not the report content itself.
- DO NOT prescribe specific tooling internals or coverage thresholds.
  Tooling choices that aren't pinned in `docs/spec.md` /
  `docs/targets.md` belong to DM4b's discretion within the plan.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM4a-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
