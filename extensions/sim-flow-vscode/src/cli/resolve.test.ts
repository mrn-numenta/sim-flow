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

  it("returns null path lookup when pathEnv is empty", () => {
    // Covers the `if (!pathEnv) return null` early-exit branch.
    // With no PATH and no bundled candidates, the resolver should
    // fall through to the not-found error.
    try {
      resolveBinary({
        settingOverride: "",
        pathEnv: "",
        bundledCandidates: () => [],
        exists: mkExists([]),
      });
      throw new Error("expected resolveBinary to throw");
    } catch (err) {
      expect((err as SimFlowCliError).kind).toBe("binary-not-found");
    }
  });

  it("skips empty PATH entries from a trailing-colon edge case", () => {
    // "/usr/bin::/usr/local/bin" -- split yields ["/usr/bin", "",
    // "/usr/local/bin"]. The empty segment must NOT be join()'d into
    // "/sim-flow" (or any other false-positive), it must be skipped.
    const visited: string[] = [];
    const exists = (p: string): boolean => {
      visited.push(p);
      return p === "/usr/local/bin/sim-flow";
    };
    const resolved = resolveBinary({
      pathEnv: "/usr/bin::/usr/local/bin",
      exists,
    });
    expect(resolved).toBe("/usr/local/bin/sim-flow");
    // /sim-flow (the empty-dir join result) must NOT have been
    // probed.
    expect(visited).not.toContain("/sim-flow");
  });

  it("uses the real fs accessSync code path when no exists hook is provided", () => {
    // /bin/sh exists and is executable on every supported test host;
    // /this-binary-does-not-exist obviously isn't. This trips the
    // default defaultExists() implementation rather than the test
    // hook, covering its try / catch branches.
    const resolved = resolveBinary({
      settingOverride: "/bin/sh",
    });
    expect(resolved).toBe("/bin/sh");
    try {
      resolveBinary({
        settingOverride: "/no/such/binary",
        pathEnv: "",
        bundledCandidates: () => [],
      });
      throw new Error("expected resolveBinary to throw");
    } catch (err) {
      expect(err).toBeInstanceOf(SimFlowCliError);
    }
  });
});
