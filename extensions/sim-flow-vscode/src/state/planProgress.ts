// Read plan-execution progress from the on-disk plan files. Used by
// the dashboard to render a milestone progress row + current-task
// line under the per-step buttons.
//
// Three plan shapes are walked, all milestone-per-file:
//
//   1. Implementation plan (DM2c outline + DM2cd detail write,
//      DM2d executes): `docs/impl-plan/plan.md` index +
//      `docs/impl-plan/milestone-NN-<name>.md` files. One box
//      per milestone.
//
//   2. Test plan (DM3a outline + DM3ad detail write, DM3b
//      executes the testbench scaffolding, DM3c executes tests):
//      `docs/test-plan/test-plan.md` index +
//      `docs/test-plan/tb-milestone-NN-*.md` (DM3b's slices) AND
//      `docs/test-plan/test-milestone-NN-*.md` (DM3c's slices).
//      Each milestone file is one box; both prefixes share the
//      same kind so the user sees the testbench + test work as
//      one pipeline.
//
//   3. Performance plan (DM4a outline + DM4ad detail write, DM4b
//      executes): `docs/perf-plan/perf-plan.md` index +
//      `docs/perf-plan/perf-milestone-NN-<name>.md` files.
//
// The "current task" is a best-guess heuristic: pick the
// most-recently-modified milestone file and return its first
// un-checked `- [ ]` row. Falls back to "first un-checked in plan
// order" when modification times don't help (e.g. the agent
// hasn't started yet).

import { existsSync, statSync } from "node:fs";
import { promises as fs } from "node:fs";
import * as path from "node:path";

import type { PlanMilestone, PlanProgress } from "./types";

// Re-export for convenience: existing TS imports from
// `state/planProgress` keep working without rewriting paths.
export type { PlanMilestone, PlanProgress } from "./types";

const EMPTY: PlanProgress = {
  kind: "none",
  milestones: [],
  currentTask: null,
  currentTaskFilePath: null,
  currentTaskLine: null,
};

/**
 * Map `current_step` to which plan file applies. Outline + detail
 * steps share their plan with the execution step (DM2c / DM2cd
 * with DM2d, DM3a / DM3ad with DM3b / DM3c, DM4a / DM4ad with
 * DM4b), so any step in the phase returns the matching kind.
 * Steps that don't drive a plan (DM0 / DM1 / DM2a / DM2b or any
 * DS step) return "none".
 */
function planKindForStep(currentStep: string): PlanProgress["kind"] {
  switch (currentStep) {
    case "DM2c":
    case "DM2cd":
    case "DM2d":
      return "impl";
    case "DM3a":
    case "DM3ad":
    case "DM3b":
    case "DM3c":
      return "test";
    case "DM4a":
    case "DM4ad":
    case "DM4b":
      return "perf";
    default:
      return "none";
  }
}

/** Per-kind config: which directory + filename prefix(es) hold
 *  the milestone files. Test plan walks two prefixes
 *  (`tb-milestone-` for DM3b's testbench scaffolding +
 *  `test-milestone-` for DM3c's test execution); both feed one
 *  pipeline. */
const PLAN_DIR_CONFIG: Record<"impl" | "test" | "perf", { dir: string; prefixes: string[] }> = {
  impl: { dir: path.join("docs", "impl-plan"), prefixes: ["milestone-"] },
  test: {
    dir: path.join("docs", "test-plan"),
    prefixes: ["tb-milestone-", "test-milestone-"],
  },
  perf: { dir: path.join("docs", "perf-plan"), prefixes: ["perf-milestone-"] },
};

/**
 * Compute progress for the project's active plan. Safe to call when
 * the plan file doesn't exist yet -- returns `EMPTY` (kind: "none").
 */
export async function readPlanProgress(
  projectDir: string,
  currentStep: string,
): Promise<PlanProgress> {
  const kind = planKindForStep(currentStep);
  if (kind === "none") {
    return EMPTY;
  }
  return await readMilestoneDirPlan(projectDir, kind);
}

/**
 * Compute progress for ALL plan kinds in one pass. The dashboard
 * uses this so a click on any plan-related step (outline, detail,
 * or execution) shows the milestone pipeline regardless of which
 * step is `current_step`. Each kind's progress is independently
 * scanned; missing-on-disk plans land as `kind: <kind>` with
 * empty milestones.
 */
export async function readAllPlanProgress(projectDir: string): Promise<{
  impl: PlanProgress;
  test: PlanProgress;
  perf: PlanProgress;
}> {
  const [impl, test, perf] = await Promise.all([
    readMilestoneDirPlan(projectDir, "impl"),
    readMilestoneDirPlan(projectDir, "test"),
    readMilestoneDirPlan(projectDir, "perf"),
  ]);
  return { impl, test, perf };
}

/**
 * Read a milestone-per-file plan: every `<prefix>NN-<name>.md`
 * under the kind's plan directory is one milestone box. Sorted
 * lexicographically across all configured prefixes (so
 * `tb-milestone-01-*.md` comes before `test-milestone-01-*.md`
 * for the test plan's combined pipeline).
 */
