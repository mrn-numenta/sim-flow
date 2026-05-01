import { describe, expect, it } from "vitest";

import type { StatusResult } from "../cli/types";
import { checkStepGate } from "./gating";

function state(current: string, gates: Record<string, { passed: boolean }> = {}): StatusResult {
  return {
    flow: "direct-modeling",
    current_step: current,
    started: null,
    gates: Object.fromEntries(
      Object.entries(gates).map(([id, g]) => [
        id,
        { passed: g.passed, timestamp: null, candidates: {} },
      ]),
    ),
    archived_gates: {},
  };
}

describe("checkStepGate", () => {
  it("refuses with an init hint when state is undefined", () => {
    const out = checkStepGate(undefined, "DM0", "work");
    expect(out.kind).toBe("refused");
    if (out.kind === "refused") {
      expect(out.message).toMatch(/\/init/);
    }
  });

  it("allows the current step", () => {
    expect(checkStepGate(state("DM0"), "DM0", "work").kind).toBe("ok");
    expect(checkStepGate(state("DM2a"), "DM2a", "critique").kind).toBe("ok");
  });

  it("refuses a step that already passed and points at /reset", () => {
    const s = state("DM1", { DM0: { passed: true } });
    const out = checkStepGate(s, "DM0", "work");
    expect(out.kind).toBe("refused");
    if (out.kind === "refused") {
      expect(out.message).toMatch(/already passed/);
      expect(out.message).toMatch(/\/reset DM0/);
    }
  });

  it("refuses a step that is ahead of the current step and names the current one", () => {
    const s = state("DM0");
    const out = checkStepGate(s, "DM2a", "work");
    expect(out.kind).toBe("refused");
    if (out.kind === "refused") {
      expect(out.message).toMatch(/not the current step/);
      expect(out.message).toMatch(/DM0/);
    }
  });
});
