# DM4ad - Performance Plan, Detail (critique session)

You are reviewing one milestone of the DM4ad perf-plan DETAIL
step. The orchestrator scopes you to ONE milestone file per
critique session -- the same one the work session just
expanded.

## Inputs

The orchestrator inlines:

- `docs/perf-plan/perf-plan.md` -- the index (for context).
- The current detailed milestone file (the one you're
  reviewing).

Read on demand:

- `docs/impl-plan/plan-management.md` -- task / state
  conventions.
- `docs/spec.md`, `docs/targets.md`,
  `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/test-plan/test-plan.md` -- only sections the milestone
  Trace points at.

## Evaluation

{{ critique_kinds }}

This critique reviews ONE milestone's task list.

1. Is the `<!-- detail-pending -->` placeholder GONE? If still
   present, work session didn't land -- `BLOCKER:` and stop.
2. Does the `## Tasks` section now contain real `- [ ]` rows?
   Empty section = `BLOCKER:`.
3. Does each task name a CONCRETE artifact: a run-id, a sweep
   config path, a metric extraction with output location, or
   a report file path? Vague tasks ("measure performance",
   "analyze bottlenecks") = `BLOCKER:`.
4. Do run-ids match the scheme pinned in `perf-plan.md`'s
   Run-ID scheme section (e.g.
   `baseline-<workload>` / `sweep-<param>-<value>`)? Tasks
   with ad-hoc run-ids = `BLOCKER:`.
5. For Target verification milestones: does every task spell
   out how the target-met / not-met disposition is recorded
   (which file, which run-id citation)? Missing disposition
   recording = `BLOCKER:`.
6. For sweeps: does each task name the parameter, the value
   range, and the sweep config path / `sim-flow sweep`
   invocation? Missing config = `BLOCKER:`.
7. For reporting milestones: does each report task name a
   specific output path under `docs/analysis/`, and require
   run-id citations for every measurement? Reports that don't
   trace numbers to run-ids = `BLOCKER:`.
8. Are tasks within scope of THIS milestone? Tasks belonging
   in a sibling milestone = `BLOCKER:`.
9. Is the task count ≤10? Overflows = `UNRESOLVED:`.
10. Does the task list trace cleanly to the milestone's Trace
    section? Tasks lacking link back to spec / target /
    decomposition entries = `UNRESOLVED:`; tasks contradicting
    the trace = `BLOCKER:`.
11. Does any task pre-empt DM4b implementation choices
    (specific runner internals, framework helper choices)?
    The plan describes WHAT measurements + reports; HOW each
    is computed is DM4b's discretion. `BLOCKER:` for
    prescription.
12. Are `## Auto-decisions` entries (when present) reasonable?
    Auto-decisions that contradict the outline's Scope /
    Workloads = `BLOCKER:`.

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM4ad-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM4ad-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}