async function readMilestoneDirPlan(
  projectDir: string,
  kind: "impl" | "test" | "perf",
): Promise<PlanProgress> {
  const cfg = PLAN_DIR_CONFIG[kind];
  const planDir = path.join(projectDir, cfg.dir);
  if (!existsSync(planDir)) {
    return { ...EMPTY, kind };
  }
  let entries: string[];
  try {
    entries = await fs.readdir(planDir);
  } catch {
    return { ...EMPTY, kind };
  }
  const milestoneFiles = entries
    .filter(
      (name) =>
        name.endsWith(".md") &&
        !name.endsWith("-critique.md") &&
        cfg.prefixes.some((p) => name.startsWith(p)),
    )
    .sort();
  if (milestoneFiles.length === 0) {
    return { ...EMPTY, kind };
  }
  const milestones: PlanMilestone[] = [];
  let mostRecent: { mtimeMs: number; index: number } | null = null;
  for (let i = 0; i < milestoneFiles.length; i++) {
    const name = milestoneFiles[i];
    const filePath = path.join(planDir, name);
    let body: string;
    let mtimeMs = 0;
    try {
      body = await fs.readFile(filePath, "utf8");
      mtimeMs = statSync(filePath).mtimeMs;
    } catch {
      continue;
    }
    const counts = countCheckboxes(body);
    // Extract the milestone numeric label, allowing letter-suffixed
    // splits (`tb-milestone-02b-edge.md` -> `M02b`). The matched
    // prefix is the longest prefix in `cfg.prefixes` that the
    // filename starts with -- otherwise `tb-milestone-` would
    // shadow `test-milestone-` for files starting with `tb-`.
    const matchedPrefix = cfg.prefixes
      .filter((p) => name.startsWith(p))
      .reduce((a, b) => (a.length >= b.length ? a : b), "");
    const remainder = name.slice(matchedPrefix.length);
    const numMatch = remainder.match(/^(\d+[a-z]?)-(.+)\.md$/);
    const numLabel = numMatch ? `M${numMatch[1]}` : name;
    const titleSuffix = numMatch ? numMatch[2].replace(/-/g, " ") : name.replace(/\.md$/, "");
    // Differentiate testbench-milestone vs test-execution-milestone
    // entries in the title so the user can tell them apart at a
    // glance when both walk the same pipeline.
    const prefixTag =
      matchedPrefix === "tb-milestone-"
        ? "TB"
        : matchedPrefix === "test-milestone-"
          ? "Test"
          : null;
    const fullTitle = prefixTag
      ? `${prefixTag} ${numLabel}: ${titleSuffix}`
      : `${numLabel}: ${titleSuffix}`;
    milestones.push({
      id: numLabel,
      title: fullTitle,
      filePath,
      fileLine: undefined,
      done: counts.done,
      deferred: counts.deferred,
      pending: counts.pending,
    });
    // Track the most-recently-modified milestone file as the
    // best-guess "agent is currently here" indicator.
    if (counts.pending > 0 && (!mostRecent || mtimeMs > mostRecent.mtimeMs)) {
      mostRecent = { mtimeMs, index: milestones.length - 1 };
    }
  }
  // Pick the current-task source: most-recent-with-pending if we
  // saw one, else the first milestone with any pending row.
  const targetIdx = mostRecent?.index ?? milestones.findIndex((m) => m.pending > 0);
  if (targetIdx < 0 || targetIdx >= milestones.length) {
    return {
      kind,
      milestones,
      currentTask: null,
      currentTaskFilePath: null,
      currentTaskLine: null,
    };
  }
  const target = milestones[targetIdx];
  let task: { text: string; line: number } | null = null;
  try {
    const body = await fs.readFile(target.filePath, "utf8");
    task = firstPendingRow(body);
  } catch {
    task = null;
  }
  return {
    kind,
    milestones,
    currentTask: task?.text ?? null,
    currentTaskFilePath: task ? target.filePath : null,
    currentTaskLine: task?.line ?? null,
  };
}

interface CheckboxCounts {
  done: number;
  deferred: number;
  pending: number;
}

/**
 * Count `- [ ]`, `- [x]`, and `- [-]` rows in `text`. Tolerates
 * leading whitespace and `*` instead of `-`.
 */
function countCheckboxes(text: string): CheckboxCounts {
  const counts: CheckboxCounts = { done: 0, deferred: 0, pending: 0 };
  const re = /^\s*[-*]\s+\[([ xX-])\]/gm;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const ch = m[1];
    if (ch === " ") {
      counts.pending++;
    } else if (ch === "x" || ch === "X") {
      counts.done++;
    } else if (ch === "-") {
      counts.deferred++;
    }
  }
  return counts;
}

/**
 * Return the first `- [ ]` row in `text`: its text (without the
 * checkbox prefix) and its 0-indexed line number. `null` when none
 * remain.
 */
function firstPendingRow(text: string): { text: string; line: number } | null {
  const lines = text.split("\n");
  const re = /^\s*[-*]\s+\[\s\]\s+(.+)$/;
  for (let i = 0; i < lines.length; i++) {
    const m = re.exec(lines[i]);
    if (m) {
      return { text: m[1].trim(), line: i };
    }
  }
  return null;
}
