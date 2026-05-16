import { beforeEach, describe, expect, it } from "vitest";

import {
  AutoSessionManager,
  type AutoSessionDriveDelegate,
} from "./autoSessionManager";

class FakeMemento {
  private readonly values = new Map<string, unknown>();

  get<T>(key: string, defaultValue?: T): T | undefined {
    if (this.values.has(key)) {
      return this.values.get(key) as T;
    }
    return defaultValue;
  }

  async update(key: string, value: unknown): Promise<void> {
    if (value === undefined) {
      this.values.delete(key);
    } else {
      this.values.set(key, value);
    }
  }
}

type SettleResult = {
  status: "awaiting-input" | "ended";
  endReason?: string;
  endMessage?: string;
};

class FakePump {
  readonly sentMessages: string[] = [];
  readonly session = null;
  readonly descriptor = null;
  // Defaults to never-resolving so tests that only need the launch
  // phase don't block on drive completion. Tests that need drive to
  // complete can swap this for a resolved promise.
  nextSettle: Promise<SettleResult> = new Promise(() => undefined);

  settle(): Promise<SettleResult> {
    return this.nextSettle;
  }

  sendUserMessage(text: string): void {
    this.sentMessages.push(text);
  }

  cancel(): void {}

  dispose(): void {}
}

const noopDelegate: AutoSessionDriveDelegate = {
  markdown() {},
  requestTokensEstimate() {},
  async settled() {},
};

