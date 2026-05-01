// Read plan-execution progress from the on-disk plan files. Used by
// the dashboard to render a milestone progress row + current-task
// line under the per-step buttons.
//
// Three plan shapes coexist in `docs/plan/`:
//
//   1. Implementation plan (DM2c writes, DM2d executes):
//      `docs/plan/plan.md` index + `docs/plan/milestone-NN-<name>.md`
//      files. Each milestone file is one progress box.
//
//   2. Test plan (DM3a writes, DM3c executes):
//      `docs/plan/test-plan.md` -- a single file with `## Smoke`,
//      `## Edge`, `## Stress`, `## Random` sections. Each section is
//      one progress box.
//
//   3. Performance plan (DM4a writes, DM4b executes):
//      `docs/plan/perf-plan.md` index + `docs/plan/perf-milestone-NN-<name>.md`
//      files. One box per milestone.
//
// The "current task" is a best-guess heuristic: for plans split
// across multiple milestone files, we pick the most-recently-modified
// file and return its first un-checked `- [ ]` row; for the
// single-file test plan, we walk the file and return the first
// un-checked row across all sections. Falls back to "first un-checked
// in plan order" when modification times don't help (e.g. the agent
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
 * Map `current_step` to which plan file applies. Steps that don't
 * execute a plan (DM0/1/2a/2b/2c/3a/3b/4a or any DS step) return
 * "none" so the dashboard hides the progress row.
 */
function planKindForStep(currentStep: string): PlanProgress["kind"] {
  switch (currentStep) {
    case "DM2d":
      return "impl";
    case "DM3c":
      return "test";
    case "DM4b":
      return "perf";
    default:
      return "none";
  }
}

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
  if (kind === "impl") {
    return await readMilestoneDirPlan(projectDir, kind, "milestone-");
  }
  if (kind === "perf") {
    return await readMilestoneDirPlan(projectDir, kind, "perf-milestone-");
  }
  // kind === "test": single file with `## <Category>` sections.
  return await readSectionedTestPlan(projectDir);
}

/**
 * Read a milestone-per-file plan: every `<prefix>NN-<name>.md` under
 * `docs/plan/` is one milestone box. Sorted by NN.
 */
async function readMilestoneDirPlan(
  projectDir: string,
  kind: "impl" | "perf",
  prefix: "milestone-" | "perf-milestone-",
): Promise<PlanProgress> {
  const planDir = path.join(projectDir, "docs", "plan");
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
        name.startsWith(prefix) &&
        name.endsWith(".md") &&
        !name.endsWith("-critique.md"),
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
    // Extract `NN` from `<prefix>NN-<name>.md` for the id label.
    const numMatch = name.slice(prefix.length).match(/^(\d+)-(.+)\.md$/);
    const numLabel = numMatch ? `M${numMatch[1]}` : name;
    const titleSuffix = numMatch
      ? numMatch[2].replace(/-/g, " ")
      : name.replace(/\.md$/, "");
    milestones.push({
      id: numLabel,
      title: `${numLabel}: ${titleSuffix}`,
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
  const targetIdx =
    mostRecent?.index ??
    milestones.findIndex((m) => m.pending > 0);
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

const TEST_SECTIONS: ReadonlyArray<string> = [
  "Smoke",
  "Edge",
  "Stress",
  "Random",
];

/**
 * Read DM3a's test plan (single file, `## <Category>` sections).
 * Each of Smoke / Edge / Stress / Random becomes one progress box.
 * Sections we don't recognize (e.g. `## Coverage`, `## Traceability`)
 * are skipped -- those don't have completion semantics.
 */
async function readSectionedTestPlan(
  projectDir: string,
): Promise<PlanProgress> {
  const filePath = path.join(projectDir, "docs", "plan", "test-plan.md");
  if (!existsSync(filePath)) {
    return { ...EMPTY, kind: "test" };
  }
  let body: string;
  try {
    body = await fs.readFile(filePath, "utf8");
  } catch {
    return { ...EMPTY, kind: "test" };
  }
  const milestones: PlanMilestone[] = [];
  let firstPending: { text: string; line: number } | null = null;
  for (const section of TEST_SECTIONS) {
    const sliceInfo = sliceSection(body, section);
    if (!sliceInfo) {
      // The plan is missing this section. Surface the box as
      // empty so the user sees the gap.
      milestones.push({
        id: section,
        title: `${section} (section missing)`,
        filePath,
        fileLine: undefined,
        done: 0,
        deferred: 0,
        pending: 0,
      });
      continue;
    }
    const counts = countCheckboxes(sliceInfo.body);
    milestones.push({
      id: section,
      title: section,
      filePath,
      fileLine: sliceInfo.headingLine,
      done: counts.done,
      deferred: counts.deferred,
      pending: counts.pending,
    });
    if (!firstPending) {
      const row = firstPendingRow(sliceInfo.body);
      if (row) {
        firstPending = {
          text: row.text,
          line: sliceInfo.bodyStartLine + row.line,
        };
      }
    }
  }
  return {
    kind: "test",
    milestones,
    currentTask: firstPending?.text ?? null,
    currentTaskFilePath: firstPending ? filePath : null,
    currentTaskLine: firstPending?.line ?? null,
  };
}

interface SectionSlice {
  /** Section body (everything between this heading and the next). */
  body: string;
  /** 0-indexed line number of the `## <name>` heading in the parent file. */
  headingLine: number;
  /** 0-indexed line number where `body` begins in the parent file. */
  bodyStartLine: number;
}

/**
 * Pull the body of a `## <name>` section out of `text`. Returns
 * `null` if the heading is missing. Heading match is
 * case-insensitive on the section name and tolerates extra heading
 * adornments (e.g. `## Smoke (15 tests)`).
 */
function sliceSection(text: string, name: string): SectionSlice | null {
  const lines = text.split("\n");
  const pattern = new RegExp(`^##\\s+${name}\\b`, "i");
  let headingLine = -1;
  for (let i = 0; i < lines.length; i++) {
    if (pattern.test(lines[i])) {
      headingLine = i;
      break;
    }
  }
  if (headingLine < 0) {
    return null;
  }
  const bodyStart = headingLine + 1;
  let bodyEnd = lines.length;
  for (let i = bodyStart; i < lines.length; i++) {
    if (/^##\s+/.test(lines[i])) {
      bodyEnd = i;
      break;
    }
  }
  return {
    body: lines.slice(bodyStart, bodyEnd).join("\n"),
    headingLine,
    bodyStartLine: bodyStart,
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
    if (ch === " ") {counts.pending++;}
    else if (ch === "x" || ch === "X") {counts.done++;}
    else if (ch === "-") {counts.deferred++;}
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
