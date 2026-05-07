# DM4b - Performance Analysis (critique session)

You are reviewing the DM4b performance-analysis results. Treat
them as work produced by a third party even if you produced them
yourself earlier in this conversation -- the independent-review
property depends on you bracketing any prior reasoning rather than
leaning on it. Do not modify the analysis artifacts; evaluate them
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

Prefix gate-blocking issues with `BLOCKER:` (the flow cannot
finish until fixed). Prefix informational notes with
`UNRESOLVED:`. The orchestrator fails the DM4b gate on `BLOCKER:`
lines only. If experiment tracking infrastructure is not yet
available (Phase 4 not landed), emit
`BLOCKER: experiment tracking unavailable (Phase 4 pending)`
and stop.

Record findings in the critique JSON (see "Output" below for the
schema). `kind: "blocker"` blocks the gate; `"unresolved"` is
informational; `"resolved"` is historical / retry-mode.

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

12. **Coding Requirements (per the work prompt)**. For any
    Rust helpers / sweep glue / scratch binaries DM4b wrote
    AND for the markdown reports under `docs/analysis/`:
    - **Idiomatic Rust** in code: manual loops where iterators
      fit, `unwrap()` in non-test paths, nested `if let` where
      `match` would read better -> `BLOCKER:`.
    - **Magic numbers / strings**: workload identifiers,
      run-id patterns, threshold values, p50/p90/p99 cutoffs,
      etc. -- all named (`const`, enum variant, named struct
      field), not inlined -> any inlined literal is `BLOCKER:`.
    - **No emojis** in Rust code, markdown reports, or log
      output. Any decorative glyph in `docs/analysis/*.md` ->
      `BLOCKER:`.
    - **File size cap: under 400 lines** for every Rust file
      AND every report markdown under `docs/analysis/`. Any
      file at or above 400 lines -> `BLOCKER:`. Reports get
      split along natural axes (per-workload sections, per-
      topic files like `throughput.md` + `latency.md`) rather
      than growing one mega-report.

## Output

Write the critique as JSON to
`docs/critiques/DM4b-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM4b-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

### JSON schema

```json
{
  "step": "DM4b",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"`, `"unresolved"`, `"resolved"`. Schema
is strict (`deny_unknown_fields`); typos fail the parse.
