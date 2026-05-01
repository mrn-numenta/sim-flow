import { beforeEach, describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => {
  class TinyEmitter {
    private readonly listeners = new Map<string, Array<(...args: unknown[]) => void>>();

    on(event: string, listener: (...args: unknown[]) => void): void {
      const existing = this.listeners.get(event) ?? [];
      existing.push(listener);
      this.listeners.set(event, existing);
    }

    emit(event: string, ...args: unknown[]): void {
      for (const listener of this.listeners.get(event) ?? []) {
        listener(...args);
      }
    }
  }

  class FakeReadable extends TinyEmitter {
    setEncoding(): void {}
  }

  class FakeProcess extends TinyEmitter {
    readonly stdout = new FakeReadable();
    readonly stderr = new FakeReadable();
    readonly stdin = {
      writes: [] as string[],
      write: (line: string) => {
        this.stdin.writes.push(line);
        return true;
      },
    };

    kill(): void {}
  }

  const state = {
    processes: [] as FakeProcess[],
  };

  function reset(): void {
    state.processes = [];
  }

  function spawnProcess(): FakeProcess {
    const process = new FakeProcess();
    state.processes.push(process);
    return process;
  }

  return {
    state,
    reset,
    spawnProcess,
  };
});

vi.mock("node:child_process", () => ({
  spawn: () => mock.spawnProcess(),
}));

vi.mock("vscode", () => ({
  workspace: {
    getConfiguration: () => ({
      get: (key: string) => {
        if (key === "llm.source") {
          return "vscode";
        }
        if (key === "llm.model") {
          return "";
        }
        if (key === "llm.verbose") {
          return true;
        }
        return undefined;
      },
    }),
  },
  CancellationTokenSource: class {
    readonly token = { isCancellationRequested: false };
  },
}));

vi.mock("../cli", () => ({
  bundledFrameworkDocsRoot: () => undefined,
  bundledPdfiumLibPath: () => undefined,
}));

vi.mock("../llm", () => {
  class MockLlmError extends Error {
    constructor(
      readonly kind: string,
      message: string,
      readonly detail?: string,
    ) {
      super(message);
    }
  }

  return {
    createBackend: () => ({
      stream: async function* () {
        return;
      },
    }),
    LlmError: MockLlmError,
  };
});

vi.mock("./debug-log", () => ({
  DebugLog: {
    fromTokens: () => ({
      logProcessSpawn() {},
      logSpawnError() {},
      logProcessExit() {},
      logRawIn() {},
      logEventIn() {},
      logRawOut() {},
      logEventOut() {},
      logLlmDispatch() {},
      logLlmChunk() {},
      logLlmEnd() {},
      logLlmError() {},
      dispose() {},
    }),
  },
}));

const { SessionPump } = await import("./pump");

describe("session/pump", () => {
  beforeEach(() => {
    mock.reset();
  });

  it("renders a diagnostic and keeps settling after malformed protocol JSON", async () => {
    const pump = new SessionPump(
      {
        binary: "/mock/bin/sim-flow",
        args: ["session", "DM0.work", "--jsonl"],
        cwd: "/tmp/example",
      },
      {
        source: "vscode",
        projectDir: "/tmp/example",
        binary: "/mock/bin/sim-flow",
        debugTokens: "",
      },
    );
    const process = mock.state.processes.at(-1)!;
    const rendered: string[] = [];

    const settlePromise = pump.settle({
      markdown: (text: string) => {
        rendered.push(text);
      },
    });

    process.stdout.emit("data", "{ definitely not valid json }\n");
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);

    const result = await settlePromise;
    expect(result).toEqual({ status: "awaiting-input" });
    expect(rendered.join("")).toContain("protocol: bad JSON from sim-flow");
  });

  it("handles chunked protocol lines and surfaces orchestrator-driven session end", async () => {
    const pump = new SessionPump(
      {
        binary: "/mock/bin/sim-flow",
        args: ["session", "DM0.work", "--jsonl"],
        cwd: "/tmp/example",
      },
      {
        source: "vscode",
        projectDir: "/tmp/example",
        binary: "/mock/bin/sim-flow",
        debugTokens: "",
      },
    );
    const process = mock.state.processes.at(-1)!;
    const rendered: string[] = [];

    const settlePromise = pump.settle({
      markdown: (text: string) => {
        rendered.push(text);
      },
    });

    const diagnosticLine = JSON.stringify({
      event: "diagnostic",
      level: "warning",
      message: "Heads up",
    });
    const sessionEndLine = JSON.stringify({
      event: "session-end",
      reason: "cancelled",
      message: "Mock orchestrator cancelled the session.",
    });
    const splitAt = diagnosticLine.indexOf("Heads");
    process.stdout.emit("data", diagnosticLine.slice(0, splitAt));
    process.stdout.emit("data", `${diagnosticLine.slice(splitAt)}\n${sessionEndLine}\n`);

    const result = await settlePromise;
    expect(result).toEqual({
      status: "ended",
      endReason: "cancelled",
      endMessage: "Mock orchestrator cancelled the session.",
    });
    expect(rendered.join("")).toContain("**Warning**: Heads up");
  });
});
