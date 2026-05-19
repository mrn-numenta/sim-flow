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

describe("SimFlowCli.planProgress / planProgressAll", () => {
  it("planProgress forwards the current step", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({ milestones: [], current_task: null, plan_path: "docs/impl-plan/plan.md" }),
    );
    const cli = makeCli(execute);
    const out = await cli.planProgress("DM0");
    expect(out.plan_path).toBe("docs/impl-plan/plan.md");
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "plan-progress",
      "--current-step",
      "DM0",
    ]);
  });

  it("planProgressAll passes --all", async () => {
    const { execute, calls } = capturing(JSON.stringify({ DM0: { milestones: [] } }));
    const cli = makeCli(execute);
    await cli.planProgressAll();
    expect(calls[0][1]).toContain("plan-progress");
    expect(calls[0][1]).toContain("--all");
  });
});

describe("SimFlowCli.critiques / critiqueForStep", () => {
  it("critiques returns the JSON-decoded array", async () => {
    const { execute, calls } = capturing(JSON.stringify([{ step: "DM0", clean: true }]));
    const cli = makeCli(execute);
    const out = await cli.critiques();
    expect(out).toHaveLength(1);
    expect(calls[0][1]).toEqual(["--project", "/proj", "critiques"]);
  });

  it("critiqueForStep narrows to one step via --step", async () => {
    const { execute, calls } = capturing(JSON.stringify({ step: "DM2c", clean: false }));
    const cli = makeCli(execute);
    const out = await cli.critiqueForStep("DM2c");
    expect(out).toMatchObject({ step: "DM2c", clean: false });
    expect(calls[0][1]).toEqual(["--project", "/proj", "critiques", "--step", "DM2c"]);
  });
});

describe("SimFlowCli.documents", () => {
  it("forwards the flow as --flow", async () => {
    const { execute, calls } = capturing("[]");
    const cli = makeCli(execute);
    await cli.documents("direct-modeling");
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "documents",
      "--flow",
      "direct-modeling",
    ]);
  });
});

describe("SimFlowCli.advance", () => {
  it("calls advance with an optional step + --json", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        step: "DM0",
        clean: true,
        advanced: true,
        next_step: "DM1",
        failures: [],
      }),
    );
    const cli = makeCli(execute);
    await cli.advance("DM0");
    expect(calls[0][1]).toEqual(["--project", "/proj", "advance", "DM0", "--json"]);
  });

  it("omits the step when called with no argument", async () => {
    const { execute, calls } = capturing(
      JSON.stringify({
        step: "DM0",
        clean: true,
        advanced: false,
        next_step: null,
        failures: [],
      }),
    );
    const cli = makeCli(execute);
    await cli.advance();
    expect(calls[0][1]).toEqual(["--project", "/proj", "advance", "--json"]);
  });

  it("tolerates non-zero exit when stdout still carries a JSON failure payload", async () => {
    const payload = JSON.stringify({
      step: "DM0",
      clean: false,
      advanced: false,
      next_step: null,
      failures: [{ description: "x", reason: "y" }],
    });
    const execute: Execute = async () => {
      const err: Error & { code?: number; stdout?: string } = new Error("exit 1");
      err.code = 1;
      err.stdout = payload;
      throw err;
    };
    const cli = makeCli(execute);
    const out = await cli.advance("DM0");
    expect(out.clean).toBe(false);
  });
});

