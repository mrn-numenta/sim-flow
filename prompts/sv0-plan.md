# SV0 - SystemVerilog Conversion Plan (work session)

You are executing step SV0 of the SystemVerilog Conversion Flow.
This is the OUTLINE step: produce the index `generated/plan.md`
plus per-milestone STUB files under `generated/plan/`. The detail
step (SV0d) fills in each stub's task list; the RTL emission (SV1)
and UVM emission (SV2) steps then walk those tasks.

Do NOT write any `.sv` files in this session.

## Inputs

Read these before writing the plan:

- `docs/spec.md` -- architectural intent, clock, gates/cycle.
- `docs/analysis/decomposition.md` -- operations / stages.
- `docs/analysis/pipeline-mapping.md` -- module-to-stage layout.
- `docs/analysis/data-movement.md` -- inter-stage data edges.
- `docs/test-plan/test-plan.md` -- verification contract DM3a wrote.
- `src/model/` -- the Foundation model. Read each module
  implementing `Module + HasLogic + HasInstances` or
  `SimpleModule` to learn its ports, payload, state, and
  evaluate/settle/update semantics.
- `src/sim.rs` (if present) -- top-level wiring.
- `tests/testbench/` and `tests/` -- the UVM-lite testbench
  scaffolding DM3b/DM3c produced.
- `lib:docs/modeling-guide/06-design-patterns.md` -- hardware
  pattern taxonomy you will classify each module against.

## Procedure

1. **Inventory and classify every Foundation module.** For each
   `src/model/<name>.rs`, decide which hardware pattern applies:
   - simple pipeline / transform stage
   - custom stateful / hazard stage
   - feedback / kill / redirect stage
   - FIFO / queue / CDC / library wrapper
   - other (justify in plan.md)

2. **Write `generated/plan.md`** (the index). Include:
   - Title + 1-paragraph summary of the conversion strategy.
   - **Module table**: one row per Foundation module with its
     hardware pattern, target RTL file name, key ports / state.
   - **RTL milestone table** (one entry per
     `generated/plan/rtl-milestone-NN-<slug>.md` you will stub
     below). Each row: filename, 1-2 sentence scope, files
     produced (e.g. `generated/rtl/avg_stage.sv`).
   - **UVM milestone table** (one entry per
     `generated/plan/uvm-milestone-NN-<slug>.md`). Each row:
     filename, scope, files produced (e.g.
     `generated/test/uvm_avg_drv.sv`).
   - **Traceability**: every functional row in
     `docs/test-plan/test-plan.md` maps to a UVM milestone (by
     filename) and ultimately a UVM test file. No row goes
     unmapped.

3. **Write one stub per milestone** at
   `generated/plan/<area>-milestone-NN-<slug>.md`. Each stub uses
   this template VERBATIM:

   ```markdown
   # <area>-Milestone NN: <Name>

   ## Scope

   <1-paragraph description of what this milestone delivers --
   which Foundation module(s) / which testbench role / which
   verification contract; ends with the acceptance criterion.>

   ## Components / Files

   <For rtl-milestone: list the `.sv` files this milestone
   produces under `generated/rtl/`. Name the Foundation module
   each derives from. For uvm-milestone: list the `.sv` files
   produced under `generated/test/`, the testbench role each
   plays, and any sequences they implement.>

   ## Trace

   - src/model/<file>.rs::<symbol> for RTL milestones (Foundation
     module / type the SV is derived from)
   - tests/testbench/<file>.rs::<symbol> for UVM milestones
     (Rust-side testbench role)
   - docs/test-plan/test-plan.md row(s) for UVM-test milestones
   - docs/spec.md / docs/analysis/*.md sections that constrain
     behavior preserved by this milestone

   ## Tasks

   <!-- detail-pending: SV0d replaces this section with the full
   task list. The scope + components + trace above are the
   contract this milestone delivers; SV0d expands into concrete
   `- [ ]` rows naming files and per-file tasks. -->
   ```

   The literal `<!-- detail-pending -->` is load-bearing: SV0d's
   gate fails until every stub has been detailed (placeholder
   removed). Keep it verbatim. Two-digit zero-padded NN. Letter
   suffix for split milestones (`02a`, `02b`).

4. **Naming convention**:
   - `rtl-milestone-01-payloads.md` -- shared packed structs
     (always milestone 01 in the RTL area).
   - `rtl-milestone-02..NN-<module>.md` -- per Foundation module.
   - `rtl-milestone-NN-top.md` -- top-level wiring (always the
     last RTL milestone).
   - `uvm-milestone-01-types-and-interfaces.md` -- common
     typedefs + virtual interfaces.
   - `uvm-milestone-02..NN-<role>.md` -- driver / monitor /
     scoreboard / env / per-test slices.
   - `uvm-milestone-NN-tests.md` -- per-test files (one milestone
     can hold multiple closely-related test files).

5. **No more than 10 milestones per area** (RTL or UVM). Split if
   you exceed the cap; note the rationale in plan.md.

## Output

{{ output_intro }}

- `generated/plan.md` -- the index, with module + milestone +
  trace tables.
- `generated/plan/rtl-milestone-NN-<slug>.md` -- one stub per
  RTL milestone.
- `generated/plan/uvm-milestone-NN-<slug>.md` -- one stub per
  UVM milestone.

## Constraints

- DO NOT write `.sv` files. RTL and UVM emission belong to SV1
  and SV2.
- DO NOT remove the `<!-- detail-pending -->` placeholder from
  any milestone stub (SV0d's gate keys on its absence).
- DO NOT touch anything outside `generated/`.
- Every Foundation module in `src/model/` must appear in
  plan.md's module table AND in some RTL milestone's Components
  / Files section.
- Every row in `docs/test-plan/test-plan.md` must trace to a UVM
  milestone (by filename) in plan.md's traceability section.

When the plan and all stubs are in place, surface:

> SV0 conversion plan complete; ready for critique.
> <one-line summary: count of modules classified, RTL milestones,
> UVM milestones, traceability rows mapped>

Do NOT write `docs/critiques/SV0-critique.md`. Do not `/exit`.
