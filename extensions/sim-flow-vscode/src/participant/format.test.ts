import { describe, expect, it } from "vitest";

import {
  formatBaselinesMarkdown,
  formatGateMarkdown,
  formatRunsMarkdown,
  formatStatusMarkdown,
} from "./format";

describe("formatStatusMarkdown", () => {
  it("renders an empty-gate state as a bulleted header with a note", () => {
    const md = formatStatusMarkdown({
      flow: "direct-modeling",
      current_step: "DM0",
      started: null,
      gates: {},
      archived_gates: {},
    });
    expect(md).toContain("current step:");
    expect(md).toContain("(no gates recorded)");
  });

  it("renders a gate table and per-candidate rows", () => {
    const md = formatStatusMarkdown({
      flow: "design-study",
      current_step: "DS5a",
      started: "2026-04-22T00:00:00Z",
      gates: {
        DS4: { passed: true, timestamp: "t1", candidates: {} },
        DS5a: {
          passed: false,
          timestamp: null,
          candidates: {
            "mesh-noc": { passed: true, timestamp: "t2", candidates: {} },
            "ring-noc": { passed: false, timestamp: null, candidates: {} },
          },
        },
      },
      archived_gates: {},
    });
    expect(md).toContain("| DS4 |");
    expect(md).toContain("| DS5a |");
    expect(md).toContain("↳ mesh-noc");
    expect(md).toContain("↳ ring-noc");
  });
});

describe("formatGateMarkdown", () => {
  it("reports clean gates with a check mark", () => {
    const md = formatGateMarkdown({ step: "DM0", clean: true, failures: [] });
    expect(md).toContain("✅");
    expect(md).toContain("DM0");
  });

  it("lists every failure when the gate is not clean", () => {
    const md = formatGateMarkdown({
      step: "DM0",
      clean: false,
      failures: [
        { description: "spec.md missing", reason: "no such file" },
        { description: "critique missing", reason: "no critique" },
      ],
    });
    expect(md).toContain("❌ 2 failure(s)");
    expect(md).toContain("spec.md missing");
    expect(md).toContain("critique missing");
  });
});

describe("formatRunsMarkdown", () => {
  it("prints an empty-state note when no rows match", () => {
    expect(formatRunsMarkdown([])).toContain("no runs");
  });

  it("truncates long git commits and flags dirty", () => {
    const md = formatRunsMarkdown([
      {
        id: 1,
        run_id: "001-a",
        timestamp: "t",
        git_commit: "deadbeefcafef00d",
        git_branch: null,
        git_dirty: true,
        config_fingerprint: "fp",
        manifest_path: null,
        workload: "wk",
        candidate: null,
        study: null,
        metrics_summary: null,
        parent_run_id: null,
        sweep_parameter: null,
        sweep_value: null,
        tags: null,
        notes: null,
        lifecycle: "active",
      },
    ]);
    expect(md).toContain("deadbeef");
    expect(md).toContain("(dirty)");
    expect(md).not.toContain("cafef00d");
  });
});

describe("formatBaselinesMarkdown", () => {
  it("notes when no baselines are defined", () => {
    expect(formatBaselinesMarkdown([])).toContain("no baselines");
  });

  it("renders each baseline as a row", () => {
    const md = formatBaselinesMarkdown([
      { name: "v1", run_id: "001-a", timestamp: "t" },
      { name: "v2", run_id: "002-b", timestamp: "u" },
    ]);
    expect(md).toContain("| v1 |");
    expect(md).toContain("| v2 |");
  });
});
