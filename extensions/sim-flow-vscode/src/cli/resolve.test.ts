import { describe, expect, it } from "vitest";

import { resolveBinary } from "./resolve";
import { SimFlowCliError } from "./errors";

function mkExists(existing: string[]): (path: string) => boolean {
  const set = new Set(existing);
  return (p) => set.has(p);
}

describe("resolveBinary", () => {
  it("prefers the setting override when it exists", () => {
    const resolved = resolveBinary({
      settingOverride: "/opt/sim-flow",
      pathEnv: "/usr/bin:/usr/local/bin",
      exists: mkExists(["/opt/sim-flow", "/usr/local/bin/sim-flow"]),
    });
    expect(resolved).toBe("/opt/sim-flow");
  });

  it("throws when the setting override is missing", () => {
    expect(() =>
      resolveBinary({
        settingOverride: "/no/such/thing",
        pathEnv: "/usr/bin",
        exists: mkExists([]),
      }),
    ).toThrowError(SimFlowCliError);
  });

  it("falls back to PATH lookup when the setting is empty", () => {
    const resolved = resolveBinary({
      settingOverride: "",
      pathEnv: "/usr/bin:/usr/local/bin",
      exists: mkExists(["/usr/local/bin/sim-flow"]),
    });
    expect(resolved).toBe("/usr/local/bin/sim-flow");
  });

  it("falls back to bundled candidates when PATH yields nothing", () => {
    const resolved = resolveBinary({
      pathEnv: "/usr/bin",
      bundledCandidates: () => ["/bundle/macos-arm64/sim-flow"],
      exists: mkExists(["/bundle/macos-arm64/sim-flow"]),
    });
    expect(resolved).toBe("/bundle/macos-arm64/sim-flow");
  });

  it("throws a binary-not-found error when nothing is resolvable", () => {
    try {
      resolveBinary({
        pathEnv: "/usr/bin",
        bundledCandidates: () => [],
        exists: mkExists([]),
      });
      throw new Error("expected resolveBinary to throw");
    } catch (err) {
      expect(err).toBeInstanceOf(SimFlowCliError);
      expect((err as SimFlowCliError).kind).toBe("binary-not-found");
    }
  });
});
