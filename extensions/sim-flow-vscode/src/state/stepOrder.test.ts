import { describe, expect, test } from "vitest";

import { SV_STEP_ORDER, stepOrderFor, stepsFromOnward } from "./stepOrder";

describe("stepOrderFor", () => {
  test("direct-modeling order matches the orchestrator's DM registry", () => {
    expect(stepOrderFor("direct-modeling")).toEqual([
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
    ]);
  });

  test("design-study order matches the orchestrator's DS registry", () => {
    expect(stepOrderFor("design-study")).toEqual([
      "DS0",
      "DS1",
      "DS2",
      "DS3a",
      "DS3b",
      "DS3c",
      "DS4",
      "DS5",
    ]);
  });

  test("systemverilog-convert order matches SV_STEP_ORDER", () => {
    expect(stepOrderFor("systemverilog-convert")).toEqual([...SV_STEP_ORDER]);
    // And the SV ordering itself is the documented canonical list.
    expect(SV_STEP_ORDER).toEqual(["SV0", "SV0d", "SV1", "SV2", "SV3"]);
  });
});

describe("stepsFromOnward", () => {
  test("returns the step plus every successor for a mid-flow step", () => {
    expect(stepsFromOnward("direct-modeling", "DM2c")).toEqual([
      "DM2c",
      "DM2d",
      "DM3a",
      "DM3b",
      "DM3c",
      "DM4a",
      "DM4b",
    ]);
  });

  test("returns just the head step when called on the first step of a flow", () => {
    expect(stepsFromOnward("design-study", "DS0")).toEqual([
      "DS0",
      "DS1",
      "DS2",
      "DS3a",
      "DS3b",
      "DS3c",
      "DS4",
      "DS5",
    ]);
  });

  test("returns just the step when called on the terminal step of a flow", () => {
    expect(stepsFromOnward("systemverilog-convert", "SV3")).toEqual(["SV3"]);
  });

  test("returns [] when the step is not in the flow (caller bug guard)", () => {
    // Symmetric across all flows -- protects callers that pass a
    // step from the wrong flow without a runtime crash.
    expect(stepsFromOnward("direct-modeling", "DS0")).toEqual([]);
    expect(stepsFromOnward("design-study", "DM2a")).toEqual([]);
    expect(stepsFromOnward("systemverilog-convert", "DM0")).toEqual([]);
    // And clearly-bogus step ids.
    expect(stepsFromOnward("direct-modeling", "")).toEqual([]);
    expect(stepsFromOnward("direct-modeling", "DM9z")).toEqual([]);
  });
});
