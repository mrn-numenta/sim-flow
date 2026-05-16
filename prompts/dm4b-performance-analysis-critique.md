# DM4b - Performance Analysis (critique session)

You are reviewing the DM4b performance-analysis results. {{ third_party_reviewer_note }} Do not modify the analysis artifacts; evaluate them
and write the critique file.

This critique runs more than once:

- after each milestone-complete checkpoint, to validate the newly
  landed runs / measurements / reports before the next milestone
  begins
- once after the final milestone, as the end-to-end performance
  analysis and reporting review

Determine which milestone was just completed from the plan files,
review that milestone in detail, and also sanity-check that the
new work did not regress earlier milestones.

## Inputs

- `docs/perf-plan/perf-plan.md` -- the plan; check that every
  `- [ ]` is now `- [x]` (or documented as deferred), and that
  run-ids cited in milestones tie back to rows in
  `.sim-flow/experiments.db`.
- `docs/perf-plan/perf-milestone-*.md` -- per-milestone task lists.
- `docs/spec.md` -- workload assumptions and intended behavior;
  needed to judge whether the reported numbers are consistent
  with the design intent (check 5).
- `docs/targets.md`
- `docs/analysis/` report markdown
- `.sim-flow/experiments.db` (the experiments index) if populated

## Evaluation

{{ critique_kinds }}

Experiment tracking is in place: the `record_run` agent tool
writes to `.sim-flow/experiments.db` (the database is created on
first call). Do not emit a "Phase 4 pending" blocker; if the
database is missing or empty, the agent skipped its
`record_run` discipline -- raise a `"blocker"` finding whose
title names the specific cited run-ids that don't appear, with a
body explaining that `record_run` must follow each `cargo run`.

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
8. **Milestone stop-points (proxy check)**. The artifacts alone
   can't tell whether DM4b paused at each milestone boundary or
   chained them in one session, but a structural proxy works:
   does each `perf-milestone-NN-*.md` file have its task rows
   ticked off in roughly chronological order with the run-ids
   they cite landing in `.sim-flow/experiments.db` in matching
   order, or do all rows flip in one burst at the end? An
   all-at-once flip (every milestone's last-task timestamp
   close together; no per-milestone summary notice in the
   transcript) is a `BLOCKER:` because the workflow's
   milestone-by-milestone critique signal was bypassed.
9. **Report-per-topic structure**. Is `docs/analysis/` organized
   by topic (throughput, latency, sweeps, bottlenecks) rather
   than one mega-report?
10. **Plan fidelity**. If the analysis deviated from
    `docs/perf-plan/perf-plan.md` (skipped milestones, renamed
    run-ids, ignored bottleneck modules), flag every deviation.
11. **Checkpoint discipline**. If this is a milestone critique
    rather than the final DM4b review, is the just-completed
    milestone solid enough that the next milestone can safely
    build on it? If this is the final review, do the milestone-
    local runs and reports compose into a coherent end-to-end
    performance story without regression?

12. **Coding Requirements (per the work prompt)**. Inspect any
    Rust helpers / sweep glue / scratch binaries DM4b wrote.
    The same 400-line cap and no-emojis rule extend to the
    markdown reports under `docs/analysis/`; flag any report
    at or above 400 lines (split per-topic) or carrying
    decorative glyphs as `BLOCKER:`.
{{ coding_requirements_checks }}

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM4b-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM4b-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}
