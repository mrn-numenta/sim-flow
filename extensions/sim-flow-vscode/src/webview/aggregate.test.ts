import { describe, expect, it } from "vitest";

import { aggregateDashboardState } from "./aggregate";
import type { RunRow } from "../cli/types";
import type { FlowState } from "../state/flowState";

function flow(current: string = "DM0"): FlowState {
  return {
    flow: "direct-modeling",
    current_step: current,
    started: null,
    gates: {},
    archived_gates: {},
  };
}

function row(run_id: string): RunRow {
  return {
    id: 0,
    run_id,
    timestamp: "t",
    git_commit: "c",
    git_branch: null,
    git_dirty: false,
    config_fingerprint: "fp",
    manifest_path: null,
    workload: null,
    candidate: null,
    study: null,
    metrics_summary: null,
    parent_run_id: null,
    sweep_parameter: null,
    sweep_value: null,
    tags: null,
    notes: null,
    lifecycle: "active",
  };
}

describe("aggregateDashboardState", () => {
  it("passes fields through and stamps generatedAt when missing", () => {
    const out = aggregateDashboardState({
      projectDir: "/proj",
      flow: flow(),
      critiques: [],
      planProgress: { kind: "none", milestones: [], currentTask: null, currentTaskFilePath: null, currentTaskLine: null },
      runs: [],
      baselines: [],
    });
    expect(out.projectDir).toBe("/proj");
    expect(out.flow.current_step).toBe("DM0");
    expect(typeof out.generatedAt).toBe("string");
    expect(out.generatedAt.length).toBeGreaterThan(0);
  });

  it("caps runs to the configured max", () => {
    const rows = Array.from({ length: 50 }, (_, i) => row(`run-${i}`));
    const out = aggregateDashboardState({
      projectDir: "/proj",
      flow: flow(),
      critiques: [],
      planProgress: { kind: "none", milestones: [], currentTask: null, currentTaskFilePath: null, currentTaskLine: null },
      runs: rows,
      baselines: [],
      maxRuns: 10,
    });
    expect(out.runs).toHaveLength(10);
    expect(out.runs[0].run_id).toBe("run-0");
  });

  it("respects a maxRuns of 0 by emitting an empty runs array", () => {
    const out = aggregateDashboardState({
      projectDir: "/proj",
      flow: flow(),
      critiques: [],
      planProgress: { kind: "none", milestones: [], currentTask: null, currentTaskFilePath: null, currentTaskLine: null },
      runs: [row("a"), row("b")],
      baselines: [],
      maxRuns: 0,
    });
    expect(out.runs).toEqual([]);
  });

  it("preserves the caller-supplied timestamp", () => {
    const ts = "2026-04-22T00:00:00Z";
    const out = aggregateDashboardState({
      projectDir: "/proj",
      flow: flow(),
      critiques: [],
      planProgress: { kind: "none", milestones: [], currentTask: null, currentTaskFilePath: null, currentTaskLine: null },
      runs: [],
      baselines: [],
      generatedAt: ts,
    });
    expect(out.generatedAt).toBe(ts);
  });
});
