/**
 * Disk / CLI loaders for the dashboard state. Each function wraps
 * an orchestrator CLI call and returns a defensible empty value on
 * failure so the dashboard keeps rendering the rest of project
 * state. Extracted from `webview/host.ts` to keep that file under
 * the refactor threshold.
 */

import type { SimFlowCli } from "../../cli/simflow";
import type { CritiqueFile, FlowState, PlanProgress } from "../../state/types";
import type { DashboardState, DocumentEntry } from "../messages";
import { MAX_DASHBOARD_RUNS } from "../host";

export async function loadDocuments(cli: SimFlowCli, flow: string): Promise<DocumentEntry[]> {
  // MVP architecture: project-documents enumeration goes through
  // `sim-flow documents --flow <flow>`. The orchestrator owns
  // the STEP_ARTIFACTS table, the directory walker, and the
  // markdown-table preview extraction. Empty array on CLI
  // failure keeps the dashboard rendering the rest of project
  // state.
  try {
    return await cli.documents(flow);
  } catch {
    return [];
  }
}

export async function loadCritiques(cli: SimFlowCli): Promise<CritiqueFile[]> {
  // MVP architecture: critique enumeration goes through
  // `sim-flow critiques --json`. The orchestrator owns the
  // JSON+markdown parsing so any UI surface consumes the same
  // structured shape. Empty array on CLI failure keeps the rest
  // of the dashboard rendering.
  try {
    return await cli.critiques();
  } catch {
    return [];
  }
}

export async function loadPlanProgress(
  cli: SimFlowCli,
  currentStep: string,
): Promise<PlanProgress> {
  // MVP architecture: plan-progress walks come from
  // `sim-flow plan-progress --current-step <step>` (orchestrator
  // owns the milestone-file parser). Falls back to a `kind: none`
  // shape on CLI failure so the dashboard renders the rest of
  // project state.
  try {
    return await cli.planProgress(currentStep);
  } catch {
    return {
      kind: "none",
      milestones: [],
      currentTask: null,
      currentTaskFilePath: null,
      currentTaskLine: null,
      currentTaskIndex: null,
      currentTaskTotal: null,
    };
  }
}

export async function loadAllPlanProgress(cli: SimFlowCli): Promise<{
  impl: PlanProgress;
  test: PlanProgress;
  perf: PlanProgress;
}> {
  try {
    return await cli.planProgressAll();
  } catch {
    const empty = (kind: PlanProgress["kind"]): PlanProgress => ({
      kind,
      milestones: [],
      currentTask: null,
      currentTaskFilePath: null,
      currentTaskLine: null,
      currentTaskIndex: null,
      currentTaskTotal: null,
    });
    return {
      impl: empty("impl"),
      test: empty("test"),
      perf: empty("perf"),
    };
  }
}

export async function loadFlowState(cli: SimFlowCli): Promise<FlowState> {
  // MVP architecture: flow state flows through the orchestrator
  // CLI (`sim-flow status --json`), not via direct state.toml
  // parse. `StatusResult` is structurally identical to `FlowState`
  // (type alias in src/state/types.ts). Falls back to an "empty"
  // state on CLI failure so the dashboard still renders something
  // navigable even when the orchestrator is unreachable.
  try {
    return await cli.status();
  } catch {
    return {
      flow: "direct-modeling",
      current_step: "DM0",
      started: null,
      gates: {},
      archived_gates: {},
    };
  }
}

export async function loadRuns(cli: SimFlowCli): Promise<DashboardState["runs"]> {
  // MVP architecture: all run data flows through the orchestrator
  // CLI (`sim-flow runs --json`), not via direct experiments.db
  // open. Decouples the extension from better-sqlite3 + the DB
  // schema, and lets future UI surfaces consume the same data
  // path. Empty array on CLI failure so the dashboard still
  // renders the rest of project state.
  try {
    return await cli.runs({ limit: MAX_DASHBOARD_RUNS });
  } catch {
    return [];
  }
}

export async function loadBaselines(cli: SimFlowCli): Promise<DashboardState["baselines"]> {
  // Orchestrator-mediated (see `loadRuns`).
  try {
    return await cli.baselineList();
  } catch {
    return [];
  }
}

export async function readCoverageState(
  projectDir: string,
): Promise<import("../messages").CoverageState> {
  try {
    const { readCoverageSettings } = await import("../../state/projectConfig");
    const settings = await readCoverageSettings(projectDir);
    return { thresholdPct: settings.thresholdPct, level: settings.level };
  } catch {
    // Don't surface read failures: the agent side will catch a
    // malformed file when it next loads the config, and the
    // user can still edit fields here to overwrite a broken
    // section.
    return { thresholdPct: 90, level: "total" };
  }
}
