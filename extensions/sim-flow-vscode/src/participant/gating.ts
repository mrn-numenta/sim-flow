// Centralized gate decisions for @sim-flow chat turns.
//
// The rule from docs/architecture/ai-flow/06-vscode-extension.md:
// every chat request is cross-checked against `state.toml`. If the
// requested step is not the current step the participant refuses
// politely; the chat tab stays visible so the user can scroll prior
// history, but no new agent turn runs until the step is reactivated
// via `/reset <step>`.
//
// This module is pure: no vscode imports, so it can be unit-tested
// without loading the extension host.

import type { StatusResult } from "../cli/types";
import type { StepKind } from "./args";

export type GateOutcome = { kind: "ok" } | { kind: "refused"; message: string };

/**
 * Decide whether a `/step <step>.<kind>` invocation is allowed given
 * the current orchestrator state.
 *
 * Rules:
 * - If state is unknown (no state.toml yet), refuse with an init hint.
 * - If the requested step matches `state.current_step`, allow.
 * - If the requested step has already passed its gate, refuse with a
 *   reset hint.
 * - Otherwise the step is "ahead of" the current step (not yet
 *   reachable); refuse and point at the current step.
 */
export function checkStepGate(
  state: StatusResult | undefined,
  requestedStep: string,
  _kind: StepKind,
): GateOutcome {
  if (!state) {
    return {
      kind: "refused",
      message:
        "No sim-flow state found in this project. Run `/init` first to create `.sim-flow/state.toml`.",
    };
  }
  if (state.current_step === requestedStep) {
    return { kind: "ok" };
  }
  const gate = state.gates[requestedStep];
  if (gate?.passed) {
    return {
      kind: "refused",
      message: [
        `\`${requestedStep}\` already passed its gate.`,
        "",
        "The chat history for this step stays visible; scroll up to review it.",
        `To re-enter \`${requestedStep}\`, run \`/reset ${requestedStep}\`; that will also cascade-reset any downstream gates.`,
      ].join("\n"),
    };
  }
  return {
    kind: "refused",
    message: [
      `\`${requestedStep}\` is not the current step.`,
      "",
      `Current step: \`${state.current_step}\`.`,
      `Finish the current step first, or run \`/reset ${state.current_step}\` to back up.`,
    ].join("\n"),
  };
}