describe("SimFlowCli describe / prompts / convertSv / blockDiagram", () => {
  it("describe forwards <step>.<kind>", async () => {
    const { execute, calls } = capturing(JSON.stringify({ slug: "dm0-spec", body: "..." }));
    const cli = makeCli(execute);
    await cli.describe("DM0", "work");
    expect(calls[0][1]).toEqual(["--project", "/proj", "describe", "DM0.work", "--json"]);
  });

  it("promptsList hits prompts/list with --json", async () => {
    const { execute, calls } = capturing("[]");
    const cli = makeCli(execute);
    await cli.promptsList();
    expect(calls[0][1]).toEqual(["--project", "/proj", "prompts", "list", "--json"]);
  });

  it("promptShow returns stdout verbatim and forwards <slug>.<kind>", async () => {
    const { execute, calls } = capturing("raw prompt body\n");
    const cli = makeCli(execute);
    const out = await cli.promptShow("dm0-spec", "critique");
    expect(out).toBe("raw prompt body\n");
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "prompts",
      "show",
      "dm0-spec.critique",
    ]);
  });

  it("promptShow wraps subprocess errors into a typed SimFlowCliError", async () => {
    const execute: Execute = async () => {
      throw new Error("ENOENT");
    };
    const cli = makeCli(execute);
    await expect(cli.promptShow("dm0", "work")).rejects.toBeInstanceOf(SimFlowCliError);
  });

  it("convertSv adds --force only when requested", async () => {
    const { execute, calls } = capturing("");
    const cli = makeCli(execute);
    await cli.convertSv(false);
    expect(calls[0][1]).toEqual(["--project", "/proj", "convert-sv"]);
    await cli.convertSv(true);
    expect(calls[1][1]).toEqual(["--project", "/proj", "convert-sv", "--force"]);
  });

  it("convertSv wraps subprocess errors", async () => {
    const execute: Execute = async () => {
      throw new Error("boom");
    };
    const cli = makeCli(execute);
    await expect(cli.convertSv()).rejects.toBeInstanceOf(SimFlowCliError);
  });

  it("promptReset forwards the scope flag and wraps errors", async () => {
    const { execute, calls } = capturing("");
    const cli = makeCli(execute);
    await cli.promptReset("dm0", "work", "global");
    expect(calls[0][1]).toEqual([
      "--project",
      "/proj",
      "prompts",
      "reset",
      "dm0.work",
      "--scope",
      "global",
    ]);
    const failing = makeCli(async () => {
      throw new Error("boom");
    });
    await expect(failing.promptReset("dm0", "work", "all")).rejects.toBeInstanceOf(SimFlowCliError);
  });

  it("blockDiagram fires the block-diagram subcommand", async () => {
    const { execute, calls } = capturing("");
    const cli = makeCli(execute);
    await cli.blockDiagram();
    expect(calls[0][1]).toContain("block-diagram");
  });
});

describe("toCliError fallback (non-object throw)", () => {
  it("execJson wraps a non-object throw value into a SimFlowCliError", async () => {
    // A throw value that is not an Error / object exercises the
    // `toCliError` fallback branch (no .message, no .stderr).
    const execute: Execute = async () => {
      // eslint-disable-next-line @typescript-eslint/no-throw-literal
      throw "not an object";
    };
    const cli = makeCli(execute);
    await expect(cli.status()).rejects.toBeInstanceOf(SimFlowCliError);
    await expect(cli.status()).rejects.toMatchObject({ kind: "spawn-failed" });
  });
});

describe("shellQuote (via buildCommandLine)", () => {
  it("quotes an empty arg with ''", async () => {
    const cli = makeCli(async () => ({ stdout: "", stderr: "" }));
    const line = cli.buildCommandLine(["echo", ""]);
    // The empty arg becomes ''.
    expect(line.split(" ").pop()).toBe("''");
  });

  it("does not quote argv tokens that match the safe charset", async () => {
    const cli = makeCli(async () => ({ stdout: "", stderr: "" }));
    const line = cli.buildCommandLine(["reset", "DM2a"]);
    // No quoting should be applied to DM2a or reset.
    expect(line).toMatch(/\breset\s+DM2a$/);
  });

  it("escapes single quotes inside an argv value", async () => {
    const cli = makeCli(async () => ({ stdout: "", stderr: "" }));
    const line = cli.buildCommandLine(["echo", "it's tricky"]);
    expect(line).toContain("'it'\\''s tricky'");
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
