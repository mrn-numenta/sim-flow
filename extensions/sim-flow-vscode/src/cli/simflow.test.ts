import { describe, expect, it } from "vitest";

import { SimFlowCliError } from "./errors";
import type { Execute } from "./executor";
import { SimFlowCli } from "./simflow";

function makeCli(
  execute: Execute,
  overrides: { foundationRoot?: string; projectDir?: string } = {},
): SimFlowCli {
  return new SimFlowCli(
    {
      binary: "/fake/sim-flow",
      projectDir: overrides.projectDir ?? "/proj",
      foundationRoot: overrides.foundationRoot,
    },
    execute,
  );
}

function capturing(
  stdout: string,
  stderr = "",
): { execute: Execute; calls: Array<[string, string[]]> } {
  const calls: Array<[string, string[]]> = [];
  const execute: Execute = async (bin, args) => {
    calls.push([bin, args]);
    return { stdout, stderr };
  };
  return { execute, calls };
}

describe("SimFlowCli.status", () => {
  it("parses a valid status payload", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        flow: "direct-modeling",
        current_step: "DM0",
        started: null,
        gates: {},
        archived_gates: {},
      }),
    );
    const cli = makeCli(execute);
    const result = await cli.status();
    expect(result.flow).toBe("direct-modeling");
    expect(result.current_step).toBe("DM0");
    expect(calls).toEqual([["/fake/sim-flow", ["--project", "/proj", "status", "--json"]]]);
  });

  it("forwards --foundation-root when set", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        flow: "design-study",
        current_step: "DS0",
        started: null,
        gates: {},
        archived_gates: {},
      }),
    );
    const cli = makeCli(execute, { foundationRoot: "/sf" });
    await cli.status();
    expect(calls[0][1]).toEqual([
      "--foundation-root",
      "/sf",
      "--project",
      "/proj",
      "status",
      "--json",
    ]);
  });

  it("ignores empty foundation-root strings", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        flow: "direct-modeling",
        current_step: "DM0",
        started: null,
        gates: {},
        archived_gates: {},
      }),
    );
    const cli = makeCli(execute, { foundationRoot: "   " });
    await cli.status();
    expect(calls[0][1]).not.toContain("--foundation-root");
  });

  it("throws a typed error on non-JSON stdout", async () => {
    const { execute } = capturing("not json at all");
    const cli = makeCli(execute);
    await expect(cli.status()).rejects.toBeInstanceOf(SimFlowCliError);
    await expect(cli.status()).rejects.toMatchObject({ kind: "json-parse-failed" });
  });

  it("throws a typed error on empty stdout", async () => {
    const { execute } = capturing("");
    const cli = makeCli(execute);
    await expect(cli.status()).rejects.toMatchObject({ kind: "unexpected-stdout" });
  });
});

describe("SimFlowCli.runs", () => {
  it("forwards all supported filters", async () => {
    const { execute, calls } = capturing("[]");
    const cli = makeCli(execute);
    await cli.runs({
      workload: "throughput",
      candidate: "mesh",
      study: "noc",
      sweep: "001-parent",
      limit: 5,
    });
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "runs",
      "--json",
      "--workload",
      "throughput",
      "--candidate",
      "mesh",
      "--study",
      "noc",
      "--sweep",
      "001-parent",
      "--limit",
      "5",
    ]);
  });

  it("returns parsed rows", async () => {
    const { execute } = capturing(
      JSON.stringify([
        {
          id: 1,
          run_id: "001-a",
          timestamp: "t",
          git_commit: "c",
          git_branch: null,
          git_dirty: false,
          config_fingerprint: "fp",
          manifest_path: null,
          workload: "w",
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
      ]),
    );
    const cli = makeCli(execute);
    const rows = await cli.runs();
    expect(rows).toHaveLength(1);
    expect(rows[0].run_id).toBe("001-a");
    expect(rows[0].workload).toBe("w");
  });
});

