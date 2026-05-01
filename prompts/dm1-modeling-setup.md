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

1. Read `docs/spec.md`.
2. Read the spec for explicit requirements, implied requirements,
   `## Open Questions`, and any `## Auto-decisions`.
   - Preserve explicit requirements faithfully.
   - If the spec leaves something genuinely unconstrained, say so; do
     not invent false precision.
   - If a target or strategy choice depends on an open question or
     non-trivial assumption, call that out explicitly.
3. Create `docs/targets.md` as the target-and-metrics strategy document.
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
4. `docs/targets.md` must include a gate-budget-per-cycle target or
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
5. `docs/targets.md` should capture the modeling strategy, not just raw
   numbers.
   - Prefer quantitative targets when the spec supports them.
   - When the spec does not support a hard number, use a bounded or
     qualitative target if that is the most faithful interpretation.
   - Mark area and power as `unconstrained` or `deferred` unless the
     spec actually makes them part of the intended model contract.
   - Add any design-specific targets the spec implies, such as
     injection rate, occupancy limits, fairness, ordering guarantees,
     deadlock freedom, or backpressure SLAs.
6. Create `docs/testbench.md` as the verification-strategy and
   testbench-architecture document. This file should describe what the
   eventual testbench must be capable of proving, not the detailed test
   plan.
7. In `docs/testbench.md`, include:
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
   - **target traceability**: which parts of the testbench strategy
     support which items in `docs/targets.md`
8. Keep the two files distinct:
   - `docs/targets.md` explains what must be achieved or measured
   - `docs/testbench.md` explains how the later verification environment
     will be structured to validate those things
9. Do not write detailed directed tests, sequence implementations, or
   exhaustive test matrices here. That belongs in DM3.

## Output

- `docs/targets.md` at the project root.
- `docs/testbench.md` at the project root.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM1-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
