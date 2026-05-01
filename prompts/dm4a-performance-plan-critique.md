# DM4a - Performance Analysis Plan (critique session)

You are reviewing the DM4a performance-analysis plan. Treat it as
work produced by a third party even if you produced it yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. The plan is the contract DM4b will execute against; gaps
here propagate as missing measurements or under-cited reports. Do
not modify the plan; evaluate it and write the critique file.

## Inputs

- `docs/plan/plan-management.md` -- plan-file conventions.
- `docs/plan/perf-plan.md` -- plan index + TOC.
- `docs/plan/perf-milestone-*.md` -- per-milestone task lists.
- `docs/spec.md`
- `docs/targets.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/plan/test-plan.md`

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM4b cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM4a gate on `BLOCKER:` lines only.

1. Does `docs/plan/perf-plan.md` follow the conventions in
   `plan-management.md`? Is there an overview and a TOC pointing
   at each `perf-milestone-NN-<name>.md`?
2. Are milestones named `Milestone NN: <description>` and saved
   as `perf-milestone-NN-<name>.md`? Are the numbers contiguous?
   Is the `perf-` prefix used on milestone files (so they don't
   collide with DM2c's `milestone-NN-*.md`)?
3. **Required milestones present**. Does the plan cover (or
   explicitly justify dropping):
   - Baseline measurement (canonical workloads, run-ids,
     core metrics)
   - Parameter sweeps (or "no sweeps -- design is fixed" with
     rationale)
   - Bottleneck analysis (per-module / per-stage)
   - Target verification (one task per `docs/targets.md` row)
   - Reporting (`docs/analysis/<topic>.md` files)
4. **Task concreteness**. Is every task a `[ ]`-prefixed bullet
   that names a concrete artifact: a specific run-id, a sweep
   config, a metric extraction, or a `docs/analysis/<topic>.md`
   report path? Reject vague tasks like "measure performance",
   "do a sweep", "write a report".
5. **Target traceability**. Does every row of `docs/targets.md`
   map to at least one Target-verification task? Does each map
   also name the measurement task (run-id or sweep cell) that
   produces the number? Reject unmapped targets and vague
   mappings ("covered by overall analysis").
6. **Module / stage coverage**. Does the bottleneck-analysis
   milestone reference every non-trivial module from
   `docs/analysis/decomposition.md`? Per-module observations
   should be planned, not picked-up-as-we-go.
7. **Run-id scheme**. Is the run-id naming scheme documented and
   followed (e.g. `baseline-<workload>`, `sweep-<param>-<value>`)?
   Are run-ids stable enough that DM4b can re-run a single id
   without ambiguity?
8. **Milestone ordering**. Baseline before sweeps, sweeps before
   bottleneck analysis (analysis depends on sweep data), all
   measurement before reporting. Flag tasks whose dependencies
   live in later milestones.
9. **Stop-points for review**. Does the plan tell DM4b to stop
   for user review at each milestone boundary (rather than
   running the entire perf flow unattended)? Long perf runs are
   the exact place where the user wants checkpoints.
10. **Scope discipline**. Reject tasks that pre-empt DM4b's
    execution (specific scripts, embedded TOML, full report
    text). The plan describes WHAT will be measured and WHAT
    reports will be written, not HOW each measurement is run.

## Output

Write `docs/critiques/DM4a-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
