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
});
