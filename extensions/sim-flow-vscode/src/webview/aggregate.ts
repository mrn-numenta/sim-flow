// Pure aggregation helpers factored out of DashboardHost so they can
// be unit-tested without loading the vscode module. The host calls
// these to assemble the DashboardState payload it posts to the
// webview.

import type { BaselineRecord, RunRow } from "../cli/types";
import type { CritiqueFile, FlowState, PlanProgress } from "../state/types";
import type { DashboardState, DocumentEntry } from "./messages";

export interface AggregateInput {
  projectDir: string;
  flow: FlowState;
  critiques: CritiqueFile[];
  runs: RunRow[];
  baselines: BaselineRecord[];
  documents: DocumentEntry[];
  planProgress: PlanProgress;
  /** Persisted spec path; empty string when nothing is recorded. */
  specPath?: string;
  /** Mirrors `sim-flow.dashboard.showFullyAutomated`. Defaults to false. */
  fullyAutomatedEnabled?: boolean;
  /** Mirrors `sim-flow.dashboard.verilogSimEnabled`. Defaults to false. */
  verilogSimEnabled?: boolean;
  /** Mirrors `sim-flow.dashboard.verilogSimulatorPath`. Defaults to "". */
  verilogSimulatorPath?: string;
  generatedAt?: string;
  cliVersion?: string;
  maxRuns?: number;
}

/**
 * Produce the `DashboardState` payload given already-loaded inputs.
 * Caps `runs` to `maxRuns` entries (default 200) newest-first.
 */
export function aggregateDashboardState(input: AggregateInput): DashboardState {
  const max = input.maxRuns ?? 200;
  const runs = input.runs.slice(0, Math.max(0, max));
  return {
    projectDir: input.projectDir,
    flow: input.flow,
    critiques: input.critiques,
    runs,
    baselines: input.baselines,
    documents: input.documents,
    planProgress: input.planProgress,
    specPath: input.specPath ?? "",
    fullyAutomatedEnabled: input.fullyAutomatedEnabled ?? false,
    verilogSimEnabled: input.verilogSimEnabled ?? false,
    verilogSimulatorPath: input.verilogSimulatorPath ?? "",
    generatedAt: input.generatedAt ?? new Date().toISOString(),
    cliVersion: input.cliVersion,
  };
}
