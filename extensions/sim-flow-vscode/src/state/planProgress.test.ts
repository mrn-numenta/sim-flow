import * as fs from "node:fs";
import * as path from "node:path";
import { tmpdir } from "node:os";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { readAllPlanProgress, readPlanProgress } from "./planProgress";

let projectDir: string;

beforeEach(() => {
  projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-plan-"));
});

afterEach(() => {
  fs.rmSync(projectDir, { recursive: true, force: true });
});

function writeMilestone(
  rel: string,
  rows: ReadonlyArray<"x" | " " | "-">,
  extraLines: string[] = [],
): string {
  const full = path.join(projectDir, rel);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  const body = rows.map((c, i) => `- [${c}] task ${i + 1}`).join("\n");
  const text = [body, ...extraLines].join("\n");
  fs.writeFileSync(full, text, "utf8");
  return full;
}

describe("readPlanProgress (kind selection)", () => {
  it("returns kind=none for steps that don't drive a plan", async () => {
    for (const step of ["DM0", "DM1", "DM2a", "DM2b", "DS0", ""]) {
      const got = await readPlanProgress(projectDir, step);
      expect(got.kind).toBe("none");
      expect(got.milestones).toEqual([]);
    }
  });

  it("maps DM2c / DM2cd / DM2d to the impl plan", async () => {
    writeMilestone("docs/impl-plan/milestone-01-foo.md", [" "]);
    for (const step of ["DM2c", "DM2cd", "DM2d"]) {
      const got = await readPlanProgress(projectDir, step);
      expect(got.kind).toBe("impl");
      expect(got.milestones).toHaveLength(1);
    }
  });

  it("maps DM3a / DM3ad / DM3b / DM3c to the test plan", async () => {
    writeMilestone("docs/test-plan/test-milestone-01-foo.md", [" "]);
    for (const step of ["DM3a", "DM3ad", "DM3b", "DM3c"]) {
      const got = await readPlanProgress(projectDir, step);
      expect(got.kind).toBe("test");
      expect(got.milestones).toHaveLength(1);
    }
  });

  it("maps DM4a / DM4ad / DM4b to the perf plan", async () => {
    writeMilestone("docs/perf-plan/perf-milestone-01-foo.md", [" "]);
    for (const step of ["DM4a", "DM4ad", "DM4b"]) {
      const got = await readPlanProgress(projectDir, step);
      expect(got.kind).toBe("perf");
      expect(got.milestones).toHaveLength(1);
    }
  });
});

describe("readPlanProgress (milestone parsing)", () => {
  it("returns empty milestones when the plan directory is missing", async () => {
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.kind).toBe("impl");
    expect(got.milestones).toEqual([]);
    expect(got.currentTask).toBeNull();
  });

  it("counts done / deferred / pending checkboxes", async () => {
    writeMilestone("docs/impl-plan/milestone-01-foo.md", ["x", "x", " ", " ", "-"]);
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.milestones).toHaveLength(1);
    const m = got.milestones[0];
    expect(m.done).toBe(2);
    expect(m.deferred).toBe(1);
    expect(m.pending).toBe(2);
    expect(m.id).toBe("M01");
    expect(m.title).toMatch(/^M01: foo$/);
    expect(m.filePath.endsWith("milestone-01-foo.md")).toBe(true);
  });

  it("sorts milestones lexicographically and walks letter-suffixed slices", async () => {
    writeMilestone("docs/impl-plan/milestone-02-second.md", ["x"]);
    writeMilestone("docs/impl-plan/milestone-01-first.md", ["x"]);
    writeMilestone("docs/impl-plan/milestone-02b-second-detail.md", [" "]);
    const got = await readPlanProgress(projectDir, "DM2d");
    const ids = got.milestones.map((m) => m.id);
    expect(ids).toEqual(["M01", "M02", "M02b"]);
    // Letter-suffixed milestone parses titles correctly too.
    const detail = got.milestones.find((m) => m.id === "M02b")!;
    expect(detail.title).toBe("M02b: second detail");
  });

  it("ignores files that don't match the prefix or end in -critique.md", async () => {
    writeMilestone("docs/impl-plan/milestone-01-foo.md", [" "]);
    fs.writeFileSync(
      path.join(projectDir, "docs", "impl-plan", "README.md"),
      "ignored",
      "utf8",
    );
    fs.writeFileSync(
      path.join(projectDir, "docs", "impl-plan", "milestone-01-foo-critique.md"),
      "ignored",
      "utf8",
    );
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.milestones).toHaveLength(1);
    expect(got.milestones[0].id).toBe("M01");
  });

  it("falls back to the original filename when the numeric pattern doesn't match", async () => {
    writeMilestone("docs/impl-plan/milestone-no-number.md", [" "]);
    const got = await readPlanProgress(projectDir, "DM2d");
    const m = got.milestones[0];
    // No numeric prefix means we use the filename directly.
    expect(m.id).toBe("milestone-no-number.md");
  });
});

