import { describe, expect, it } from "vitest";

import {
  extractProjectHint,
  parseGateArgs,
  parseResetArgs,
  parseRunsArgs,
  parseStepRef,
  tokenize,
} from "./args";

describe("tokenize", () => {
  it("splits on whitespace", () => {
    expect(tokenize("a b  c")).toEqual(["a", "b", "c"]);
  });

  it("respects double-quoted groups", () => {
    expect(tokenize('DM0 --candidate "mesh noc"')).toEqual(["DM0", "--candidate", "mesh noc"]);
  });

  it("returns an empty array for empty input", () => {
    expect(tokenize("")).toEqual([]);
    expect(tokenize("   ")).toEqual([]);
  });
});

describe("parseStepRef", () => {
  it("parses work and critique kinds", () => {
    expect(parseStepRef("DM0.work")).toEqual({ step: "DM0", kind: "work" });
    expect(parseStepRef("DM3c.critique")).toEqual({
      step: "DM3c",
      kind: "critique",
    });
  });

  it("picks up --candidate", () => {
    expect(parseStepRef("DS5a.work --candidate mesh-noc")).toEqual({
      step: "DS5a",
      kind: "work",
      candidate: "mesh-noc",
    });
  });

  it("errors on missing dot", () => {
    const r = parseStepRef("DM0");
    expect(r).toHaveProperty("error");
  });

  it("errors on unknown kind", () => {
    const r = parseStepRef("DM0.bogus");
    expect(r).toHaveProperty("error");
  });

  it("errors on empty prompt", () => {
    expect(parseStepRef("")).toHaveProperty("error");
  });
});

describe("parseGateArgs", () => {
  it("returns empty object when no step is supplied", () => {
    expect(parseGateArgs("")).toEqual({});
  });

  it("picks up a step", () => {
    expect(parseGateArgs("DM2a")).toEqual({ step: "DM2a" });
  });

  it("picks up --candidate after the step", () => {
    expect(parseGateArgs("DS5a --candidate ring-noc")).toEqual({
      step: "DS5a",
      candidate: "ring-noc",
    });
  });
});

describe("parseRunsArgs", () => {
  it("accepts no filters", () => {
    expect(parseRunsArgs("")).toEqual({});
  });

  it("parses all supported filters", () => {
    expect(
      parseRunsArgs(
        "--workload wk --candidate mesh --study nocstudy --sweep 001-parent --limit 25",
      ),
    ).toEqual({
      workload: "wk",
      candidate: "mesh",
      study: "nocstudy",
      sweep: "001-parent",
      limit: 25,
    });
  });

  it("ignores a non-numeric --limit", () => {
    expect(parseRunsArgs("--limit abc")).toEqual({});
  });
});

describe("parseResetArgs", () => {
  it("reads the step argument", () => {
    expect(parseResetArgs("DM2b")).toEqual({ step: "DM2b" });
  });

  it("errors on missing step", () => {
    expect(parseResetArgs("")).toHaveProperty("error");
  });
});

describe("extractProjectHint", () => {
  it("returns the original prompt when no --project flag is present", () => {
    expect(extractProjectHint("DM0.work")).toEqual({
      hint: undefined,
      stripped: "DM0.work",
    });
  });

  it("pulls --project out of the middle of the prompt", () => {
    expect(extractProjectHint("DM0.work --project /repo/model-a")).toEqual({
      hint: "/repo/model-a",
      stripped: "DM0.work",
    });
  });

  it("pulls --project out from the front of the prompt", () => {
    expect(extractProjectHint("--project /repo/model-a DS5a.work --candidate mesh")).toEqual({
      hint: "/repo/model-a",
      stripped: "DS5a.work --candidate mesh",
    });
  });

  it("feeds the stripped prompt cleanly into a per-command parser", () => {
    const { hint, stripped } = extractProjectHint(
      "DS5a.work --candidate mesh --project /repo/model-a",
    );
    expect(hint).toBe("/repo/model-a");
    expect(parseStepRef(stripped)).toEqual({
      step: "DS5a",
      kind: "work",
      candidate: "mesh",
    });
  });

  it("ignores a trailing --project with no value", () => {
    expect(extractProjectHint("DM0.work --project")).toEqual({
      hint: undefined,
      stripped: "DM0.work --project",
    });
  });
});
