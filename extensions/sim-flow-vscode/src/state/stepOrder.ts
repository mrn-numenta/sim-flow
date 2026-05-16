// Canonical step ordering for each flow. Duplicates the structure
// the orchestrator's steps registry encodes (in Rust at
// `tools/sim-flow/src/__internal/steps/`) so TS callers can reason
// about "this step and all steps after it" without round-tripping
// to the orchestrator.
//
// Kept here -- rather than alongside the dashboard's StepDef
// arrays -- so the chat panel host (which doesn't bundle the
// dashboard webview) can reuse it.

import type { Flow } from "../cli/types";

const DM_STEP_ORDER: readonly string[] = [
  "DM0",
  "DM1",
  "DM2a",
  "DM2b",
  "DM2c",
  "DM2d",
  "DM3a",
  "DM3b",
  "DM3c",
  "DM4a",
  "DM4b",
];

const DS_STEP_ORDER: readonly string[] = [
  "DS0",
  "DS1",
  "DS2",
  "DS3a",
  "DS3b",
  "DS3c",
  "DS4",
  "DS5",
];

/**
 * SystemVerilog conversion flow step ordering. Mirrors the Rust
 * step registry in `tools/sim-flow/src/__internal/steps/sv.rs`.
 * Used both for the SV rail rendered alongside DMF when verilog
 * generation is enabled, and as the per-flow ordering once
 * `sim-flow convert-sv` flips a project into this flow.
 */
export const SV_STEP_ORDER: readonly string[] = [
  "SV0",
  "SV0d",
  "SV1",
  "SV2",
  "SV3",
];

/** Return the ordered step IDs for the given flow. */
export function stepOrderFor(flow: Flow): readonly string[] {
  switch (flow) {
    case "direct-modeling":
      return DM_STEP_ORDER;
    case "design-study":
      return DS_STEP_ORDER;
    case "systemverilog-convert":
      return SV_STEP_ORDER;
  }
}

/**
 * Return the step + every step that follows it in the flow. Used
 * by Reset: resetting step X also discards X+1, X+2, ... so the
 * flow can be replayed from X cleanly.
 */
export function stepsFromOnward(flow: Flow, step: string): string[] {
  const order = stepOrderFor(flow);
  const start = order.indexOf(step);
  if (start < 0) {
    return [];
  }
  return order.slice(start);
}
