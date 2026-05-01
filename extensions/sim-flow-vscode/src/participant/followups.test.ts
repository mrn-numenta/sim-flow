import { describe, expect, it } from "vitest";

import { DM_ORDER, DS_ORDER, flowOrderFor, suggestFollowups } from "./followups";
import type { StatusResult } from "../cli/types";

function status(current: string, passed: Record<string, boolean> = {}): StatusResult {
  return {
    flow: "direct-modeling",
    current_step: current,
    started: null,
    gates: Object.fromEntries(
      Object.entries(passed).map(([id, p]) => [id, { passed: p, timestamp: null, candidates: {} }]),
    ),
    archived_gates: {},
  };
}

describe("flowOrderFor", () => {
  it("returns the DM order for direct-modeling", () => {
    expect(flowOrderFor("direct-modeling")).toBe(DM_ORDER);
  });

  it("returns the DS order for design-study", () => {
    expect(flowOrderFor("design-study")).toBe(DS_ORDER);
  });
});

describe("suggestFollowups", () => {
  it("returns a /status fallback when state is missing", () => {
    const out = suggestFollowups(undefined, DM_ORDER);
    expect(out).toHaveLength(1);
    expect(out[0].command).toBe("status");
  });

  it("suggests work/critique/gate when the current step has not passed", () => {
    const s = status("DM0", {});
    const out = suggestFollowups(s, DM_ORDER);
    const commands = out.map((f) => f.command);
    expect(commands).toContain("step");
    expect(commands).toContain("gate");
    const prompts = out.map((f) => f.prompt);
    expect(prompts).toContain("DM0.work");
    expect(prompts).toContain("DM0.critique");
    expect(prompts).toContain("DM0");
  });

  it("suggests the next step once the current one passes", () => {
    const s = status("DM0", { DM0: true });
    const out = suggestFollowups(s, DM_ORDER);
    expect(out[0].command).toBe("step");
    expect(out[0].prompt).toBe("DM1.work");
  });

  it("falls through to /status when the final step has passed", () => {
    const s = status("DM4", { DM4: true });
    const out = suggestFollowups(s, DM_ORDER);
    expect(out[0].command).toBe("status");
  });
});
