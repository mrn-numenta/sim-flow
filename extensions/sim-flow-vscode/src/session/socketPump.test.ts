import { describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => {
  class TinyEmitter {
    private readonly listeners = new Map<string, Array<(...args: unknown[]) => void>>();

    on(event: string, listener: (...args: unknown[]) => void): this {
      const existing = this.listeners.get(event) ?? [];
      existing.push(listener);
      this.listeners.set(event, existing);
      return this;
    }

    once(event: string, listener: (...args: unknown[]) => void): this {
      const wrapper = (...args: unknown[]) => {
        this.off(event, wrapper);
        listener(...args);
      };
      return this.on(event, wrapper);
    }

    off(event: string, listener: (...args: unknown[]) => void): this {
      const existing = this.listeners.get(event) ?? [];
      this.listeners.set(
        event,
        existing.filter((entry) => entry !== listener),
      );
      return this;
    }

    emit(event: string, ...args: unknown[]): void {
      for (const listener of this.listeners.get(event) ?? []) {
        listener(...args);
      }
    }
  }

  class FakeSocket extends TinyEmitter {
    destroyed = false;
    readonly writes: string[] = [];

    setEncoding(): void {}

    write(line: string): void {
      this.writes.push(line);
      const trimmed = line.trim();
      if (trimmed.length === 0) {
        return;
      }
      const event = JSON.parse(trimmed) as { event: string };
      if (event.event === "hello") {
        this.emit(
          "data",
          `${JSON.stringify({
            event: "hello-ack",
            protocol_version: "1",
            sim_flow_version: "0.0.0-test",
            session: {
              step: "DM0",
              kind: "work",
              candidate: null,
            },
            step_descriptor: {
              step: "DM0",
              kind: "work",
              flow: "dm",
              prerequisite: null,
              instruction_path: "/tmp/spec.md",
              work_artifacts: [],
              predecessor_inputs: [],
              per_candidate: false,
              phases: ["chat"],
              tools: [],
            },
          })}\n`,
        );
        this.emit(
          "data",
          `${JSON.stringify({
            event: "request-user-input",
            prompt: "continue",
            placeholder: "Reply",
          })}\n`,
        );
      } else if (event.event === "user-message") {
        this.emit(
          "data",
          `${JSON.stringify({
            event: "session-end",
            reason: "completed",
            message: "done",
          })}\n`,
        );
        this.emit("close");
      }
    }

    destroy(): void {
      this.destroyed = true;
    }
  }

  const state = {
    sockets: [] as FakeSocket[],
  };

  function reset(): void {
    state.sockets = [];
  }

  function createConnection(): FakeSocket {
    const socket = new FakeSocket();
    state.sockets.push(socket);
    queueMicrotask(() => {
      socket.emit("connect");
    });
    return socket;
  }

  return {
    state,
    reset,
    createConnection,
  };
});

vi.mock("node:net", () => ({
  createConnection: () => mock.createConnection(),
}));

