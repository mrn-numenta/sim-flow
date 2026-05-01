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

class FakePump {
  readonly sentMessages: string[] = [];

  settle(): Promise<{ status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string }> {
    return new Promise(() => undefined);
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

  it("persists and clears the active auto-session record", async () => {
    const session = await manager.launch(
      {
        projectDir: "/tmp/example",
        pump: new FakePump() as never,
        sourceTag: "ollama",
        model: "llama3.1",
        launchSpecPath: "docs/spec.md",
      },
      noopDelegate,
    );

    expect(manager.readStoredRecord("/tmp/example")).toMatchObject({
      projectDir: "/tmp/example",
      awaitingInput: false,
      sourceTag: "ollama",
      model: "llama3.1",
      launchSpecPath: "docs/spec.md",
    });

    await manager.markAwaitingInput(session);
    expect(manager.readStoredRecord("/tmp/example")).toMatchObject({
      awaitingInput: true,
    });

    await manager.clearIfActive(session);
    expect(manager.readStoredRecord("/tmp/example")).toBeUndefined();
  });
});