describe("SimFlowCli.gate", () => {
  it("parses a clean gate result", async () => {
    const { execute } = capturing(JSON.stringify({ step: "DM0", clean: true, failures: [] }));
    const cli = makeCli(execute);
    const result = await cli.gate("DM0");
    expect(result.clean).toBe(true);
    expect(result.failures).toEqual([]);
  });

  it("tolerates non-zero exit codes carrying a failure JSON payload", async () => {
    const payload = JSON.stringify({
      step: "DM0",
      clean: false,
      failures: [{ description: "spec.md missing", reason: "no such file" }],
    });
    const execute: Execute = async () => {
      const err: Error & { code?: number; stdout?: string; stderr?: string } = new Error("exit 1");
      err.code = 1;
      err.stdout = payload;
      err.stderr = "";
      throw err;
    };
    const cli = makeCli(execute);
    const result = await cli.gate("DM0");
    expect(result.clean).toBe(false);
    expect(result.failures).toHaveLength(1);
    expect(result.failures[0].description).toBe("spec.md missing");
  });

  it("propagates spawn failures that lack a JSON payload", async () => {
    const execute: Execute = async () => {
      throw new Error("ENOENT");
    };
    const cli = makeCli(execute);
    await expect(cli.gate("DM0")).rejects.toMatchObject({ kind: "non-zero-exit" });
  });
});

describe("SimFlowCli.baseline*", () => {
  it("create sends --run and --notes when supplied", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({ name: "v1", run_id: "001-a", timestamp: "t" }),
    );
    const cli = makeCli(execute);
    await cli.baselineCreate("v1", "001-a", "initial");
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "baseline",
      "create",
      "v1",
      "--run",
      "001-a",
      "--notes",
      "initial",
      "--json",
    ]);
  });

  it("compare parses deltas with null entries", async () => {
    const { execute } = capturing(
      JSON.stringify({
        baseline_run_id: "a",
        current_run_id: "b",
        entries: [
          {
            metric: "throughput",
            baseline: 0.8,
            current: 0.9,
            delta: 0.1,
            delta_pct: 12.5,
          },
          {
            metric: "latency_p99",
            baseline: null,
            current: 10,
            delta: null,
            delta_pct: null,
          },
        ],
      }),
    );
    const cli = makeCli(execute);
    const delta = await cli.baselineCompare("v1");
    expect(delta.entries).toHaveLength(2);
    expect(delta.entries[1].delta).toBeNull();
  });

  it("list returns an empty array when no baselines exist", async () => {
    const { execute } = capturing("[]");
    const cli = makeCli(execute);
    expect(await cli.baselineList()).toEqual([]);
  });
});

describe("SimFlowCli.newModel", () => {
  it("threads options through to argv", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        project_dir: "/out/smoke-model",
        crate_name: "smoke_model",
        next_step: "DM0",
      }),
    );
    const cli = makeCli(execute);
    await cli.newModel({
      name: "smoke-model",
      destination: "/out",
      libraryPath: "../lib",
      skipCargoCheck: true,
    });
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "new",
      "model",
      "smoke-model",
      "--destination",
      "/out",
      "--library-path",
      "../lib",
      "--skip-cargo-check",
      "--json",
    ]);
  });
});

describe("SimFlowCli argv helpers", () => {
  it("buildArgs composes global and subcommand args", () => {
    const cli = makeCli(async () => ({ stdout: "", stderr: "" }), {
      foundationRoot: "/sf",
    });
    expect(cli.buildArgs(["sweep", "--file", "x.toml"])).toEqual([
      "--foundation-root",
      "/sf",
      "--project",
      "/proj",
      "sweep",
      "--file",
      "x.toml",
    ]);
  });

  it("buildCommandLine quotes values with spaces", () => {
    const cli = makeCli(async () => ({ stdout: "", stderr: "" }), {
      projectDir: "/has space",
    });
    const line = cli.buildCommandLine(["reset", "DM2a"]);
    expect(line).toContain("'/has space'");
    expect(line.startsWith("/fake/sim-flow")).toBe(true);
    expect(line.endsWith("reset DM2a")).toBe(true);
  });
});
