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
