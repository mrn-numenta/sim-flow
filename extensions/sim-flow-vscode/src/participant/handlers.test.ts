import { describe, expect, test, vi } from "vitest";

// All handlers are pure orchestration over a CLI stub + a stream
// stub. The only real subprocess machinery is in `runQuiet` (used by
// handleReset + handleInit), which is mocked at the node:child_process
// level so we never spawn an actual process.

vi.mock("vscode", () => ({}));

const execFileCalls: Array<{ bin: string; args: string[] }> = [];
let execFileError: Error | null = null;

vi.mock("node:child_process", () => ({
  execFile: (
    bin: string,
    args: string[],
    _opts: unknown,
    cb: (err: Error | null, value: { stdout: string; stderr: string }) => void,
  ) => {
    execFileCalls.push({ bin, args });
    if (execFileError) {
      cb(execFileError, { stdout: "", stderr: "" });
    } else {
      cb(null, { stdout: "", stderr: "" });
    }
  },
}));

const { handleStatus, handleRuns, handleGate, handleAdvance, handleReset, handleInit } =
  await import("./handlers");
const { SimFlowCliError } = await import("../cli/errors");

interface FakeStream {
  chunks: string[];
  markdown(s: string): void;
}

function fakeStream(): FakeStream {
  const s: FakeStream = {
    chunks: [],
    markdown(c: string) {
      this.chunks.push(c);
    },
  };
  return s;
}

interface CliStub {
  binary?: string;
  status?: () => Promise<unknown>;
  runs?: (filter: unknown) => Promise<unknown[]>;
  gate?: (step: string, candidate?: string) => Promise<unknown>;
  advance?: (step?: string) => Promise<unknown>;
  buildArgs?: (args: string[]) => string[];
}

function fakeContext(cli: CliStub): { projectDir: string; cli: CliStub } {
  return { projectDir: "/tmp/proj", cli };
}

function runArgs(cli: CliStub, prompt = "") {
  return {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    context: fakeContext(cli) as any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    request: {} as any,
    prompt,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    stream: fakeStream() as any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    token: {} as any,
  };
}

describe("handleStatus", () => {
  test("renders the CLI status payload as markdown", async () => {
    const args = runArgs({
      status: async () => ({
        flow: "direct-modeling",
        current_step: "DM0",
        next_step: null,
        per_candidate: false,
        candidates: [],
        gates: [],
      }),
    });
    await handleStatus(args);
    const chunks = (args.stream as unknown as FakeStream).chunks.join("");
    // Output should mention something concrete from the status payload.
    expect(chunks.length).toBeGreaterThan(0);
    expect(chunks).toContain("DM0");
  });
});

describe("handleRuns", () => {
  test("passes the parsed filter to cli.runs and renders the table", async () => {
    let captured: unknown = null;
    const args = runArgs(
      {
        runs: async (filter) => {
          captured = filter;
          return [];
        },
      },
      "--step DM0",
    );
    await handleRuns(args);
    expect(captured).toBeTruthy();
    const chunks = (args.stream as unknown as FakeStream).chunks.join("");
    expect(chunks.length).toBeGreaterThan(0);
  });
});

describe("handleGate", () => {
  test("forwards the parsed step (and optional candidate) to cli.gate", async () => {
    const calls: Array<{ step: string; candidate?: string }> = [];
    const args = runArgs(
      {
        gate: async (step, candidate) => {
          calls.push({ step, candidate });
          return { step, candidate, clean: true, failures: [] };
        },
      },
      "DM0",
    );
    await handleGate(args);
    expect(calls).toEqual([{ step: "DM0", candidate: undefined }]);
  });
});

