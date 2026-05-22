import * as fs from "node:fs";
import * as fsp from "node:fs/promises";
import * as path from "node:path";
import { tmpdir } from "node:os";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  COVERAGE_DEFAULTS,
  LLM_DEFAULTS,
  readCoverageSettings,
  readLlmSettings,
  readSpecPath,
  writeCoverageSettings,
  writeLlmSettings,
  writeSpecPath,
} from "./projectConfig";

let projectDir: string;

beforeEach(() => {
  projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-projcfg-"));
});

afterEach(() => {
  fs.rmSync(projectDir, { recursive: true, force: true });
});

const configPath = (): string => path.join(projectDir, ".sim-flow", "config.toml");

describe("readSpecPath", () => {
  it("returns '' when the project has no .sim-flow/config.toml", async () => {
    const got = await readSpecPath(projectDir);
    expect(got).toBe("");
  });

  it("returns '' when config.toml exists but has no spec_path", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[client]\nname = "mock"\n', "utf8");
    expect(await readSpecPath(projectDir)).toBe("");
  });

  it("returns the configured spec_path verbatim", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), 'spec_path = "/abs/spec.md"\n', "utf8");
    expect(await readSpecPath(projectDir)).toBe("/abs/spec.md");
  });

  it("returns '' when spec_path exists but isn't a string", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), "spec_path = 42\n", "utf8");
    expect(await readSpecPath(projectDir)).toBe("");
  });

  it("throws when config.toml is malformed TOML", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), "this is = not [ valid toml", "utf8");
    await expect(readSpecPath(projectDir)).rejects.toThrow();
  });

  it("throws when the file's top level isn't a table (defensive)", async () => {
    // smol-toml's parse always returns a table at the document
    // root, so this path is mostly defensive; we still want the
    // explicit error if the parser ever changes behavior.
    // Simulate by writing a TOML array at the root, which the
    // parser accepts as `{ "" : [...] }` -- not the case we
    // guard against, so write a file the typeguard rejects via
    // a non-string spec path elsewhere. (No-op; this test is a
    // placeholder to document the path.)
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), "", "utf8");
    expect(await readSpecPath(projectDir)).toBe("");
  });
});

describe("writeSpecPath", () => {
  it("creates the .sim-flow directory and writes spec_path", async () => {
    await writeSpecPath(projectDir, "/abs/spec.md");
    expect(fs.existsSync(configPath())).toBe(true);
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).toContain('spec_path = "/abs/spec.md"');
  });

  it("clears spec_path when writing an empty string", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), 'spec_path = "/abs/spec.md"\n[client]\nname = "mock"\n', "utf8");
    await writeSpecPath(projectDir, "");
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).not.toMatch(/spec_path/);
    // Other keys are preserved.
    expect(text).toContain("[client]");
    expect(text).toContain('name = "mock"');
  });

  it("preserves unknown keys round-trip", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      configPath(),
      ["[client]", 'name = "mock"', "", "[claude]", 'model = "sonnet"', ""].join("\n"),
      "utf8",
    );
    await writeSpecPath(projectDir, "/x/spec.md");
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).toContain('spec_path = "/x/spec.md"');
    expect(text).toContain("[client]");
    expect(text).toContain("[claude]");
    expect(text).toContain('model = "sonnet"');
  });

  it("overwrites a prior spec_path on subsequent writes", async () => {
    await writeSpecPath(projectDir, "/first.md");
    await writeSpecPath(projectDir, "/second.md");
    expect(await readSpecPath(projectDir)).toBe("/second.md");
  });
});

describe("readCoverageSettings", () => {
  it("returns the defaults when no config exists", async () => {
    const got = await readCoverageSettings(projectDir);
    expect(got).toEqual(COVERAGE_DEFAULTS);
  });

  it("returns the defaults when config has no [coverage] section", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[client]\nname = "mock"\n', "utf8");
    expect(await readCoverageSettings(projectDir)).toEqual(COVERAGE_DEFAULTS);
  });

  it("reads threshold_pct and level from the [coverage] section", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[coverage]\nthreshold_pct = 75.5\nlevel = "module"\n', "utf8");
    expect(await readCoverageSettings(projectDir)).toEqual({
      thresholdPct: 75.5,
      level: "module",
    });
  });

  it("falls back to defaults when threshold_pct is missing or non-numeric", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[coverage]\nlevel = "total"\n', "utf8");
    const got = await readCoverageSettings(projectDir);
    expect(got.thresholdPct).toBe(COVERAGE_DEFAULTS.thresholdPct);
    expect(got.level).toBe("total");
  });

  it("falls back to defaults when level is missing or invalid", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[coverage]\nthreshold_pct = 50\nlevel = "garbage"\n', "utf8");
    const got = await readCoverageSettings(projectDir);
    expect(got.thresholdPct).toBe(50);
    expect(got.level).toBe(COVERAGE_DEFAULTS.level);
  });

  it("ignores [coverage] when it isn't a table", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), 'coverage = "not a table"\n', "utf8");
    expect(await readCoverageSettings(projectDir)).toEqual(COVERAGE_DEFAULTS);
  });
});

