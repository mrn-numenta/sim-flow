# DM1 - Modeling Setup (work session)

You are executing step DM1 (Modeling Setup) of the Direct Modeling Flow.
Prerequisite: DM0 gate passed. Don't critique your own output here;
that's the critique pass.

## Goal

Derive the modeling and verification strategy from `docs/spec.md`.
DM1 is about deciding what must be measured, what must be verified, and
what kind of testbench structure will be needed later. It is not the
step for detailed testbench implementation or detailed test planning.
DM3a will implement the testbench; DM3b and DM3c will flesh out the test
plan and tests; DM4 will measure against the targets.

## Procedure

1. Read the spec. It lands in one of two layouts (the gate accepts
   either):
   - Single file: `read_file("docs/spec.md")`.
   - Paginated: section files under `docs/spec/` (e.g.
     `docs/spec/01-overview.md`, ...). The system stack's TOC
     block lists every section; `list_dir("docs/spec/")` if you
     need to enumerate. Read each section in numeric order.
   For paginated specs, treat the union of section files as "the
   spec" for everything below; quote `<section-file>:<line>` when
   citing requirements.
2. Read the spec for explicit requirements, implied requirements,
   `## Open Questions`, and any `## Auto-decisions`.
   - Preserve explicit requirements faithfully.
   - If the spec leaves something genuinely unconstrained, say so; do
     not invent false precision.
   - If a target or strategy choice depends on an open question or
     non-trivial assumption, call that out explicitly.
3. Check whether `docs/targets.md` and `docs/testbench.md` exist.
   - If yes, review them against `docs/targets.md.tmpl` and
     `docs/testbench.md.tmpl` and fill in any missing or incomplete
     sections.
   - If no, copy `docs/targets.md.tmpl` to `docs/targets.md` and
     `docs/testbench.md.tmpl` to `docs/testbench.md`, then use those
     templates as the required structure for this step.
4. Create `docs/targets.md` as the target-and-metrics strategy document.
   For each target, record:
   - **name**
   - **category**: throughput, latency, functional correctness,
     ordering, fairness, deadlock freedom, backpressure behavior, area,
     power, or other design-specific category
   - **status**: explicit, derived, inferred, unconstrained, or deferred
   - **target statement**: the actual quantitative or qualitative goal
   - **units / measurement method**, when applicable
   - **source**: cite the relevant `docs/spec.md` section
   - **notes / rationale**: especially when the target is derived,
     inferred, unconstrained, or deferred
5. `docs/targets.md` must include a gate-budget-per-cycle target or
   estimate. This requirement is hard because DM2 uses it to reason
   about functional decomposition and pipeline staging.
   - If `docs/spec.md` gives an explicit gate budget per cycle, preserve
     it as an explicit target.
   - Otherwise, derive a reasonable gate-budget-per-cycle estimate from
     the frequency and technology target in `docs/spec.md`.
   - Record whether the value is explicit, derived, or inferred, and
     explain the basis for the estimate.
   - If `docs/spec.md` does not provide enough information to derive a
     reasonable estimate, treat that as a blocker-level gap in the DM0
     output and say so in your rationale rather than inventing false
     precision.
6. `docs/targets.md` should capture the modeling strategy, not just raw
   numbers.
   - Prefer quantitative targets when the spec supports them.
   - When the spec does not support a hard number, use a bounded or
     qualitative target if that is the most faithful interpretation.
   - Mark area and power as `unconstrained` or `deferred` unless the
     spec actually makes them part of the intended model contract.
   - Add any design-specific targets the spec implies, such as
     injection rate, occupancy limits, fairness, ordering guarantees,
     deadlock freedom, or backpressure SLAs.
7. Create `docs/testbench.md` as the verification-strategy and
   testbench-architecture document. This file should describe what the
   eventual testbench must be capable of proving, not the detailed test
   plan.
8. In `docs/testbench.md`, include:
   - **verification scope**: what behaviors and guarantees from the spec
     must be checked
   - **coverage intent**: what classes of behavior must be exercised
     later (normal flow, corner cases, reset/init, stalls, backpressure,
     arbitration, error handling, performance scenarios)
   - **stimulus strategy**: the kinds of traffic or scenario families
     the future testbench will need
   - **observability strategy**: what internal or external behavior must
     be observed to determine correctness
   - **checking strategy**: what scoreboards, assertions, reference
     models, or invariants will likely be needed
   - **testbench architecture**: the likely Sequencers, Drivers,
     Monitors, Agents, Envs, and Scoreboards, mapped to the interfaces
     and behaviors in `docs/spec.md`
   - **implementation baseline**: name the closest-matching worked
     example -- `lib:examples/<NN-name>/test/` -- whose UVM-lite
     layout this testbench will mirror at DM3b. Browse
     `lib:examples/README.md`, then read the `tests/` of two or three
     candidates whose port shape and stage count match the design;
     pick the closest. Record the chosen baseline path, a one-line
     rationale (why the topology / port shape matches), and the
     expected adaptations DM3b will need (extra Monitors / Scoreboards
     for design-specific invariants, renamed components, etc.).
     DM3b copies that example's `test/` file structure as a starting
     point and adapts the component bodies; getting this right at DM1
     saves DM3b from re-deriving the choice from spec under a tighter
     context budget.
   - **target traceability**: which parts of the testbench strategy
     support which items in `docs/targets.md`
9. Keep the two files distinct:
   - `docs/targets.md` explains what must be achieved or measured
   - `docs/testbench.md` explains how the later verification environment
     will be structured to validate those things
10. Use the template headings as the required document structure, but use
    engineering judgement about depth. Remove placeholder text as you
    replace it with real content. If a section truly does not apply, say
    so explicitly rather than leaving placeholder text in place.
11. Do not write detailed directed tests, sequence implementations, or
   exhaustive test matrices here. That belongs in DM3.

## Output

{{ output_intro }}

`docs/targets.md` supports the same dual layout as the spec:

- **Single-file:** `docs/targets.md` at the project root. Use this
  when the target set is small (rough rule: under ~500 lines).
- **Paginated:** a directory `docs/targets/` containing numbered
  section files (`docs/targets/01-throughput.md`,
  `docs/targets/02-area-power.md`, ...). Use this when a big design
  has many target categories. The numbered prefix is the canonical
  reading order; the slug is free-form. Each file holds one
  self-contained section. The orchestrator's predecessor TOC lists
  every section file with size so downstream steps can `read_file`
  the specific ones they need.

`docs/testbench.md` stays single-file (it's a fixed structure --
named components + verification strategy).

Either targets layout is the input to every later DM step.
**Pick one layout per project and stick with it** -- mixing a
populated `docs/targets.md` with a populated `docs/targets/`
confuses downstream readers.

Output artifacts:

- `docs/targets.md` OR `docs/targets/<NN>-<slug>.md` files.
- `docs/testbench.md`.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM1-critique.json`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
