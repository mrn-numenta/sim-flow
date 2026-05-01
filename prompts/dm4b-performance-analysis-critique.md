# DM4b - Performance Analysis (critique session)

You are reviewing the DM4b performance-analysis results. Treat
them as work produced by a third party even if you produced them
yourself earlier in this conversation -- the independent-review
property depends on you bracketing any prior reasoning rather than
leaning on it. Do not modify the analysis artifacts; evaluate them
and write the critique file.

## Inputs

- `docs/plan/perf-plan.md` -- the plan; check that every
  `- [ ]` is now `- [x]` (or documented as deferred), and that
  run-ids cited in milestones tie back to rows in
  `.sim-flow/experiments.db`.
- `docs/plan/perf-milestone-*.md` -- per-milestone task lists.
- `docs/targets.md`
- `docs/analysis/` report markdown
- `.sim-flow/experiments.db` (the experiments index) if populated

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (the flow cannot
finish until fixed). Prefix informational notes with
`UNRESOLVED:`. The orchestrator fails the DM4b gate on `BLOCKER:`
lines only. If experiment tracking infrastructure is not yet
available (Phase 4 not landed), emit
`BLOCKER: experiment tracking unavailable (Phase 4 pending)`
and stop.

1. **Plan completion**. Is every task in the
   `perf-milestone-NN-*.md` files either `[x]` or documented as
   deferred with a reason? Reject silently-skipped tasks.
2. **Target coverage**. Is every row in `docs/targets.md`
   measured and reported? Each target should map to a specific
   run-id (or sweep cell) cited in the report.
3. **Bottleneck evidence**. Are bottlenecks identified with
   supporting evidence (per-module stall counts, queue
   occupancy, link utilization), not just speculation?
4. **Sweep sanity**. Do sweep results pass a sanity check
   (monotonicity where expected, physically plausible numbers)?
   Reject "the numbers are what they are" without a check.
5. **Spec consistency**. Are any results inconsistent with
   `docs/spec.md` or the intended behavior? Inconsistencies
   that aren't called out in the report are `BLOCKER:`-eligible.
6. **Distributions**. Does the report use distributions (p50 /
   p90 / p99) where appropriate, not just scalar summaries?
7. **Experiment-index linkage**. Is there at least one row in
   `.sim-flow/experiments.db` for this project? Do the run-ids
   in the reports match rows in the index, and do they follow
   the naming scheme declared in `perf-plan.md`?
8. **Milestone stop-points**. Did DM4b honor the milestone stop
   points (one stop per milestone boundary), or did it chain
   straight through? The user is meant to review between
   milestones; chaining them silently regresses the workflow.
9. **Report-per-topic structure**. Is `docs/analysis/` organized
   by topic (throughput, latency, sweeps, bottlenecks) rather
   than one mega-report?
10. **Plan fidelity**. If the analysis deviated from
    `docs/plan/perf-plan.md` (skipped milestones, renamed
    run-ids, ignored bottleneck modules), flag every deviation.

## Output

Write `docs/critiques/DM4b-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
