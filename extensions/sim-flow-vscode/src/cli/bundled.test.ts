import { afterEach, describe, expect, it } from "vitest";

import { bundledCandidates, platformDir, setBundledRoot } from "./bundled";

afterEach(() => {
  // Reset between tests so they stay independent.
  setBundledRoot("");
});

describe("platformDir", () => {
  it("maps supported (platform, arch) combinations", () => {
    expect(platformDir("darwin", "arm64")).toBe("darwin-arm64");
    expect(platformDir("darwin", "x64")).toBe("darwin-x64");
    expect(platformDir("linux", "x64")).toBe("linux-x64");
    expect(platformDir("win32", "x64")).toBe("win32-x64");
  });

  it("returns null for platforms we do not ship a binary for", () => {
    expect(platformDir("linux", "arm64")).toBeNull();
    expect(platformDir("freebsd", "x64")).toBeNull();
    expect(platformDir("win32", "arm64")).toBeNull();
  });
});

describe("bundledCandidates", () => {
  it("returns an empty array when setBundledRoot has not been called", () => {
    setBundledRoot("");
    expect(bundledCandidates()).toEqual([]);
  });

  it("produces a single path under <root>/bin/<platform-dir>/ when the platform is supported", () => {
    setBundledRoot("/tmp/ext-root");
    const candidates = bundledCandidates();
    const mapped = platformDir(process.platform, process.arch);
    if (mapped === null) {
      expect(candidates).toEqual([]);
      return;
    }
    const exe = process.platform === "win32" ? "sim-flow.exe" : "sim-flow";
    expect(candidates).toEqual([`/tmp/ext-root/bin/${mapped}/${exe}`]);
  });
});