describe("readPlanProgress (current task selection)", () => {
  it("picks the most-recently-modified milestone with pending rows", async () => {
    const older = writeMilestone("docs/impl-plan/milestone-01-foo.md", [" ", " "]);
    const newer = writeMilestone("docs/impl-plan/milestone-02-bar.md", [" "]);
    // Force a known mtime ordering: older before newer.
    const past = new Date("2024-01-01T00:00:00Z");
    const now = new Date();
    fs.utimesSync(older, past, past);
    fs.utimesSync(newer, now, now);
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.currentTaskFilePath).toBe(newer);
    expect(got.currentTask).toBe("task 1");
  });

  it("falls back to the first milestone with pending rows when mtimes don't help", async () => {
    // Both files all-checked except the last; mtime same, so the
    // mostRecent tracker never finds a pending file. The fallback
    // (`findIndex(m.pending > 0)`) should catch the trailing one.
    const f1 = writeMilestone("docs/impl-plan/milestone-01-done.md", ["x"]);
    const f2 = writeMilestone("docs/impl-plan/milestone-02-pending.md", [" "]);
    const t = new Date(0);
    fs.utimesSync(f1, t, t);
    fs.utimesSync(f2, t, t);
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.currentTaskFilePath).toBe(f2);
    expect(got.currentTask).toBe("task 1");
  });

  it("returns null currentTask when every milestone is fully resolved", async () => {
    writeMilestone("docs/impl-plan/milestone-01-foo.md", ["x", "x"]);
    writeMilestone("docs/impl-plan/milestone-02-bar.md", ["x"]);
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.currentTask).toBeNull();
    expect(got.currentTaskFilePath).toBeNull();
    expect(got.currentTaskLine).toBeNull();
  });

  it("reports the 0-indexed line of the first pending row", async () => {
    // Write the file directly so the line numbering is unambiguous.
    const filePath = path.join(projectDir, "docs/impl-plan/milestone-01-foo.md");
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(
      filePath,
      ["# header", "", "- [x] done", "- [ ] pending one", "- [ ] pending two"].join("\n"),
      "utf8",
    );
    const got = await readPlanProgress(projectDir, "DM2d");
    expect(got.currentTask).toBe("pending one");
    // header=0, blank=1, done=2, first pending=3.
    expect(got.currentTaskLine).toBe(3);
  });
});

describe("readPlanProgress (test-plan dual-prefix walk)", () => {
  it("merges tb-milestone-* and test-milestone-* into one ordered pipeline", async () => {
    writeMilestone("docs/test-plan/test-milestone-01-smoke.md", [" "]);
    writeMilestone("docs/test-plan/tb-milestone-01-payloads.md", ["x"]);
    writeMilestone("docs/test-plan/tb-milestone-02-scoreboard.md", [" "]);
    const got = await readPlanProgress(projectDir, "DM3c");
    expect(got.kind).toBe("test");
    // Lexicographic sort across both prefixes.
    expect(got.milestones.map((m) => m.id)).toEqual(["M01", "M02", "M01"]);
    // Titles include the testbench / test tag so the user can tell
    // which side of the pipeline each box belongs to.
    expect(got.milestones[0].title.startsWith("TB ")).toBe(true);
    expect(got.milestones[1].title.startsWith("TB ")).toBe(true);
    expect(got.milestones[2].title.startsWith("Test ")).toBe(true);
  });
});

describe("readAllPlanProgress", () => {
  it("returns each kind independently, with empty pipelines for missing plans", async () => {
    writeMilestone("docs/impl-plan/milestone-01-foo.md", [" "]);
    const got = await readAllPlanProgress(projectDir);
    expect(got.impl.kind).toBe("impl");
    expect(got.impl.milestones).toHaveLength(1);
    expect(got.test.kind).toBe("test");
    expect(got.test.milestones).toEqual([]);
    expect(got.perf.kind).toBe("perf");
    expect(got.perf.milestones).toEqual([]);
  });

  it("populates all three when every plan is present", async () => {
    writeMilestone("docs/impl-plan/milestone-01-impl.md", [" "]);
    writeMilestone("docs/test-plan/test-milestone-01-test.md", [" "]);
    writeMilestone("docs/perf-plan/perf-milestone-01-perf.md", [" "]);
    const got = await readAllPlanProgress(projectDir);
    expect(got.impl.milestones).toHaveLength(1);
    expect(got.test.milestones).toHaveLength(1);
    expect(got.perf.milestones).toHaveLength(1);
  });
});
