# DM4a - Performance Analysis Plan, Outline (work session)

You are executing step DM4a (Performance Analysis Plan, OUTLINE)
of the Direct Modeling Flow. Prerequisite: DM3c gate passed.

## Goal

Produce the PERF-PLAN OUTLINE for DM4b. **You write the index
(`perf-plan.md`) plus a STUB file per milestone.** You do NOT
yet write the per-milestone task list -- that's DM4ad's job.

The split exists so a non-trivial design (with many parameters,
workloads, and target rows) can name its perf milestones in one
session even when the per-milestone task lists are too large to
fit alongside the spec / targets / decomposition in one prompt.

## Inputs

Read these before writing the outline:

- `docs/impl-plan/plan-management.md` -- plan-file conventions.
- `docs/spec.md` -- workload assumptions, parameterization,
  design intent.
- `docs/targets.md` -- the quantitative targets every milestone
  must trace back to.
- `docs/analysis/decomposition.md` -- module list (per-module
  utilization + bottleneck reporting).
- `docs/analysis/pipeline-mapping.md` -- pipeline shape
  (stall / occupancy reporting).
- `docs/test-plan/test-plan.md` -- the verification surface;
  Stress milestones name workloads usable as perf experiments.
- `src/`, `tests/` -- the model under test.

## Procedure

1. Read each input above.

2. If `docs/perf-plan/perf-plan.md` does not exist, copy
   `docs/perf-plan/perf-plan.md.tmpl` body verbatim into the
   live file with `write_file`, then edit. For each milestone
   stub use `docs/impl-plan/perf-milestone.md.tmpl` as a
   starting structure, but replace the templated `## Tasks`
   body with the `<!-- detail-pending -->` placeholder per
   step 4.

3. **Decide the milestone breakdown**. Use this skeleton; drop
   or merge only when the design genuinely doesn't need a
   milestone (document why in the index):

   - **Baseline measurement** -- canonical workload runs.
   - **Parameter sweep(s)** -- one milestone per parameter or
     per coupled-parameter group. Skip if the design has no
     parameters (note in the index).
   - **Bottleneck analysis** -- per-module / per-stage stall +
     queue + utilization measurements.
   - **Target verification** -- one milestone covering all
     `docs/targets.md` rows.
   - **Reporting** -- per-topic markdown reports under
     `docs/analysis/`.

4. **Write `docs/perf-plan/perf-plan.md`** (the index):

   - Brief overview of the perf-analysis strategy.
   - A `## Run-ID scheme` section pinning the run-id naming
     scheme upfront (e.g.
     `baseline-<workload>` / `sweep-<param>-<value>`). DM4b
     uses these deterministically.
   - A TOC pointing at every `perf-milestone-NN-<name>.md`.
   - A 1-2 sentence scope blurb per milestone in the TOC (NOT
     a task list -- the stub holds that placeholder).
   - **Workload registry**: list each baseline / stress /
     sweep workload, its source (test plan reference / spec
     section), and which target row(s) it supports. The
     workload registry is whole-plan context; stubs reference
     entries here rather than redefining them.

5. **Write one stub file per milestone** at
   `docs/perf-plan/perf-milestone-NN-<name>.md`. Each stub uses
   this template:

   ```markdown
   # Perf-Milestone NN: <Name>

   ## Scope

   <1-paragraph description: what this milestone measures /
   sweeps / verifies / reports, ending with the acceptance
   criterion (e.g. "every target row has a recorded
   measurement and target-met / not-met disposition").>

   ## Workloads / Sweeps

   <Reference workloads from the index's Workload registry by
   name (e.g. "uses workload `random-256-saturated` from
   the registry"). For sweeps, name the parameter and the
   value range.>

   ## Trace

   - spec.md: <which workload assumptions / design intent>
   - targets.md: <which target rows, if any>
   - decomposition.md / pipeline-mapping.md: <which modules /
     stages, for bottleneck milestones>
   - test-plan.md: <which stress milestone(s) the workload
     comes from, if applicable>

   ## Tasks

   <!-- detail-pending: DM4ad replaces this section with the full
   task list per `docs/impl-plan/plan-management.md`. The scope +
   workloads + trace above are the contract this milestone
   delivers; expand into concrete `- [ ]` rows naming run-ids,
   sweep configs, metric extractions, report paths. -->
   ```

   The literal `<!-- detail-pending -->` is load-bearing.

6. **Trace every target row** in `docs/targets.md` to at least
   one milestone (the Target verification milestone). The
   stub's Trace section names the target rows it covers.

## Output

**Use the path as the fence info-string, verbatim.** Opening
the fence with a language tag (`markdown`, `json`, `toml`, `rust`,
`yaml`, `text`, `md`, `rs`, `yml`, `txt`) means the body is
**silently dropped** -- the file never lands on disk, the gate
fails, and the work session burns its retry budget. See
`_conventions/fenced-blocks.md` ("Language-tag info-strings are
SILENTLY DROPPED") for the failure mode in detail. If you don't
remember the exact path, run `tool:read_file` / `tool:list_dir`
to discover it -- never guess `\`\`\`markdown` as a fallback.

A directory at `docs/perf-plan/` containing:

- `perf-plan.md` -- index with overview, run-id scheme,
  workload registry, TOC + scope blurbs.
- `perf-milestone-NN-<name>.md` -- one stub per milestone.

Gate-significant rules:

- `perf-plan.md` references at least one numbered milestone.
- At least one `perf-milestone-NN-*.md` stub exists.
- Every stub contains the literal `<!-- detail-pending -->`
  marker.

## Constraints

- DO NOT run simulations, sweeps, or `sim-flow record-run`
  invocations. Plan files only.
- DO NOT write `docs/analysis/<topic>.md` reports -- that's
  DM4b.
- DO NOT write the per-milestone task list -- DM4ad does that.
- DO NOT remove the `<!-- detail-pending -->` placeholder.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM4a-critique.md`; the critique is a distinct
task. Do not `/exit` on your own.
