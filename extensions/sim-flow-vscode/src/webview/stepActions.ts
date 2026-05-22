import type { GateResult } from "../cli/types";
import type { DashboardState } from "./messages";

export interface StepActionState {
  showGenerateVerilog: boolean;
  runStepEnabled: boolean;
  runStepReason: string;
  runCritiqueEnabled: boolean;
  runCritiqueReason: string;
  runGateEnabled: boolean;
  runGateReason: string;
  advanceEnabled: boolean;
  advanceReason: string;
  resetEnabled: boolean;
  resetReason: string;
}

interface DeriveStepActionInput {
  data: DashboardState;
  stepId: string;
  gateReport: GateResult | null;
}

const STEP_ORDER: Record<DashboardState["flow"]["flow"], string[]> = {
  "direct-modeling": [
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
  ],
  "design-study": ["DS0", "DS1", "DS2", "DS3", "DS4", "DS5a", "DS5b", "DS6", "DS7", "DS8", "DS9"],
  "systemverilog-convert": ["SV0", "SV0d", "SV1", "SV2", "SV3"],
};

export function deriveStepActionState(input: DeriveStepActionInput): StepActionState {
  const { data, stepId, gateReport } = input;
  const isCurrent = data.flow.current_step === stepId;
  const stepPassed = data.flow.gates[stepId]?.passed === true;
  const critique = data.critiques.find((entry) => entry.step === stepId);
  const critiqueClean = critique !== undefined && !critique.hasBlocking;
  const gateClean =
    stepPassed || (gateReport !== null && gateReport.step === stepId && gateReport.clean);
  const workCompleted = stepPassed || critique !== undefined || hasFreshWorkArtifacts(data, stepId);
  const hasResettableProgress = workCompleted || gateReport?.step === stepId || stepPassed;
  const showGenerateVerilog =
    data.flow.flow === "direct-modeling" &&
    isCurrent &&
    data.flow.gates["DM2d"]?.passed === true &&
    isStepAtOrAfter(data.flow.flow, stepId, "DM3a");

  return {
    showGenerateVerilog,
    runStepEnabled: isCurrent && !stepPassed,
    runStepReason: stepPassed
      ? "This step already passed. Reset it to run again."
      : isCurrent
        ? "Run the current step's work session."
        : "Only the current step can be run.",
    runCritiqueEnabled: isCurrent && !stepPassed && workCompleted,
    runCritiqueReason: stepPassed
      ? "This step already passed. Reset it to critique it again."
      : !isCurrent
        ? "Only the current step can be critiqued."
        : workCompleted
          ? "Review the current step's produced artifacts."
          : "Run Step first so this step produces fresh artifacts.",
    runGateEnabled: isCurrent && !stepPassed && critiqueClean,
    runGateReason: stepPassed
      ? "This step already passed. Reset it to re-run the gate."
      : !isCurrent
        ? "Only the current step can be gate-checked."
        : critique === undefined
          ? "Run Critique first."
          : critiqueClean
            ? "Validate the structural gate for this step."
            : "The critique still has BLOCKER or UNRESOLVED findings.",
    advanceEnabled: isCurrent && !stepPassed && gateClean,
    advanceReason: stepPassed
      ? "This step already passed."
      : !isCurrent
        ? "Only the current step can advance."
        : gateClean
          ? "Mark this clean step passed and move to the next step."
          : "Run Gate and get a clean result first.",
    resetEnabled: hasResettableProgress,
    resetReason: hasResettableProgress
      ? "Reset this step and clear downstream gate progress."
      : "Nothing to reset for this step yet.",
  };
}

export function isStepSelectableInRail(data: DashboardState, stepId: string): boolean {
  const order = STEP_ORDER[data.flow.flow] ?? [];
  const currentIndex = order.indexOf(data.flow.current_step);
  const stepIndex = order.indexOf(stepId);
  if (currentIndex === -1 || stepIndex === -1) {
    return false;
  }
  if (stepIndex <= currentIndex) {
    return true;
  }
  return hasVisitedStep(data, stepId);
}

function hasFreshWorkArtifacts(data: DashboardState, stepId: string): boolean {
  const workArtifacts = data.documents.filter(
    (entry) => entry.step === stepId && entry.category === "work-artifact" && entry.exists,
  );
  if (workArtifacts.length === 0) {
    return false;
  }
  const previousGateTimestamp = priorStepGateTimestamp(data, stepId);
  if (previousGateTimestamp === null) {
    return true;
  }
  return workArtifacts.some((entry) => {
    if (!entry.modifiedAt) {
      return false;
    }
    const modified = Date.parse(entry.modifiedAt);
    return Number.isFinite(modified) && modified > previousGateTimestamp;
  });
}

function hasVisitedStep(data: DashboardState, stepId: string): boolean {
  const gate = data.flow.gates[stepId];
  if (gate && (gate.passed || gate.timestamp !== null || Object.keys(gate.candidates).length > 0)) {
    return true;
  }
  if (data.critiques.some((entry) => entry.step === stepId)) {
    return true;
  }
  return data.documents.some(
    (entry) =>
      entry.step === stepId &&
      entry.exists &&
      (entry.category === "work-artifact" || entry.category === "critique"),
  );
}

function priorStepGateTimestamp(data: DashboardState, stepId: string): number | null {
  const order = STEP_ORDER[data.flow.flow] ?? [];
  const index = order.indexOf(stepId);
  if (index <= 0) {
    return null;
  }
  const previous = order[index - 1];
  const timestamp = data.flow.gates[previous]?.timestamp;
  if (!timestamp) {
    return null;
  }
  const parsed = Date.parse(timestamp);
  return Number.isFinite(parsed) ? parsed : null;
}

function isStepAtOrAfter(
  flow: DashboardState["flow"]["flow"],
  stepId: string,
  minimumStepId: string,
): boolean {
  const order = STEP_ORDER[flow] ?? [];
  const stepIndex = order.indexOf(stepId);
  const minimumIndex = order.indexOf(minimumStepId);
  return stepIndex !== -1 && minimumIndex !== -1 && stepIndex >= minimumIndex;
}
