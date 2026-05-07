# DM4ad - Performance Plan, Detail (work session)

You are executing step DM4ad (Performance Analysis Plan, DETAIL)
of the Direct Modeling Flow. Prerequisite: DM4a gate passed.

## Goal

Walk each `docs/perf-plan/perf-milestone-NN-<name>.md` STUB
written by DM4a and replace its `<!-- detail-pending -->`
placeholder with the full task list per
`docs/impl-plan/plan-management.md`. The orchestrator scopes
you to ONE milestone per work + critique session.

## Inputs

The orchestrator inlines:

- `docs/perf-plan/perf-plan.md` -- the index (run-id scheme,
  workload registry, scope blurbs).
- The CURRENT milestone stub file you are detailing this turn.

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state
  conventions.
- `docs/spec.md`, `docs/targets.md`,
  `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/test-plan/test-plan.md` -- only the sections the
  current stub's Trace points at.

## Procedure

1. Open the current milestone stub. Read its Scope, Workloads,
   and Trace sections -- DM4a's contract.

2. Replace the `## Tasks` section's
   `<!-- detail-pending: ... -->` comment with a real task list.

   Each task names a CONCRETE artifact: a run-id, a sweep
   config path, a metric extraction, a report file path. Vague
   tasks ("measure performance") are not acceptable.

   Typical task shapes by milestone type:

   - **Baseline measurement**:
     ```markdown
     - [ ] Run baseline workload `<name>` -> record run-id
       `baseline-<name>` in `.sim-flow/experiments.db`
       - cmd: `cargo run -- --run-id baseline-<name>`
       - metrics captured: throughput / latency p50, p90, p99
       - traces to: targets.md row "<target>"
     ```
   - **Parameter sweep**:
     ```markdown
     - [ ] Sweep parameter `<param>` over values `[<v1>, <v2>,
       ...]` via `sim-flow sweep <sweep.toml>`
       - run-id pattern: `sweep-<param>-<value>`
       - traces to: targets.md row "<target>" / spec.md
         §"<workload assumption>"
     ```
   - **Bottleneck analysis**:
     ```markdown
     - [ ] Per-module utilization: extract stall count for
       `<module>` from `baseline-<workload>` run -> record in
       `docs/analysis/bottlenecks.md`
       - traces to: decomposition.md `<module>`
     ```
   - **Target verification**:
     ```markdown
     - [ ] Verify target row "<target>" against measurement
       from `baseline-<workload>` -> record `target met`
       / `target not met` + run-id in
       `docs/analysis/target-verification.md`
       - traces to: targets.md row "<target>"
     ```
   - **Reporting**:
     ```markdown
     - [ ] Write `docs/analysis/<topic>.md` -- summary table
       (measured-vs-target) + bottleneck identification +
       run-id citations + p50/p90/p99 distributions where
       applicable
     ```

3. **Cap at ~10 tasks per milestone**. If you'd need more,
   surface a structural concern in `## Auto-decisions`. The
   outline step should have split the milestone if it's too
   big.

4. **Run-id discipline**. Every run-recording task names a
   run-id matching the scheme in `perf-plan.md`'s Run-ID
   scheme section. DM4b uses these deterministically.

5. **Target verification rows MUST include a target-met / not-
   met disposition**: tasks for the Target verification
   milestone explicitly state how the disposition is recorded
   so DM4b can fill in the actual outcome.

6. Add a `## Auto-decisions` trailing section recording any
   structural choices.

7. Once the task list is in place, the placeholder marker
   (`<!-- detail-pending -->`) MUST be gone.

8. Surface the canonical milestone-complete notice:

   > `<milestone-name> complete; ready for critique.`
   > `<one-line summary: count of tasks added, decisions
   > flagged, deferred items>`

## Constraints

- DO NOT run simulations or sweeps; planning only.
- DO NOT modify other milestone stubs.
- DO NOT modify `docs/perf-plan/perf-plan.md`. The outline step
  (DM4a) owns it.
- DO NOT change the run-id scheme; if it's wrong, flag in
  `## Auto-decisions`.
- DO NOT remove the placeholder by leaving `## Tasks` empty.

When the current milestone is fully detailed, stop. Do not
write `docs/critiques/DM4ad-critique.md`. Do not `/exit` on
your own.
