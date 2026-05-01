# DM4b - Performance Analysis (work session)

You are executing step DM4b (Performance Analysis) of the Direct
Modeling Flow. Prerequisite: DM4a gate passed. DM4b depends on
experiment tracking (Phase 4). If the tracking infrastructure is
not yet available, surface
`BLOCKER: experiment tracking unavailable` in the critique and
stop.

## Goal

Execute the performance-analysis plan written in DM4a -- run the
canonical workloads + sweeps, analyze bottlenecks, verify targets,
and produce per-topic reports under `docs/analysis/`. Every claim
in your reports must trace to a recorded experiment in
`.sim-flow/experiments.db`.

## Inputs

- `docs/plan/perf-plan.md` -- the milestone index. Read this first
  to orient.
- `docs/plan/perf-milestone-*.md` -- per-milestone task lists.
  Walk them in order.
- `docs/targets.md` -- the targets the plan traces back to.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md` -- module / stage names for
  bottleneck reporting.
- `docs/spec.md` -- workload assumptions.
- `src/`, `tests/` -- the model and testbench.

Reference material (read on demand):

- **Modeling guide -- observability + analysis chapter**:
  `lib:docs/modeling-guide/05-observability-and-analysis.md` for
  Foundation's metrics conventions and the
  `ObservabilityRunWriter` pattern.
- **Worked examples** with measurement: pick one whose targets
  shape matches yours from `lib:examples/`.

## Procedure

1. Read `docs/plan/perf-plan.md`. Then process each milestone in
   order from `docs/plan/perf-milestone-*.md`.
2. **For each milestone**:
   - Read its `perf-milestone-NN-<name>.md` file.
   - Work through tasks IN ORDER. As you complete each task,
     edit the milestone file to flip its `[ ]` to `[x]`. Don't
     skip ahead.
   - When you complete the LAST task in the milestone, **STOP**.
     Surface a clear notice -- `Milestone NN: <name> complete;
     ready for review.` followed by a one-line summary of what
     landed -- and wait for user input before starting the next
     milestone. Do NOT chain milestones automatically. The user
     reviews the recorded runs / reports between milestones.
3. **Run-record discipline**. Every `cargo run -- --run-id <id>`
   invocation should use a run-id that matches the scheme
   declared in `perf-plan.md` (typically `baseline-<workload>`
   or `sweep-<param>-<value>`). The orchestrator records each
   run into `.sim-flow/experiments.db`; check that the row is
   present before flipping the task to `[x]`.
4. **Sweep discipline**. Use `sim-flow sweep <sweep.toml>` for
   parameter sweeps. The sweep TOML should reference the run-id
   pattern from the plan. Don't roll your own loops over single
   runs when a sweep config does the same job.
5. **Reporting**. Each report under `docs/analysis/<topic>.md`
   must:
   - Open with a summary table of measured-vs-target metrics.
   - Identify bottlenecks with supporting evidence (per-module
     stall counts, link utilization, queue occupancy, NOT
     speculation).
   - Cite the run-ids that back every number, so the data is
     reproducible.
   - Use distributions (p50 / p90 / p99) where appropriate, not
     just scalar summaries.
   - Conclude with the next optimization lever for any target
     that's missed.
6. **Target verification milestone**. For each row of
   `docs/targets.md`, the corresponding task should record:
   `target met / not met` + the run-id that produced the
   measurement. Mark `BLOCKER:`-eligible items in the report
   prose so the critique can flag them.

## Order, jumping, and deferring

`docs/plan/plan-management.md` is the source of truth: task
states (`- [ ]` / `- [x]` / `- [-]` with `defer reason:`),
out-of-order work (`order swap:` sub-bullet), and additions
(`added:` sub-bullet). Read it before starting; the conventions
apply to perf-milestone task rows the same way they apply to
DM2c's implementation milestones.

DM4b-specific note: a deferred (`- [-]`) target-verification
row is allowed only when the target is genuinely out of scope
for this measurement run (e.g. "requires a workload not yet
written"). Deferring a target because the design misses it is
NOT acceptable -- leave the row `- [ ]` so the critique flags
it as a `BLOCKER:`.

## Constraints

- Stay inside the plan. If a task in `perf-plan.md` turns out
  to be wrong or impossible, flag the issue rather than
  silently deviating. The plan is the contract.
- Do NOT skip the milestone stop-points; the user is meant to
  review each one's runs / reports. Auto-mode should still
  honor the stop -- emit the "milestone complete" notice and
  let the user decide when to advance.
- Do NOT modify `docs/plan/perf-plan.md`'s structure (only
  flip `[ ]` to `[x]` and append run-id / measurement notes
  inside task lines). Re-architecting the plan is DM4a's job.

## Output

- `docs/analysis/` populated with per-topic report markdown.
- At least one experiment run recorded in
  `.sim-flow/experiments.db` for this project (typically many).
- `cargo run -- --run-id <id>` invocations visible in the run
  log; run-ids match the plan's scheme.
- Every task in every `perf-milestone-NN-*.md` is `[x]` (or
  documented as deferred with a reason).

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM4b-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