vi.mock("vscode", () => ({
  workspace: {
    getConfiguration: () => ({
      get: (key: string) => {
        if (key === "llm.source") {
          return "ollama";
        }
        if (key === "llm.model") {
          return "llama3.1";
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

import { type PumpRenderer } from "./pump";
import { SocketSessionPump } from "./socketPump";

class RecordingRenderer implements PumpRenderer {
  readonly markdownChunks: string[] = [];

  markdown(text: string): void {
    this.markdownChunks.push(text);
  }
}

describe("session/socketPump", () => {
  it("replays queued events and resumes over the reconnectable transport", async () => {
    mock.reset();

    const pump = new SocketSessionPump(
      {
        sessionId: "session-1",
        socketPath: "/tmp/session-1.sock",
      },
      {
        source: "ollama",
        model: "llama3.1",
        projectDir: "/tmp/example",
        binary: "sim-flow",
        debugTokens: "",
      },
    );
    await pump.ready();

    const firstRenderer = new RecordingRenderer();
    const first = await pump.settle(firstRenderer);
    expect(first.status).toBe("awaiting-input");
    expect(firstRenderer.markdownChunks.join("")).toContain("Step `DM0` work session");

    pump.sendUserMessage("continue");
    const secondRenderer = new RecordingRenderer();
    const second = await pump.settle(secondRenderer);
    expect(second).toMatchObject({
      status: "ended",
      endReason: "completed",
      endMessage: "done",
    });
    expect(
      mock.state.sockets[0]?.writes.some((line) => line.includes("\"event\":\"user-message\"")),
    ).toBe(true);

    pump.dispose();
  });

  it("dispatches manual-mode commands as line-delimited host events", async () => {
    // The dashboard's per-step buttons route through `runStep` /
    // `runCritique` / etc. when an orchestrator is attached. Each
    // invocation writes one HostEvent JSON object to the transport
    // socket. The orchestrator's response (Diagnostic / GateResult /
    // SessionEnd / StepModeChanged) flows back through the existing
    // settle path; here we just assert the dispatch shape.
    mock.reset();

    const pump = new SocketSessionPump(
      {
        sessionId: "session-2",
        socketPath: "/tmp/session-2.sock",
      },
      {
        source: "ollama",
        model: "llama3.1",
        projectDir: "/tmp/example",
        binary: "sim-flow",
        debugTokens: "",
      },
    );
    await pump.ready();
    // Settle through the initial Hello/HelloAck handshake so we know
    // the socket is wired up before the manual-mode dispatches go out.
    const renderer = new RecordingRenderer();
    await pump.settle(renderer);

    pump.runStep("DM1a", "work");
    pump.runCritique("DM1a");
    pump.runGate("DM1a");
    pump.advance("DM1a");
    pump.reset("DM0");
    pump.setStepMode("auto");
    pump.shutdown();
    // sendHostEventAfterReady awaits the connectionReady promise via
    // a microtask before writing; flush so the test sees the writes.
    await new Promise<void>((resolve) => queueMicrotask(resolve));

    const writes = mock.state.sockets[0]?.writes ?? [];
    const events = writes.map((line) => JSON.parse(line.trim()) as { event: string });
    const eventNames = events.map((e) => e.event);
    // Hello (handshake) precedes the manual-mode dispatches; assert
    // the manual-mode tail rather than the full sequence so adding
    // events to the handshake later doesn't break the test.
    expect(eventNames).toContain("run-step");
    expect(eventNames).toContain("run-critique");
    expect(eventNames).toContain("run-gate");
    expect(eventNames).toContain("advance");
    expect(eventNames).toContain("reset");
    expect(eventNames).toContain("set-step-mode");
    expect(eventNames).toContain("shutdown");
    const setStepMode = events.find((e) => e.event === "set-step-mode");
    expect(setStepMode).toMatchObject({ mode: "auto" });

    pump.dispose();
  });

  async function makePump(sessionId: string): Promise<{
    pump: SocketSessionPump;
    socket: ReturnType<typeof mock.createConnection>;
    renderer: RecordingRenderer;
  }> {
    mock.reset();
    const pump = new SocketSessionPump(
      { sessionId, socketPath: `/tmp/${sessionId}.sock` },
      {
        source: "ollama",
        model: "llama3.1",
        projectDir: "/tmp/example",
        binary: "sim-flow",
        debugTokens: "",
      },
    );
    await pump.ready();
    const renderer = new RecordingRenderer();
    await pump.settle(renderer); // through hello-ack -> request-user-input
    const socket = mock.state.sockets[0]!;
    return { pump, socket, renderer };
  }

  it("notifies subscribers when a sub-session-started / sub-session-ended pair arrives", async () => {
    const { pump, socket } = await makePump("sub-pair");
    const observed: boolean[] = [];
    const dispose = pump.onSubSessionChanged((inSubSession) => {
      observed.push(inSubSession);
    });
    expect(pump.inSubSession).toBe(false);
    socket.emit(
      "data",
      `${JSON.stringify({ event: "sub-session-started", step: "DM1", kind: "work" })}\n`,
    );
    expect(pump.inSubSession).toBe(true);
    socket.emit("data", `${JSON.stringify({ event: "sub-session-ended" })}\n`);
    expect(pump.inSubSession).toBe(false);
    expect(observed).toEqual([true, false]);
    dispose();
    pump.dispose();
  });

  it("forwards gate-result events through onGateResult subscribers", async () => {
    const { pump, socket } = await makePump("gate-sub");
    const observed: Array<{ step: string; clean: boolean; failures: number }> = [];
    const dispose = pump.onGateResult((msg) => {
      observed.push({
        step: msg.step,
        clean: msg.clean,
        failures: msg.failures.length,
      });
    });
    socket.emit(
      "data",
      `${JSON.stringify({
        event: "gate-result",
        step: "DM2c",
        clean: false,
        failures: [{ description: "x", reason: "y" }],
      })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({ event: "gate-result", step: "DM2c", clean: true, failures: [] })}\n`,
    );
    expect(observed).toEqual([
      { step: "DM2c", clean: false, failures: 1 },
      { step: "DM2c", clean: true, failures: 0 },
    ]);
    dispose();
    pump.dispose();
  });

  it("forwards followup events to onFollowup subscribers (in-settle)", async () => {
    const { pump, socket } = await makePump("followup-sub");
    const observed: Array<{ label: string; action: string }> = [];
    const dispose = pump.onFollowup((msg) => {
      observed.push({ label: msg.label, action: msg.action });
    });
    // Resume drive so the renderer is attached when follow-up events
    // arrive (the pump queues events when currentRenderer is null --
    // followup is NOT in the bypass allowlist).
    const renderer = new RecordingRenderer();
    const settle2 = pump.settle(renderer);
    socket.emit(
      "data",
      `${JSON.stringify({ event: "followup", label: "Run gate", action: "/gate" })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({ event: "followup", label: "Continue", action: "continue" })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    expect(observed).toEqual([
      { label: "Run gate", action: "/gate" },
      { label: "Continue", action: "continue" },
    ]);
    dispose();
    pump.dispose();
  });

  it("forwards next-action-hint events to onNextActionHint subscribers (including null, in-settle)", async () => {
    const { pump, socket } = await makePump("nah-sub");
    const observed: Array<{ label: string | null }> = [];
    const dispose = pump.onNextActionHint((msg) => observed.push({ label: msg.label }));
    const renderer = new RecordingRenderer();
    const settle2 = pump.settle(renderer);
    socket.emit(
      "data",
      `${JSON.stringify({ event: "next-action-hint", label: "Run DM2c" })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "next-action-hint", label: null })}\n`);
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    expect(observed).toEqual([{ label: "Run DM2c" }, { label: null }]);
    dispose();
    pump.dispose();
  });

  it("forwards request-user-input events to onRequestUserInput subscribers", async () => {
    const { pump, socket } = await makePump("rui-sub");
    const observed: Array<{ prompt: string | null; placeholder: string | null }> = [];
    const dispose = pump.onRequestUserInput((msg) => {
      observed.push({ prompt: msg.prompt ?? null, placeholder: msg.placeholder ?? null });
    });
    // Start a fresh settle so the next request-user-input -- with the
    // payload we care about -- is processed live (it would NOT be
    // bypassed when currentRenderer is null).
    const renderer = new RecordingRenderer();
    const settle2 = pump.settle(renderer);
    socket.emit(
      "data",
      `${JSON.stringify({
        event: "request-user-input",
        prompt: "Pick one",
        placeholder: "...",
      })}\n`,
    );
    await settle2;
    expect(observed).toEqual([{ prompt: "Pick one", placeholder: "..." }]);
    dispose();
    pump.dispose();
  });

  it("isViewer reports false for the first attached client and true after the first response", async () => {
    // Without orchestrator-side viewer handshake events, isViewer
    // defaults to false. This pins down the public contract.
    const { pump } = await makePump("viewer-pin");
    expect(pump.isViewer).toBe(false);
    pump.dispose();
  });

  it("dispose() is idempotent and tears down the socket", async () => {
    const { pump, socket } = await makePump("dispose-pin");
    pump.dispose();
    expect(socket.destroyed).toBe(true);
    expect(() => pump.dispose()).not.toThrow();
  });

  it("renders an in-settle artifact-written / tool-invoked / phase-changed sequence", async () => {
    const { pump, socket } = await makePump("renderer-events");
    const rendered: string[] = [];
    const settle2 = pump.settle({ markdown: (t) => rendered.push(t) });
    socket.emit(
      "data",
      `${JSON.stringify({ event: "artifact-written", path: "docs/spec.md", bytes: 42 })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({
        event: "tool-invoked",
        name: "write_file",
        args_summary: "docs/spec.md",
        status: "ok",
        duration_ms: 12,
      })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({ event: "phase-changed", phase: "impl" })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    const out = rendered.join("");
    expect(out).toContain("Wrote `docs/spec.md` (42 bytes)");
    expect(out).toContain("Tool `write_file`");
    expect(out).toContain("(12 ms)");
    expect(out).toContain("**Phase:**");
    expect(out).toContain("impl");
    pump.dispose();
  });

  it("renders state-advanced events to the active renderer (queued + flushed)", async () => {
    const { pump, socket } = await makePump("state-advance");
    const rendered: string[] = [];
    const settle2 = pump.settle({ markdown: (t) => rendered.push(t) });
    // state-advanced is NOT in the bypass list, so it queues until
    // flushQueuedEvents runs inside settle and reaches the renderer.
    socket.emit(
      "data",
      `${JSON.stringify({ event: "state-advanced", from: "DM0", to: "DM1" })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    const out = rendered.join("");
    expect(out).toContain("Advanced past `DM0`");
    expect(out).toContain("DM1");
    pump.dispose();
  });

  it("renders diagnostic events with severity tags (info / warning / error)", async () => {
    const { pump, socket } = await makePump("diag");
    const rendered: string[] = [];
    const settle2 = pump.settle({ markdown: (t) => rendered.push(t) });
    socket.emit(
      "data",
      `${JSON.stringify({ event: "diagnostic", level: "info", message: "Just FYI" })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({ event: "diagnostic", level: "warning", message: "Heads up" })}\n`,
    );
    socket.emit(
      "data",
      `${JSON.stringify({ event: "diagnostic", level: "error", message: "Boom" })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    const out = rendered.join("");
    expect(out).toContain("**Info**: Just FYI");
    expect(out).toContain("**Warning**: Heads up");
    expect(out).toContain("**Error**: Boom");
    pump.dispose();
  });

  it("assistant-text routes to assistantTurn when present (incl. tool_calls)", async () => {
    const { pump, socket } = await makePump("assistant-turn");
    const turns: Array<{
      text: string;
      finalChunk: boolean;
      toolCallNames: string[];
    }> = [];
    const settle2 = pump.settle({
      markdown: () => {},
      assistantTurn: (args) =>
        turns.push({
          text: args.text,
          finalChunk: args.finalChunk,
          toolCallNames: args.toolCalls.map((t) => t.name),
        }),
    });
    socket.emit(
      "data",
      `${JSON.stringify({
        event: "assistant-text",
        text: "",
        final_chunk: true,
        tool_calls: [
          { id: "t-1", name: "write_file", arguments_json: '{"path":"x.md"}' },
        ],
      })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    expect(turns).toEqual([
      { text: "", finalChunk: true, toolCallNames: ["write_file"] },
    ]);
    pump.dispose();
  });

  it("llm-request routes to renderer.llmRequest with the full payload", async () => {
    const { pump, socket } = await makePump("llm-req");
    const reqs: Array<{ role: string; content: string; turnIndex: number }> = [];
    const settle2 = pump.settle({
      markdown: () => {},
      llmRequest: (args) =>
        reqs.push({ role: args.role, content: args.content, turnIndex: args.turnIndex }),
    });
    socket.emit(
      "data",
      `${JSON.stringify({
        event: "llm-request",
        role: "user",
        content: "Please do X",
        turn_index: 3,
        request_id: "req-5",
      })}\n`,
    );
    socket.emit("data", `${JSON.stringify({ event: "request-user-input" })}\n`);
    await settle2;
    expect(reqs).toEqual([{ role: "user", content: "Please do X", turnIndex: 3 }]);
    pump.dispose();
  });

  it("notifies subscribers when StepModeChanged arrives from the orchestrator", async () => {
    mock.reset();

    const pump = new SocketSessionPump(
      {
        sessionId: "session-3",
        socketPath: "/tmp/session-3.sock",
      },
      {
        source: "ollama",
        model: "llama3.1",
        projectDir: "/tmp/example",
        binary: "sim-flow",
        debugTokens: "",
      },
    );
    await pump.ready();

    const observed: string[] = [];
    const dispose = pump.onStepModeChanged((mode) => {
      observed.push(mode);
    });

    // Inject a StepModeChanged event over the socket as if the
    // orchestrator had emitted it. The pump tracks the latest mode
    // and notifies subscribers.
    const socket = mock.state.sockets[0];
    socket?.emit(
      "data",
      `${JSON.stringify({ event: "step-mode-changed", mode: "manual" })}\n`,
    );
    socket?.emit(
      "data",
      `${JSON.stringify({ event: "step-mode-changed", mode: "auto" })}\n`,
    );

    expect(observed).toEqual(["manual", "auto"]);
    expect(pump.stepMode).toBe("auto");

    dispose();
    pump.dispose();
  });
});
