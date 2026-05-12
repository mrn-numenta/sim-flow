# DM4a - Performance Plan, Outline (critique session)

You are reviewing the DM4a performance-plan OUTLINE. The outline
is the contract DM4ad will detail against; gaps here propagate
as missing or mis-shaped milestones.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions.
- `docs/perf-plan/perf-plan.md` -- the outline index.
- `docs/perf-plan/perf-milestone-NN-*.md` -- one stub per
  milestone (Scope / Workloads / Trace +
  `<!-- detail-pending -->` placeholder).
- `docs/spec.md`
- `docs/targets.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/test-plan/test-plan.md`

## Evaluation

{{ critique_kinds }}

This critique reviews the OUTLINE, not per-milestone task
lists. Resist reviewing content in `<!-- detail-pending -->`
placeholders; flag the ABSENCE of one as a `BLOCKER:`.

1. **Index file** (`perf-plan.md`):
   - Has a perf-analysis strategy overview.
   - Has a `## Run-ID scheme` section pinning the run-id
     naming convention. Missing = `BLOCKER:` (DM4ad needs it
     to write run-recording tasks).
   - Has a `## Workload registry` listing workloads + their
     source + which target rows they support. Missing =
     `BLOCKER:`.
   - Has a TOC pointing at every `perf-milestone-NN-*.md`.
   - Each TOC entry has a 1-2 sentence scope blurb.
2. **Stub structure** (every `perf-milestone-NN-*.md`):
   - Contains the literal `<!-- detail-pending -->` marker.
     Missing = `BLOCKER:` (DM4ad's gate keys on it).
   - Has Scope, Workloads/Sweeps, Trace sections.
   - Scope blurb names the milestone's slice and acceptance
     criterion (NOT a task list).
   - Workloads section references registry entries from the
     index (rather than redefining them).
   - Trace section points at SPECIFIC entries in spec.md /
     targets.md / decomposition.md / test-plan.md. Vague
     trace = `UNRESOLVED:`; missing trace entirely = `BLOCKER:`.
3. **Milestone breakdown coverage**:
   - Baseline measurement milestone present.
   - Bottleneck analysis milestone present (or explicit
     `RESOLVED:` line saying the design has only one stage so
     bottleneck analysis is trivial).
   - Target verification milestone present and traces every
     row in `docs/targets.md`. Missing target rows =
     `BLOCKER:` (every target must be verified).
   - Reporting milestone present.
   - Parameter-sweep milestone (or `RESOLVED:` note: design
     has no parameters).
4. **Run-ID scheme**:
   - Names a deterministic naming pattern (e.g.
     `baseline-<workload>` / `sweep-<param>-<value>`).
   - Stable enough that DM4ad can pre-fill run-ids in tasks.
5. **Stub leakage**: does ANY stub leak per-task content
   (concrete `- [ ]` rows, specific cargo-run invocations,
   specific metric extractions) into its Scope / Workloads /
   Trace? That's the detail step's job. Mark `BLOCKER:`.
6. **Pre-empting DM4b**: does the outline prescribe specific
   tooling internals (runner-helper names, framework calls)?
   The plan describes WHAT will be measured / reported; HOW
   is DM4b's choice. Mark `BLOCKER:` for prescription.

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM4a-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM4a-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}