describe("chatPanel/autoSessionManager", () => {
  let workspaceState: FakeMemento;
  let manager: AutoSessionManager;

  beforeEach(() => {
    workspaceState = new FakeMemento();
    manager = new AutoSessionManager(workspaceState as never);
  });

  async function launchOne(projectDir = "/tmp/example"): Promise<{
    session: Awaited<ReturnType<AutoSessionManager["launch"]>>;
    pump: FakePump;
  }> {
    const pump = new FakePump();
    const session = await manager.launch(
      {
        sessionId: "session-1",
        socketPath: `${projectDir}/.sim-flow/auto.sock`,
        projectDir,
        pump: pump as never,
        sourceTag: "ollama",
        model: "llama3.1",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: "docs/spec.md",
      },
      noopDelegate,
    );
    return { session, pump };
  }

  it("persists and clears the active auto-session record", async () => {
    const { session } = await launchOne();
    expect(manager.readStoredRecord("/tmp/example")).toMatchObject({
      sessionId: "session-1",
      socketPath: "/tmp/example/.sim-flow/auto.sock",
      projectDir: "/tmp/example",
      awaitingInput: false,
      sourceTag: "ollama",
      model: "llama3.1",
      sessionMode: "auto",
      stepRef: null,
      launchSpecPath: "docs/spec.md",
    });

    await manager.markAwaitingInput(session);
    expect(manager.readStoredRecord("/tmp/example")).toMatchObject({
      awaitingInput: true,
    });

    await manager.clearIfActive(session);
    expect(manager.readStoredRecord("/tmp/example")).toBeUndefined();
  });

  it("emits active-session change notifications around launch/clear", async () => {
    const events: Array<unknown> = [];
    manager.onActiveSessionChanged((s) => events.push(s));
    expect(events.length).toBe(0);
    const { session } = await launchOne();
    // Launch fires once with the new session.
    expect(events.length).toBe(1);
    expect(events[events.length - 1]).toBeTruthy();
    await manager.clearIfActive(session);
    // Clear fires once with undefined.
    expect(events[events.length - 1]).toBeUndefined();
  });

  it("resumeWithPrompt forwards the user message through the pump", async () => {
    // launchOne with a settle that resolves to "awaiting-input" so the
    // initial drive completes and resumeWithPrompt's `await drivePromise`
    // doesn't hang.
    const pump = new FakePump();
    pump.nextSettle = Promise.resolve({ status: "awaiting-input" });
    const session = await manager.launch(
      {
        sessionId: "session-1",
        socketPath: "/tmp/example/.sim-flow/auto.sock",
        projectDir: "/tmp/example",
        pump: pump as never,
        sourceTag: "ollama",
        model: "llama3.1",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: "docs/spec.md",
      },
      noopDelegate,
    );
    // Let the initial drive finish.
    await new Promise((r) => setTimeout(r, 5));
    await manager.markAwaitingInput(session);
    // For resumeWithPrompt's own drive cycle, also keep it never-pending
    // so this test doesn't block on the second drive (we only care that
    // sendUserMessage was called).
    pump.nextSettle = new Promise(() => undefined);
    await manager.resumeWithPrompt(session, "continue please", noopDelegate);
    expect(pump.sentMessages).toEqual(["continue please"]);
    expect(manager.readStoredRecord(session.projectDir)).toMatchObject({
      awaitingInput: false,
    });
  });

  it("resumeWithPrompt is a no-op when the session is not active anymore", async () => {
    const pump = new FakePump();
    pump.nextSettle = Promise.resolve({ status: "awaiting-input" });
    const session = await manager.launch(
      {
        sessionId: "session-1",
        socketPath: "/tmp/example/.sim-flow/auto.sock",
        projectDir: "/tmp/example",
        pump: pump as never,
        sourceTag: "ollama",
        model: "llama3.1",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: "docs/spec.md",
      },
      noopDelegate,
    );
    await new Promise((r) => setTimeout(r, 5));
    await manager.clearIfActive(session);
    await manager.resumeWithPrompt(session, "ignored", noopDelegate);
    expect(pump.sentMessages).toEqual([]);
  });

  it("cancel calls pump.cancel and clears awaitingInput", async () => {
    const { session, pump } = await launchOne();
    let cancelCalled = false;
    pump.cancel = () => {
      cancelCalled = true;
    };
    await manager.markAwaitingInput(session);
    await manager.cancel(session);
    expect(cancelCalled).toBe(true);
    expect(session.awaitingInput).toBe(false);
  });

  it("appendFollowup adds entries but dedupes on identical label+action pairs", async () => {
    const { session } = await launchOne();
    manager.appendFollowup(session, { label: "Continue", action: "continue" });
    manager.appendFollowup(session, { label: "Continue", action: "continue" });
    manager.appendFollowup(session, { label: "Cancel", action: "cancel" });
    expect(session.pendingFollowups.map((f) => f.label)).toEqual(["Continue", "Cancel"]);
  });

  it("appendFollowup / setNextActionHint / setPendingPrompt are no-ops on a non-active session", async () => {
    const { session } = await launchOne();
    await manager.clearIfActive(session);
    manager.appendFollowup(session, { label: "x", action: "y" });
    manager.setNextActionHint(session, "Continue");
    manager.setPendingPrompt(session, "ask", "placeholder");
    expect(session.pendingFollowups).toEqual([]);
    expect(session.nextActionHint).toBeUndefined();
    expect(session.currentPrompt).toBeNull();
    expect(session.currentPlaceholder).toBeNull();
  });

  it("setNextActionHint + setPendingPrompt update the active session state", async () => {
    const { session } = await launchOne();
    manager.setNextActionHint(session, "Run DM0");
    manager.setPendingPrompt(session, "Press to continue", "DM0");
    expect(session.nextActionHint).toBe("Run DM0");
    expect(session.currentPrompt).toBe("Press to continue");
    expect(session.currentPlaceholder).toBe("DM0");
  });

  it("forgetStoredRecord drops the persisted record and clears the active session when it matches", async () => {
    const { session } = await launchOne();
    await manager.forgetStoredRecord(session.projectDir);
    expect(manager.readStoredRecord(session.projectDir)).toBeUndefined();
  });

  it("forgetStoredRecord only drops the record when projectDir does NOT match the active session", async () => {
    const { session } = await launchOne();
    await manager.forgetStoredRecord("/tmp/other-project");
    // Active session untouched; its record still present.
    expect(manager.readStoredRecord(session.projectDir)).toBeDefined();
  });

  it("readStoredRecord defaults sessionMode and stepRef when an older record lacks them", async () => {
    // Persist a record directly via workspaceState that omits the
    // newer fields; readStoredRecord should hydrate sane defaults.
    await workspaceState.update("sim-flow.chatPanel.activeAutoSession./tmp/legacy", {
      sessionId: "old",
      socketPath: "/tmp/legacy/auto.sock",
      projectDir: "/tmp/legacy",
      awaitingInput: false,
      sourceTag: "ollama",
      model: "llama3",
      launchSpecPath: undefined,
      updatedAtMs: 1,
    });
    const r = manager.readStoredRecord("/tmp/legacy");
    expect(r).toMatchObject({ sessionMode: "auto", stepRef: null });
  });

  it("driveSession routes pump output through the delegate's markdown / llmRequest / assistantTurn / settled hooks", async () => {
    const pump = new FakePump();
    // Resolve settle synchronously to "awaiting-input" so the
    // drive cycle completes; the delegate's settled callback is the
    // termination of the drive.
    type Renderer = {
      markdown(text: string): void;
      requestTokensEstimate?(t: number): void;
      llmRequest?(args: {
        role: string;
        content: string;
        turnIndex: number;
        requestId: string;
      }): void;
      assistantTurn?(args: {
        text: string;
        finalChunk: boolean;
        toolCalls: Array<{ id?: string; name: string; argumentsJson: string }>;
      }): void;
    };
    let capturedRenderer: Renderer | undefined;
    pump.settle = ((renderer: Renderer): Promise<SettleResult> => {
      capturedRenderer = renderer;
      return Promise.resolve({ status: "awaiting-input" });
    }) as unknown as typeof pump.settle;

    const calls: string[] = [];
    const delegate: AutoSessionDriveDelegate = {
      markdown(_s, text) {
        calls.push(`md:${text}`);
      },
      requestTokensEstimate(_s, t) {
        calls.push(`tok:${t}`);
      },
      llmRequest(_s, args) {
        calls.push(`llm:${args.role}:${args.content}`);
      },
      assistantTurn(_s, args) {
        calls.push(`turn:${args.text}:${args.finalChunk}`);
      },
      async settled(_s, result) {
        calls.push(`settled:${result.status}`);
      },
    };
    const session = await manager.launch(
      {
        sessionId: "drive-1",
        socketPath: "/tmp/drive-1.sock",
        projectDir: "/tmp/drive-proj",
        pump: pump as never,
        sourceTag: "ollama",
        model: "x",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: undefined,
      },
      delegate,
    );
    // Wait for drive promise.
    await session.drivePromise;
    // The renderer we captured during settle must have all four hooks
    // wired to the delegate's session-scoped variants.
    expect(capturedRenderer).toBeTruthy();
    capturedRenderer!.markdown("hello");
    capturedRenderer!.requestTokensEstimate!(42);
    capturedRenderer!.llmRequest!({
      role: "user",
      content: "do x",
      turnIndex: 1,
      requestId: "r-1",
    });
    capturedRenderer!.assistantTurn!({
      text: "ok",
      finalChunk: true,
      toolCalls: [],
    });
    expect(calls).toContain("md:hello");
    expect(calls).toContain("tok:42");
    expect(calls).toContain("llm:user:do x");
    expect(calls).toContain("turn:ok:true");
    expect(calls).toContain("settled:awaiting-input");
  });

  it("driveSession returns early when the session is no longer active after settle", async () => {
    const pump = new FakePump();
    // Resolve settle on the next tick so we can clear the session
    // between settle's resolution and the `if (!isActive) return` guard.
    let resolveSettle: (r: SettleResult) => void = () => {};
    pump.nextSettle = new Promise<SettleResult>((resolve) => {
      resolveSettle = resolve;
    });
    let settledCalled = false;
    const delegate: AutoSessionDriveDelegate = {
      markdown() {},
      requestTokensEstimate() {},
      async settled() {
        settledCalled = true;
      },
    };
    const session = await manager.launch(
      {
        sessionId: "drive-stale",
        socketPath: "/tmp/drive-stale.sock",
        projectDir: "/tmp/drive-stale",
        pump: pump as never,
        sourceTag: "ollama",
        model: "x",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: undefined,
      },
      delegate,
    );
    // Race: clear the session BEFORE settle resolves.
    await manager.clearIfActive(session);
    resolveSettle({ status: "awaiting-input" });
    await session.drivePromise;
    expect(settledCalled).toBe(false);
  });

  it("attach hydrates a session from a stored record and persists it", async () => {
    const pump = new FakePump();
    pump.nextSettle = Promise.resolve({ status: "awaiting-input" });
    const stored = {
      sessionId: "old-session",
      socketPath: "/tmp/old.sock",
      projectDir: "/tmp/attach-proj",
      awaitingInput: true,
      sourceTag: "ollama",
      model: "x",
      sessionMode: "auto" as const,
      stepRef: null,
      launchSpecPath: undefined,
      updatedAtMs: 1,
    };
    const session = await manager.attach(stored, pump as never, noopDelegate);
    expect(session.sessionId).toBe("old-session");
    expect(session.awaitingInput).toBe(true);
    expect(manager.readStoredRecord("/tmp/attach-proj")).toMatchObject({
      sessionId: "old-session",
      awaitingInput: true,
    });
  });

  it("waitForDrive resolves once the in-flight drivePromise settles", async () => {
    const pump = new FakePump();
    let resolveSettle: (r: SettleResult) => void = () => {};
    pump.nextSettle = new Promise((r) => {
      resolveSettle = r;
    });
    const session = await manager.launch(
      {
        sessionId: "wait-1",
        socketPath: "/tmp/wait.sock",
        projectDir: "/tmp/wait",
        pump: pump as never,
        sourceTag: "ollama",
        model: "x",
        sessionMode: "auto",
        stepRef: null,
        launchSpecPath: undefined,
      },
      noopDelegate,
    );
    let resolved = false;
    const waiter = manager.waitForDrive(session).then(() => {
      resolved = true;
    });
    expect(resolved).toBe(false);
    resolveSettle({ status: "awaiting-input" });
    await waiter;
    expect(resolved).toBe(true);
  });

  it("waitForDrive is a no-op when there is no in-flight drive", async () => {
    // No launch -> no drivePromise -> awaitForDrive resolves immediately.
    const pump = new FakePump();
    // Fake a session shape with drivePromise=null.
    const session = {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      pump: pump as any,
      drivePromise: null,
      projectDir: "/tmp/no-drive",
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await manager.waitForDrive(session as any);
  });

  it("dispose() tears down the active session and clears listeners", async () => {
    const { pump } = await launchOne();
    let disposed = false;
    pump.dispose = () => {
      disposed = true;
    };
    const events: unknown[] = [];
    manager.onActiveSessionChanged((s) => events.push(s));
    manager.dispose();
    expect(disposed).toBe(true);
    // After dispose, listeners get one final undefined emission as the
    // active session flips to undefined.
    expect(events.length).toBeGreaterThan(0);
    expect(events[events.length - 1]).toBeUndefined();
  });
});