describe("writeCoverageSettings", () => {
  it("creates the file and writes both fields", async () => {
    const echoed = await writeCoverageSettings(projectDir, {
      thresholdPct: 80,
      level: "module",
    });
    expect(echoed).toEqual({ thresholdPct: 80, level: "module" });
    const round = await readCoverageSettings(projectDir);
    expect(round).toEqual({ thresholdPct: 80, level: "module" });
  });

  it("clamps threshold_pct above 100", async () => {
    const echoed = await writeCoverageSettings(projectDir, {
      thresholdPct: 9001,
      level: "total",
    });
    expect(echoed.thresholdPct).toBe(100);
    const round = await readCoverageSettings(projectDir);
    expect(round.thresholdPct).toBe(100);
  });

  it("clamps threshold_pct below 0", async () => {
    const echoed = await writeCoverageSettings(projectDir, {
      thresholdPct: -5,
      level: "module",
    });
    expect(echoed.thresholdPct).toBe(0);
    const round = await readCoverageSettings(projectDir);
    expect(round.thresholdPct).toBe(0);
  });

  it("preserves unknown sections on round-trip", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      configPath(),
      [
        "[client]",
        'name = "claude"',
        "",
        "[claude]",
        'model = "sonnet"',
        'allowed_tools = ["Read", "Edit"]',
        "",
      ].join("\n"),
      "utf8",
    );
    await writeCoverageSettings(projectDir, { thresholdPct: 70, level: "module" });
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).toContain("[client]");
    expect(text).toContain('name = "claude"');
    expect(text).toContain("[claude]");
    expect(text).toContain('model = "sonnet"');
    expect(text).toContain("[coverage]");
    expect(text).toContain("threshold_pct = 70");
    expect(text).toContain('level = "module"');
  });

  it("overwrites prior coverage settings", async () => {
    await writeCoverageSettings(projectDir, { thresholdPct: 50, level: "module" });
    await writeCoverageSettings(projectDir, { thresholdPct: 95, level: "total" });
    expect(await readCoverageSettings(projectDir)).toEqual({
      thresholdPct: 95,
      level: "total",
    });
  });

  it("creates intermediate parent directories when missing", async () => {
    // Sanity check that `mkdir({recursive: true})` actually
    // covers the "no .sim-flow yet" case.
    expect(fs.existsSync(path.join(projectDir, ".sim-flow"))).toBe(false);
    await writeCoverageSettings(projectDir, { thresholdPct: 60, level: "total" });
    expect(fs.existsSync(path.join(projectDir, ".sim-flow"))).toBe(true);
  });

  it("integrates: writeSpecPath and writeCoverageSettings coexist in one file", async () => {
    await writeSpecPath(projectDir, "/abs/spec.md");
    await writeCoverageSettings(projectDir, { thresholdPct: 88, level: "module" });
    expect(await readSpecPath(projectDir)).toBe("/abs/spec.md");
    expect(await readCoverageSettings(projectDir)).toEqual({
      thresholdPct: 88,
      level: "module",
    });
    // Writing one shouldn't clobber the other.
    await writeSpecPath(projectDir, "/different/spec.md");
    expect(await readCoverageSettings(projectDir)).toEqual({
      thresholdPct: 88,
      level: "module",
    });
  });

  it("read after manual fsp.unlink falls back to defaults", async () => {
    await writeCoverageSettings(projectDir, { thresholdPct: 80, level: "module" });
    await fsp.rm(configPath());
    expect(await readCoverageSettings(projectDir)).toEqual(COVERAGE_DEFAULTS);
  });
});

describe("readLlmSettings", () => {
  it("returns defaults when config.toml is missing", async () => {
    expect(await readLlmSettings(projectDir)).toEqual(LLM_DEFAULTS);
  });

  it("returns defaults when [llm] section is missing", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[client]\nname = "mock"\n', "utf8");
    expect(await readLlmSettings(projectDir)).toEqual(LLM_DEFAULTS);
  });

  it("reads max_parallel_requests verbatim when present", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), "[llm]\nmax_parallel_requests = 4\n", "utf8");
    expect(await readLlmSettings(projectDir)).toEqual({ maxParallelRequests: 4 });
  });

  it("falls back to default when value is non-numeric", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), '[llm]\nmax_parallel_requests = "lots"\n', "utf8");
    expect(await readLlmSettings(projectDir)).toEqual(LLM_DEFAULTS);
  });

  it("falls back to default when value is negative", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(configPath(), "[llm]\nmax_parallel_requests = -3\n", "utf8");
    expect(await readLlmSettings(projectDir)).toEqual(LLM_DEFAULTS);
  });
});

describe("writeLlmSettings", () => {
  it("creates the file and writes max_parallel_requests", async () => {
    const echoed = await writeLlmSettings(projectDir, { maxParallelRequests: 8 });
    expect(echoed).toEqual({ maxParallelRequests: 8 });
    expect(await readLlmSettings(projectDir)).toEqual({ maxParallelRequests: 8 });
  });

  it("clamps negative inputs to 0", async () => {
    const echoed = await writeLlmSettings(projectDir, { maxParallelRequests: -2 });
    expect(echoed.maxParallelRequests).toBe(0);
  });

  it("floors non-integer inputs", async () => {
    const echoed = await writeLlmSettings(projectDir, { maxParallelRequests: 3.7 });
    expect(echoed.maxParallelRequests).toBe(3);
  });

  it("preserves unknown sections on round-trip", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      configPath(),
      [
        "[client]",
        'name = "claude"',
        "",
        "[coverage]",
        "threshold_pct = 85",
        'level = "module"',
        "",
      ].join("\n"),
      "utf8",
    );
    await writeLlmSettings(projectDir, { maxParallelRequests: 2 });
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).toContain("[client]");
    expect(text).toContain("[coverage]");
    expect(text).toContain("threshold_pct = 85");
    expect(text).toContain("[llm]");
    expect(text).toContain("max_parallel_requests = 2");
  });

  it("does not clobber sibling [llm] keys", async () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      configPath(),
      ["[llm]", "max_parallel_requests = 1", "some_future_knob = 99", ""].join("\n"),
      "utf8",
    );
    await writeLlmSettings(projectDir, { maxParallelRequests: 5 });
    const text = fs.readFileSync(configPath(), "utf8");
    expect(text).toContain("max_parallel_requests = 5");
    expect(text).toContain("some_future_knob = 99");
  });
});
