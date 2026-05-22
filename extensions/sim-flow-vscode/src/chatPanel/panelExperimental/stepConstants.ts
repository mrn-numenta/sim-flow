/**
 * Step-label dictionaries used by the chat panel's step rail and
 * help popup. Pulled out of `panelExperimental.ts` so that file
 * stays under the 1000-line refactor threshold; the data is pure
 * (no module state) and lives here verbatim.
 */

/**
 * Step labels surfaced in the chat panel's step rail and footer-
 * adjacent help popup. Source: the dashboard's DM_STEPS array.
 */
export const STEP_LABELS: Record<string, string> = {
  DM0: "Spec",
  DM1: "Setup",
  DM2a: "Decomp",
  DM2b: "Pipeline",
  DM2c: "ImplPlan",
  DM2d: "Model",
  DM3a: "TestPlan",
  DM3b: "Bench",
  DM3c: "Tests",
  DM4a: "PerfPlan",
  DM4b: "Perf",
  DS0: "Spec",
  DS1: "Setup",
  DS2: "Decomp",
  DS3a: "Outline",
  DS3b: "Bench",
  DS3c: "Tests",
  DS4: "Screen",
  DS5: "Compare",
  SV0: "Plan",
  SV0d: "Detail",
  SV1: "RTL",
  SV2: "UVM",
  SV3: "Build",
};

/**
 * Full step labels surfaced on hover in the step rail. Replaces the
 * 3-4 character step id with a `<id>: <descriptive name>` form so
 * the user can mouse over any tile and read what it is without
 * leaving the chat panel. CSS shrinks the other tiles to make room.
 */
export const STEP_FULL_LABELS: Record<string, string> = {
  DM0: "DM0: Specification Intake",
  DM1: "DM1: Modeling Setup",
  DM2a: "DM2a: Design Decomposition",
  DM2b: "DM2b: Pipeline Mapping",
  DM2c: "DM2c: Implementation Plan",
  DM2d: "DM2d: Model Execution",
  DM3a: "DM3a: Test Plan",
  DM3b: "DM3b: Testbench Build",
  DM3c: "DM3c: Test Execution",
  DM4a: "DM4a: Performance Plan",
  DM4b: "DM4b: Performance Execution",
  SV0: "SV0: Verilog Plan",
  SV0d: "SV0d: Plan Detail",
  SV1: "SV1: RTL Emission",
  SV2: "SV2: UVM Emission",
  SV3: "SV3: Build & Validate",
};

/**
 * One-paragraph help text per step. Authored to be readable as a
 * standalone description; the help popup renders all entries in
 * the order returned by `stepOrderFor(flow)`. Keep these short --
 * the popup is a quick reference, not the canonical docs.
 */
export const STEP_DESCRIPTIONS: Record<string, { title: string; body: string }> = {
  DM0: {
    title: "DM0 — Specification",
    body: "Ingest the user-supplied spec (markdown or PDF) into `docs/spec.md` / `docs/spec/`. The agent asks clarifying questions until the spec declares a clock frequency and an explicit gates-per-cycle budget. Critique gate passes when no blockers remain.",
  },
  DM1: {
    title: "DM1 — Modeling Setup",
    body: "Translate the spec into engineering targets and pick a UVM-lite testbench shape. Outputs `docs/targets.md` (quantitative targets) and `docs/testbench.md` (sequencer / driver / monitor / scoreboard plus a `lib:examples/<NN-name>` baseline DM3b will mirror).",
  },
  DM2a: {
    title: "DM2a — Decomposition",
    body: "Break the design into named operations under `docs/analysis/decomposition.md` and characterize each with a data-movement summary in `docs/analysis/data-movement.md`. Every operation that DM2b will map to pipeline stages must appear here.",
  },
  DM2b: {
    title: "DM2b — Pipeline Mapping",
    body: "Assign each operation to a pipeline stage in `docs/analysis/pipeline-mapping.md`. Defines the in-order shape DM2c's implementation plan and DM2d's model will follow.",
  },
  DM2c: {
    title: "DM2c — Implementation Plan",
    body: "Break the modeling work into milestones under `docs/impl-plan/milestone-NN-*.md`, each with a checklist that DM2d will tick off as the model is implemented. The milestone files are this step's only output.",
  },
  DM2d: {
    title: "DM2d — Model Execution",
    body: "Implement the SystemVerilog model milestone-by-milestone, ticking off `- [x]` entries in each `milestone-NN-*.md` as code lands. Critique runs between milestones; the gate clears once every milestone is fully resolved.",
  },
  DM3a: {
    title: "DM3a — Test Plan",
    body: "Outline testbench scaffolding (`tb-milestone-NN-*.md`) and per-operation test sequences (`test-milestone-NN-*.md`) under `docs/test-plan/`. Both prefixes feed one pipeline that DM3b and DM3c walk in order.",
  },
  DM3b: {
    title: "DM3b — Testbench Build",
    body: "Implement the UVM-lite testbench components named in DM1's `docs/testbench.md`, ticking off the `tb-milestone-NN-*.md` rows. Lands the agents, scoreboard, and `SimEnvBuilder` wiring DM3c's tests will exercise.",
  },
  DM3c: {
    title: "DM3c — Test Execution",
    body: "Run the per-operation tests scaffolded in DM3a's `test-milestone-NN-*.md`. Failures route back through critique; the gate clears once every test milestone is resolved.",
  },
  DM4a: {
    title: "DM4a — Performance Plan",
    body: "Plan the perf experiments under `docs/perf-plan/perf-milestone-NN-*.md`. Each stub names the workload, the metric of interest, and how a run should be invoked.",
  },
  DM4b: {
    title: "DM4b — Performance Execution",
    body: "Execute each perf milestone, recording at least one run in `experiments.db`. The gate inspects experiment artifacts and clears once the perf plan is fully covered.",
  },
  SV0: {
    title: "SV0 — Verilog Plan",
    body: "Classify each Foundation module into a hardware pattern and stub RTL + UVM milestones under `generated/plan/`. Produces `generated/plan.md` (the index) plus per-area `rtl-milestone-NN-*.md` and `uvm-milestone-NN-*.md` files. Entered automatically after DM4b passes when Verilog generation is enabled.",
  },
  SV0d: {
    title: "SV0d — Plan Detail",
    body: "Fill in each milestone stub's task list. Walks every `rtl-milestone-` and `uvm-milestone-` placeholder under `generated/plan/` until every entry has been detailed.",
  },
  SV1: {
    title: "SV1 — RTL Emission",
    body: "Emit synthesizable SystemVerilog into `generated/rtl/` one module per RTL milestone, including shared packed structs (`payloads.sv`) and top-level wiring (`top.sv`). Critique runs between milestones; the gate clears once every RTL milestone task is resolved.",
  },
  SV2: {
    title: "SV2 — UVM Emission",
    body: "Emit UVM testbench scaffolding (sequence items, env, top) plus per-test files into `generated/test/`. Walks the `uvm-milestone-` set in `generated/plan/` until every UVM milestone task is resolved.",
  },
  SV3: {
    title: "SV3 — Build & Validate",
    body: "Generate the simulator file list (`sim.f`), a runnable `Makefile`, and a `validation.md` summary. Drives the emitted RTL through Verilator (or your configured simulator) and iterates the generated SystemVerilog until simulation passes.",
  },
};
