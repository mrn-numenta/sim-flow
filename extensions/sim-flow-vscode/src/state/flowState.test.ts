import { describe, expect, it } from "vitest";

import { FlowStateParseError, parseFlowStateText } from "./flowState";

describe("parseFlowStateText", () => {
  it("parses a freshly-initialized direct-modeling state", () => {
    const toml = `
flow = "direct-modeling"
current_step = "DM0"

[gates]
`;
    const state = parseFlowStateText(toml);
    expect(state.flow).toBe("direct-modeling");
    expect(state.current_step).toBe("DM0");
    expect(state.started).toBeNull();
    expect(state.gates).toEqual({});
    expect(state.archived_gates).toEqual({});
  });

  it("parses a passed flat gate", () => {
    const toml = `
flow = "direct-modeling"
current_step = "DM1"

[gates]
DM0 = { passed = true, timestamp = "2026-04-22T10:00:00Z" }
`;
    const state = parseFlowStateText(toml);
    expect(state.gates.DM0.passed).toBe(true);
    expect(state.gates.DM0.timestamp).toBe("2026-04-22T10:00:00Z");
    expect(state.gates.DM0.candidates).toEqual({});
  });

  it("parses per-candidate gate subtables", () => {
    const toml = `
flow = "design-study"
current_step = "DS5a"

[gates.DS5a]
passed = false

[gates.DS5a.candidates]
"mesh-noc" = { passed = true, timestamp = "t1" }
"ring-noc" = { passed = false }
`;
    const state = parseFlowStateText(toml);
    const g = state.gates.DS5a;
    expect(g.passed).toBe(false);
    expect(g.candidates["mesh-noc"].passed).toBe(true);
    expect(g.candidates["mesh-noc"].timestamp).toBe("t1");
    expect(g.candidates["ring-noc"].passed).toBe(false);
    expect(g.candidates["ring-noc"].timestamp).toBeNull();
  });

  it("parses archived_gates produced by a DS9 flip", () => {
    const toml = `
flow = "direct-modeling"
current_step = "DM0"

[gates]

[archived_gates.ds]
DS0 = { passed = true, timestamp = "t" }
DS9 = { passed = true }
`;
    const state = parseFlowStateText(toml);
    expect(Object.keys(state.archived_gates)).toEqual(["ds"]);
    expect(state.archived_gates.ds.DS0.passed).toBe(true);
    expect(state.archived_gates.ds.DS9.passed).toBe(true);
  });

  it("rejects an unknown flow value", () => {
    const toml = `
flow = "bogus"
current_step = "X"
[gates]
`;
    expect(() => parseFlowStateText(toml)).toThrowError(FlowStateParseError);
  });

  it("rejects malformed TOML", () => {
    expect(() => parseFlowStateText("flow = ")).toThrowError(FlowStateParseError);
  });

  it("coerces datetime values in `started` to ISO strings", () => {
    const toml = `
flow = "direct-modeling"
current_step = "DM0"
started = 2026-04-22T10:00:00Z
[gates]
`;
    const state = parseFlowStateText(toml);
    expect(typeof state.started).toBe("string");
    expect(state.started).toContain("2026-04-22");
  });
});
