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

  function makePump(): {
    pump: InstanceType<typeof SessionPump>;
    process: ReturnType<typeof mock.spawnProcess>;
  } {
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
    return { pump, process: mock.state.processes.at(-1)! };
  }

  it("renders an artifact-written event as a one-line note", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "artifact-written", path: "docs/spec.md", bytes: 512 })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Wrote `docs/spec.md` (512 bytes)");
  });

  it("renders a tool-invoked event with duration and optional args summary", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "tool-invoked",
        name: "read_file",
        args_summary: "src/lib.rs",
        status: "ok",
        duration_ms: 7,
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Tool `read_file`");
    expect(rendered.join("")).toContain("src/lib.rs");
    expect(rendered.join("")).toContain("(7 ms)");
  });

  it("renders a phase-changed event as a phase marker", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "phase-changed", phase: "test-impl" })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("**Phase:**");
    expect(rendered.join("")).toContain("test-impl");
  });

  it("renders a gate-result clean event as a green-light line", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "gate-result", step: "DM0", clean: true, failures: [] })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Gate `DM0`: clean");
  });

  it("renders a gate-result failing event with each failure line", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "gate-result",
        step: "DM0",
        clean: false,
        failures: [
          { description: "spec exists", reason: "missing" },
          { description: "spec parses", reason: "yaml invalid" },
        ],
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    const out = rendered.join("");
    expect(out).toContain("2 failure(s)");
    expect(out).toContain("spec exists: missing");
    expect(out).toContain("spec parses: yaml invalid");
  });

  it("renders a state-advanced event with the next step label", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "state-advanced", from: "DM0", to: "DM1" })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Advanced past `DM0`");
    expect(rendered.join("")).toContain("DM1");
  });

  it("renders state-advanced with no `to` as a terminal-step note", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "state-advanced", from: "DM4b", to: null })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Advanced past `DM4b`");
    expect(rendered.join("")).toContain("final step in this flow");
  });

  it("renders a followup event with label + action", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({ event: "followup", label: "Run gate", action: "/gate" })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered.join("")).toContain("Run gate");
    expect(rendered.join("")).toContain("/gate");
  });

  it("assistant-text without a renderer.assistantTurn hook falls back to plain markdown emission", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "assistant-text",
        text: "hello world",
        final_chunk: true,
        tool_calls: [],
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(rendered).toContain("hello world");
  });

  it("assistant-text routes to renderer.assistantTurn when present", async () => {
    const { pump, process } = makePump();
    const turns: Array<{ text: string; finalChunk: boolean }> = [];
    const settlePromise = pump.settle({
      markdown: () => {},
      assistantTurn: (args) => turns.push({ text: args.text, finalChunk: args.finalChunk }),
    });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "assistant-text",
        text: "chunk 1",
        final_chunk: false,
        tool_calls: [],
      })}\n`,
    );
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "assistant-text",
        text: "chunk 2",
        final_chunk: true,
        tool_calls: [],
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(turns).toEqual([
      { text: "chunk 1", finalChunk: false },
      { text: "chunk 2", finalChunk: true },
    ]);
  });

  it("llm-request fires renderer.llmRequest with role + content", async () => {
    const { pump, process } = makePump();
    const reqs: Array<{ role: string; content: string }> = [];
    const settlePromise = pump.settle({
      markdown: () => {},
      llmRequest: (args) => reqs.push({ role: args.role, content: args.content }),
    });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "llm-request",
        role: "system",
        content: "You are a helpful agent.",
        turn_index: 0,
        request_id: "r-1",
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    expect(reqs).toEqual([{ role: "system", content: "You are a helpful agent." }]);
  });

  it("hello-ack renders the session banner and exposes session + descriptor", async () => {
    const { pump, process } = makePump();
    const rendered: string[] = [];
    const settlePromise = pump.settle({ markdown: (t) => rendered.push(t) });
    process.stdout.emit(
      "data",
      `${JSON.stringify({
        event: "hello-ack",
        session: { step: "DM0", kind: "work", candidate: null },
        step_descriptor: { phases: ["chat", "impl"] },
        sim_flow_version: "0.1.0",
        protocol_version: 3,
      })}\n`,
    );
    process.stdout.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settlePromise;
    const out = rendered.join("");
    expect(out).toContain("DM0");
    expect(out).toContain("work session");
    expect(out).toContain("Phases:");
    expect(pump.session).toMatchObject({ step: "DM0", kind: "work" });
    expect(pump.descriptor).toMatchObject({ phases: ["chat", "impl"] });
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
