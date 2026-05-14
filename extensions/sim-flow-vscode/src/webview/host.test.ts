import { beforeEach, describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => {
  type PostedMessage = { type: string; [key: string]: unknown };

  class FakeWebview {
    html = "";
    cspSource = "vscode-test";
    readonly posted: PostedMessage[] = [];
    private receiver: ((message: unknown) => void | Promise<void>) | undefined;

    asWebviewUri<T>(uri: T): T {
      return uri;
    }

    onDidReceiveMessage(
      receiver: (message: unknown) => void | Promise<void>,
    ): { dispose(): void } {
      this.receiver = receiver;
      return { dispose() {} };
    }

    async postMessage(message: PostedMessage): Promise<boolean> {
      this.posted.push(message);
      return true;
    }
  }

  class FakeWebviewPanel {
    readonly webview = new FakeWebview();
    visible = true;

    reveal(): void {}

    onDidDispose(): { dispose(): void } {
      return { dispose() {} };
    }

    onDidChangeViewState(): { dispose(): void } {
      return { dispose() {} };
    }
  }

  return {
    config: new Map<string, unknown>(),
    enumerateCalls: [] as Array<Record<string, unknown>>,
    lastPanel: undefined as FakeWebviewPanel | undefined,
    reset(): void {
      this.config = new Map<string, unknown>([
        ["llm.source", "server:lmstudio-local"],
        ["llm.model", ""],
        ["llm.verbose", true],
        ["llm.debugAdaptation", false],
        ["llm.ollama.baseUrl", "http://localhost:11434/v1"],
        ["llm.lmstudio.baseUrl", "http://localhost:1234/v1"],
        [
          "llm.servers",
          [
            {
              name: "lmstudio-local",
              kind: "lmstudio",
              host: "127.0.0.1",
              port: 1234,
              path: "/v1",
            },
          ],
        ],
      ]);
      this.enumerateCalls = [];
      this.lastPanel = undefined;
    },
    FakeWebviewPanel,
  };
});

vi.mock("vscode", () => ({
  Uri: {
    joinPath: (base: { fsPath: string }, ...parts: string[]) => ({
      fsPath: [base.fsPath, ...parts].join("/"),
    }),
  },
  ViewColumn: {
    Active: 1,
  },
  ConfigurationTarget: {
    Workspace: 1,
  },
  workspace: {
    getConfiguration: () => ({
      get: (key: string, defaultValue?: unknown) =>
        mock.config.has(key) ? mock.config.get(key) : defaultValue,
      update: async (key: string, value: unknown) => {
        mock.config.set(key, value);
      },
    }),
    onDidChangeConfiguration: () => ({ dispose() {} }),
  },
  window: {
    createWebviewPanel: () => {
      const panel = new mock.FakeWebviewPanel();
      mock.lastPanel = panel;
      return panel;
    },
  },
}));


vi.mock("../state/experiments", () => ({
  openExperiments: () => null,
}));

vi.mock("../state/watcher", () => ({
  createStateWatcher: () => ({
    onDidChange: () => ({ dispose() {} }),
    dispose() {},
  }),
}));

vi.mock("../state/documents", () => ({
  enumerateProjectDocuments: async () => [],
}));

vi.mock("../llm/enumerate", () => ({
  enumerateModels: async (opts: Record<string, unknown>) => {
    mock.enumerateCalls.push(opts);
    return { models: ["qwen/qwen3-14b"] };
  },
}));

vi.mock("../session/control-client", () => ({
  ControlSocketError: class extends Error {},
  controlSocketLikelyPresent: () => false,
  sendCommand: async () => undefined,
}));

vi.mock("./aggregate", () => ({
  aggregateDashboardState: () => ({
    projectDir: "/tmp/project",
    flow: {
      flow: "direct-modeling",
      current_step: "DM0",
      started: null,
      gates: {},
      archived_gates: {},
    },
    critiques: [],
    runs: [],
    baselines: [],
    documents: [],
    planProgress: { milestoneOrder: [], milestones: {} },
    llmServers: mock.config.get("llm.servers") ?? [],
    coverage: { thresholdPct: 90, level: "total" },
    stepMode: "manual",
    sessionActive: false,
    inSubSession: false,
    isViewer: false,
    generatedAt: "2026-05-09T00:00:00.000Z",
  }),
}));

describe("DashboardHost LLM source round-tripping", () => {
  beforeEach(() => {
    vi.resetModules();
    mock.reset();
  });

  it("preserves the raw custom source in llm-config and model-list messages", async () => {
    const { DashboardHost } = await import("./host");

    const host = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: "/tmp/project",
      cli: {} as never,
      workspaceState: { get: () => undefined, update: async () => undefined } as never,
    });

    await host.open();
    await (host as never as { postLlmConfig(): Promise<void> }).postLlmConfig();
    await (host as never as { sendModelList(source: string): Promise<void> }).sendModelList(
      "server:lmstudio-local",
    );

    expect(mock.lastPanel?.webview.posted).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: "llm-config",
          source: "server:lmstudio-local",
        }),
        expect.objectContaining({
          type: "model-list",
          source: "server:lmstudio-local",
          models: ["qwen/qwen3-14b"],
        }),
      ]),
    );
    expect(mock.enumerateCalls).toEqual([
      expect.objectContaining({
        source: "lmstudio",
        baseUrl: "http://127.0.0.1:1234/v1",
      }),
    ]);
  });
});
