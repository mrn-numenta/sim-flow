// Pure helpers that produce follow-up suggestions from a sim-flow
// state snapshot. Extracted from the participant module so they can
// be unit-tested without loading vscode.

import type { StatusResult } from "../cli/types";

export interface Followup {
  prompt: string;
  label: string;
  command?: string;
}

/**
 * Suggest the next actions the user should consider given the current
 * orchestrator state. Rules:
 *
 * - If the current step has not yet passed and a critique exists on
 *   disk, suggest `/gate <current>`.
 * - Otherwise suggest `/step <current>.work` to start/continue it.
 * - Once a step passes, suggest the next step in the flow.
 */
export function suggestFollowups(
  state: StatusResult | undefined,
  flowOrder: readonly string[],
): Followup[] {
  if (!state) {
    return [{ prompt: "/status", label: "Show status", command: "status" }];
  }
  const { current_step, gates } = state;
  const currentGate = gates[current_step];
  const followups: Followup[] = [];

  if (currentGate?.passed) {
    const next = nextStepAfter(current_step, flowOrder);
    if (next) {
      followups.push({
        prompt: `${next}.work`,
        label: `Start ${next} work`,
        command: "step",
      });
    } else {
      followups.push({ prompt: "", label: "Show status", command: "status" });
    }
  } else {
    followups.push(
      { prompt: `${current_step}.work`, label: `Work on ${current_step}`, command: "step" },
      { prompt: `${current_step}.critique`, label: `Critique ${current_step}`, command: "step" },
      { prompt: current_step, label: `Run gate for ${current_step}`, command: "gate" },
    );
  }
  return followups;
}

/** Canonical step orders for the two flows. Exported for tests. */
export const DM_ORDER: readonly string[] = [
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

export const DS_ORDER: readonly string[] = [
  "DS0",
  "DS1",
  "DS2",
  "DS3",
  "DS4",
  "DS5a",
  "DS5b",
  "DS6",
  "DS7",
  "DS8",
  "DS9",
];

export function flowOrderFor(flow: "direct-modeling" | "design-study"): readonly string[] {
  return flow === "direct-modeling" ? DM_ORDER : DS_ORDER;
}

function nextStepAfter(step: string, order: readonly string[]): string | undefined {
  const i = order.indexOf(step);
  if (i < 0 || i >= order.length - 1) {
    return undefined;
  }
  return order[i + 1];
}
