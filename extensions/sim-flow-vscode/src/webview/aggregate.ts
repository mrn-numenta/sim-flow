// Pure aggregation helpers factored out of DashboardHost so they can
// be unit-tested without loading the vscode module. The host calls
// these to assemble the DashboardState payload it posts to the
// webview.

import type { BaselineRecord, RunRow } from "../cli/types";
import type { StepMode } from "../session/protocol-types";
import type { CritiqueFile, FlowState, PlanProgress } from "../state/types";
import type { CoverageState, DashboardState, DocumentEntry, LlmServerEntry } from "./messages";

export interface AggregateInput {
  projectDir: string;
  flow: FlowState;
  critiques: CritiqueFile[];
  runs: RunRow[];
  baselines: BaselineRecord[];
  documents: DocumentEntry[];
  planProgress: PlanProgress;
  /**
   * Per-kind plan progress so the dashboard's per-step view can
   * surface the milestone pipeline under any plan-related step
   * (DM2c outline / DM2cd detail / DM2d execution, etc.) regardless
   * of `current_step`. Optional so older / partial host snapshots
   * still aggregate without it.
   */
  planProgressByKind?: {
    impl: PlanProgress;
    test: PlanProgress;
    perf: PlanProgress;
  };
  /** Persisted spec path; empty string when nothing is recorded. */
  specPath?: string;
  /** Mirrors `sim-flow.dashboard.showFullyAutomated`. Defaults to false. */
  fullyAutomatedEnabled?: boolean;
  /** Mirrors `sim-flow.dashboard.verilogSimEnabled`. Defaults to false. */
  verilogSimEnabled?: boolean;
  /** Mirrors `sim-flow.dashboard.verilogSimulatorPath`. Defaults to "". */
  verilogSimulatorPath?: string;
  /** Mirrors `sim-flow.llm.servers`. Defaults to []. */
  llmServers?: LlmServerEntry[];
  /** Mirrors `[coverage]` in `.sim-flow/config.toml`. Defaults to 90% / total. */
  coverage?: CoverageState;
  /**
   * Resolved step-axis mode: orchestrator's last `StepModeChanged`
   * truth when a session is attached, otherwise the persisted
   * `sim-flow.flow.stepMode` setting. Defaults to `"manual"`.
   */
  stepMode?: StepMode;
  /** True when a `SocketSessionPump` is alive for this project. */
  sessionActive?: boolean;
  /**
   * True while the orchestrator is inside a sub-session (Work or
   * Critique). The dashboard reads this to disable per-step buttons
   * while the orchestrator is busy. Defaults to `false`.
   */
  inSubSession?: boolean;
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
    ...(input.planProgressByKind !== undefined
      ? { planProgressByKind: input.planProgressByKind }
      : {}),
    specPath: input.specPath ?? "",
    fullyAutomatedEnabled: input.fullyAutomatedEnabled ?? false,
    verilogSimEnabled: input.verilogSimEnabled ?? false,
    verilogSimulatorPath: input.verilogSimulatorPath ?? "",
    llmServers: input.llmServers ?? [],
    coverage: input.coverage ?? { thresholdPct: 90, level: "total" },
    stepMode: input.stepMode ?? "manual",
    sessionActive: input.sessionActive ?? false,
    inSubSession: input.inSubSession ?? false,
    generatedAt: input.generatedAt ?? new Date().toISOString(),
    cliVersion: input.cliVersion,
  };
}
