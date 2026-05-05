import { describe, expect, it } from "vitest";

import type { DashboardState, DocumentEntry } from "./messages";
import { deriveStepActionState, isStepSelectableInRail } from "./stepActions";
import type { GateResult, StatusResult } from "../cli/types";
import type { CritiqueFile, PlanProgress } from "../state/types";

function flowState(current: string, gates: StatusResult["gates"] = {}): StatusResult {
  return {
    flow: "direct-modeling",
    current_step: current,
    started: null,
    gates,
    archived_gates: {},
  };
}

function state(input: {
  flow: StatusResult;
  critiques?: CritiqueFile[];
  documents?: DocumentEntry[];
}): DashboardState {
  const planProgress: PlanProgress = {
    kind: "none",
    milestones: [],
    currentTask: null,
    currentTaskFilePath: null,
    currentTaskLine: null,
  };
  return {
    projectDir: "/proj",
    flow: input.flow,
    critiques: input.critiques ?? [],
    runs: [],
    baselines: [],
    documents: input.documents ?? [],
    planProgress,
    specPath: "",
    fullyAutomatedEnabled: false,
    verilogSimEnabled: false,
    verilogSimulatorPath: "",
    stepMode: "manual",
    sessionActive: false,
    inSubSession: false,
    generatedAt: "2026-04-30T00:00:00Z",
  };
}

function workArtifact(step: string, modifiedAt: string): DocumentEntry {
  return {
    absPath: `/proj/${step}/artifact`,
    relPath: `${step}/artifact`,
    category: "work-artifact",
    step,
    bytes: 10,
    modifiedAt,
    exists: true,
  };
}

function critique(step: string, hasBlocking: boolean): CritiqueFile {
  return {
    path: `/proj/docs/critiques/${step}-critique.md`,
    step,
    body: "",
    findings: hasBlocking ? [{ kind: "blocker", text: "fix me", line: 1 }] : [],
    hasBlocking,
  };
}

function cleanGate(step: string): GateResult {
  return {
    step,
    clean: true,
    failures: [],
  };
}

describe("deriveStepActionState", () => {
  it("enables only Run Step for a fresh current step", () => {
    const actions = deriveStepActionState({
      data: state({ flow: flowState("DM0") }),
      stepId: "DM0",
      gateReport: null,
    });

    expect(actions.runStepEnabled).toBe(true);
    expect(actions.runCritiqueEnabled).toBe(false);
    expect(actions.runGateEnabled).toBe(false);
    expect(actions.advanceEnabled).toBe(false);
    expect(actions.resetEnabled).toBe(false);
    expect(actions.showGenerateVerilog).toBe(false);
  });

  it("keeps critique disabled when only stale template artifacts exist", () => {
    const actions = deriveStepActionState({
      data: state({
        flow: flowState("DM2d", {
          DM2c: { passed: true, timestamp: "2026-04-30T12:00:00Z", candidates: {} },
        }),
        documents: [workArtifact("DM2d", "2026-04-30T11:00:00Z")],
      }),
      stepId: "DM2d",
      gateReport: null,
    });

    expect(actions.runStepEnabled).toBe(true);
    expect(actions.runCritiqueEnabled).toBe(false);
  });

  it("enables critique after fresh work artifacts are produced", () => {
    const actions = deriveStepActionState({
      data: state({
        flow: flowState("DM2d", {
          DM2c: { passed: true, timestamp: "2026-04-30T12:00:00Z", candidates: {} },
        }),
        documents: [workArtifact("DM2d", "2026-04-30T12:30:00Z")],
      }),
      stepId: "DM2d",
      gateReport: null,
    });

    expect(actions.runCritiqueEnabled).toBe(true);
    expect(actions.resetEnabled).toBe(true);
  });

  it("enables Run Gate only when the critique is clean", () => {
    const blocked = deriveStepActionState({
      data: state({
        flow: flowState("DM1"),
        critiques: [critique("DM1", true)],
      }),
      stepId: "DM1",
      gateReport: null,
    });
    const clean = deriveStepActionState({
      data: state({
        flow: flowState("DM1"),
        critiques: [critique("DM1", false)],
      }),
      stepId: "DM1",
      gateReport: null,
    });

    expect(blocked.runGateEnabled).toBe(false);
    expect(clean.runGateEnabled).toBe(true);
  });

  it("enables Advance only after a clean gate result", () => {
    const actions = deriveStepActionState({
      data: state({
        flow: flowState("DM3a"),
        critiques: [critique("DM3a", false)],
      }),
      stepId: "DM3a",
      gateReport: cleanGate("DM3a"),
    });

    expect(actions.runGateEnabled).toBe(true);
    expect(actions.advanceEnabled).toBe(true);
  });

  it("shows Generate Verilog only on DM3-and-later steps after DM2d has passed", () => {
    const hidden = deriveStepActionState({
      data: state({
        flow: flowState("DM3a", {
          DM2d: { passed: true, timestamp: "2026-04-30T12:00:00Z", candidates: {} },
        }),
      }),
      stepId: "DM2d",
      gateReport: null,
    });
    const hiddenBeforeDm2dPass = deriveStepActionState({
      data: state({ flow: flowState("DM2d") }),
      stepId: "DM2d",
      gateReport: null,
    });
    const shown = deriveStepActionState({
      data: state({
        flow: flowState("DM3a", {
          DM2d: { passed: true, timestamp: "2026-04-30T12:00:00Z", candidates: {} },
        }),
      }),
      stepId: "DM3a",
      gateReport: null,
    });

    expect(hidden.showGenerateVerilog).toBe(false);
    expect(hiddenBeforeDm2dPass.showGenerateVerilog).toBe(false);
    expect(shown.showGenerateVerilog).toBe(true);
  });

  it("hides Generate Verilog when viewing a non-current later step", () => {
    const actions = deriveStepActionState({
      data: state({
        flow: flowState("DM4a", {
          DM2d: { passed: true, timestamp: "2026-04-30T12:00:00Z", candidates: {} },
        }),
      }),
      stepId: "DM3a",
      gateReport: null,
    });

    expect(actions.showGenerateVerilog).toBe(false);
  });

  it("prevents selecting future steps that have never been entered", () => {
    const selectable = isStepSelectableInRail(
      state({ flow: flowState("DM1") }),
      "DM3a",
    );

    expect(selectable).toBe(false);
  });

  it("allows selecting a later step after backing up if artifacts show it was previously entered", () => {
    const selectable = isStepSelectableInRail(
      state({
        flow: flowState("DM1"),
        documents: [workArtifact("DM3a", "2026-04-30T12:30:00Z")],
      }),
      "DM3a",
    );

    expect(selectable).toBe(true);
  });
});
