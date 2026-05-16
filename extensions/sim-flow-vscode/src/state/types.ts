// Pure type declarations for the state readers. Split out from
// flowState.ts / critiques.ts so the webview's browser-only compile
// can reference these types without dragging Node's `fs` / `path` /
// `NodeJS.ErrnoException` types into its type environment.
//
// Implementation modules (critiques.ts, flowState.ts) re-export these
// types for convenience, so existing imports like
// `import type { CritiqueFile } from "./critiques"` keep working.

import type { StatusResult } from "../cli/types";

/** Semantic alias for clarity; `FlowState` and `StatusResult` are structurally identical. */
export type FlowState = StatusResult;

export type FindingKind = "resolved" | "unresolved" | "blocker";

export interface Finding {
  kind: FindingKind;
  /** Text after the `<KIND>:` prefix, trimmed. */
  text: string;
  /** 1-based line number in the source markdown. */
  line: number;
}

export interface CritiqueFile {
  /** Absolute path to the critique file on disk. */
  path: string;
  /** Step id derived from the filename (`DM0-critique.md` -> `DM0`). */
  step: string;
  /** Raw markdown body. */
  body: string;
  /** Findings in source order. */
  findings: Finding[];
  /** True when at least one finding is `unresolved` or `blocker`. */
  hasBlocking: boolean;
}

// ---------------------------------------------------------------
// Plan-execution progress (DM2d / DM3c / DM4b).
// ---------------------------------------------------------------

export interface PlanMilestone {
  /** Short id shown in the progress box label, e.g. "M01" or "Smoke". */
  id: string;
  /** Full title for hover tooltip + "current milestone" line. */
  title: string;
  /**
   * Absolute path to the milestone file (or the parent plan file
   * for section-driven test plans), for click-to-open.
   */
  filePath: string;
  /**
   * For section-driven plans, the line where this section's heading
   * sits inside `filePath`, so a click can scroll there.
   */
  fileLine: number | undefined;
  /** `- [x]` row count. */
  done: number;
  /** `- [-]` row count (resolved-but-not-completed). */
  deferred: number;
  /** `- [ ]` row count. */
  pending: number;
}

export interface PlanProgress {
  /**
   * Identifies which plan we're tracking; the panel uses this to
   * label the progress row.
   *
   * - "impl"  : implementation plan (`docs/plan/plan.md`).
   * - "test"  : test plan (`docs/plan/test-plan.md`).
   * - "perf"  : performance plan (`docs/plan/perf-plan.md`).
   * - "none"  : the active step doesn't drive a plan; nothing to render.
   */
  kind: "impl" | "test" | "perf" | "none";
  /** One per milestone (or per `## <Section>` for the test plan). */
  milestones: PlanMilestone[];
  /**
   * Best-guess current task description (the row text without the
   * checkbox). `null` when nothing pending or no plan.
   */
  currentTask: string | null;
  /** File the current task lives in. */
  currentTaskFilePath: string | null;
  /** Line within `currentTaskFilePath` for click-to-open. */
  currentTaskLine: number | null;
  /**
   * 1-based position of the current pending task within its
   * milestone's full task list (counts done + deferred + pending
   * rows). `null` when no current task. */
  currentTaskIndex: number | null;
  /** Total task rows in the current task's milestone (denominator
   *  for `t / T` progress indicators). `null` when no current
   *  task. */
  currentTaskTotal: number | null;
}