describe("handleAdvance", () => {
  test("reports the new current step when advance succeeds and moves forward", async () => {
    const args = runArgs(
      {
        advance: async () => ({
          step: "DM0",
          clean: true,
          advanced: true,
          next_step: "DM1",
          failures: [],
        }),
      },
      "DM0",
    );
    await handleAdvance(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("DM0");
    expect(out).toContain("advanced");
    expect(out).toContain("DM1");
  });

  test("reports the terminal-step case when clean but no next step", async () => {
    const args = runArgs(
      {
        advance: async () => ({
          step: "DM9",
          clean: true,
          advanced: false,
          next_step: null,
          failures: [],
        }),
      },
      "DM9",
    );
    await handleAdvance(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("passed");
    expect(out).not.toContain("advanced");
  });

  test("lists every failure when the gate is not clean", async () => {
    const args = runArgs(
      {
        advance: async () => ({
          step: "DM0",
          clean: false,
          advanced: false,
          next_step: null,
          failures: [
            { description: "spec exists", reason: "docs/spec.md missing" },
            { description: "spec parses", reason: "yaml frontmatter invalid" },
          ],
        }),
      },
      "DM0",
    );
    await handleAdvance(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("2 failure(s)");
    expect(out).toContain("docs/spec.md missing");
    expect(out).toContain("yaml frontmatter invalid");
  });

  test("renders a CLI error using kind + stderr when cli.advance throws SimFlowCliError", async () => {
    const args = runArgs(
      {
        advance: async () => {
          throw new SimFlowCliError("boom", {
            kind: "spawn-failed",
            stderr: "permission denied",
            stdout: "",
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
          } as any);
        },
      },
      "DM0",
    );
    await handleAdvance(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("spawn-failed");
    expect(out).toContain("permission denied");
  });

  test("falls back to String(err) for non-SimFlowCliError throws", async () => {
    const args = runArgs(
      {
        advance: async () => {
          throw new Error("generic crash");
        },
      },
      "DM0",
    );
    await handleAdvance(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("generic crash");
  });

  test("treats an empty prompt as no-step (cli.advance called with undefined)", async () => {
    let captured: string | undefined = "untouched";
    const args = runArgs(
      {
        advance: async (step) => {
          captured = step;
          return {
            step: "DM0",
            clean: true,
            advanced: true,
            next_step: "DM1",
            failures: [],
          };
        },
      },
      "   ",
    );
    await handleAdvance(args);
    expect(captured).toBeUndefined();
  });
});

describe("handleReset", () => {
  test("error-args case never spawns the CLI", async () => {
    execFileCalls.length = 0;
    const args = runArgs(
      {
        buildArgs: (a) => a,
      },
      "", // parseResetArgs requires a step
    );
    await handleReset(args);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out.toLowerCase()).toContain("error");
    expect(execFileCalls).toEqual([]);
  });

  test("happy path spawns the CLI with reset+step args and reports completion", async () => {
    execFileCalls.length = 0;
    execFileError = null;
    const args = runArgs(
      {
        binary: "/fake/sim-flow",
        buildArgs: (a) => ["--project", "/tmp/proj", ...a],
      },
      "DM2 --force",
    );
    await handleReset(args);
    expect(execFileCalls).toHaveLength(1);
    expect(execFileCalls[0].args).toContain("reset");
    expect(execFileCalls[0].args).toContain("DM2");
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("Reset complete");
    expect(out).toContain("/step DM2.work");
  });

  test("execFile failure surfaces an error and does not print the completion banner", async () => {
    execFileCalls.length = 0;
    execFileError = Object.assign(new Error("subprocess died"), {});
    try {
      const args = runArgs(
        {
          binary: "/fake/sim-flow",
          buildArgs: (a) => a,
        },
        "DM3 --force",
      );
      await handleReset(args);
      const out = (args.stream as unknown as FakeStream).chunks.join("");
      expect(out).toContain("subprocess died");
      expect(out).not.toContain("Reset complete");
    } finally {
      execFileError = null;
    }
  });
});

describe("handleInit", () => {
  test("spawns init --flow direct-modeling and reports success", async () => {
    execFileCalls.length = 0;
    execFileError = null;
    const args = runArgs({
      binary: "/fake/sim-flow",
      buildArgs: (a) => a,
    });
    await handleInit(args);
    expect(execFileCalls).toHaveLength(1);
    expect(execFileCalls[0].args).toEqual(["init", "--flow", "direct-modeling"]);
    const out = (args.stream as unknown as FakeStream).chunks.join("");
    expect(out).toContain("Initialized");
  });

  test("surfaces execFile failure as an error message and skips the success banner", async () => {
    execFileCalls.length = 0;
    execFileError = new Error("already initialized");
    try {
      const args = runArgs({
        binary: "/fake/sim-flow",
        buildArgs: (a) => a,
      });
      await handleInit(args);
      const out = (args.stream as unknown as FakeStream).chunks.join("");
      expect(out).toContain("already initialized");
      expect(out).not.toContain("Initialized. Run");
    } finally {
      execFileError = null;
    }
  });
});
