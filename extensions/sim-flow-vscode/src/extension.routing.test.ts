import { beforeEach, describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => {
  const state = {
    config: new Map<string, unknown>(),
    commandHandlers: new Map<string, (...args: unknown[]) => unknown>(),
    chatOpenArgs: [] as unknown[],
    terminalRuns: [] as string[],
    autoSessionLaunches: [] as Array<{ specPath: string | undefined; projectDirHint: string | undefined }>,
    stepSessionLaunches: [] as Array<{
      step: string;
      kind: "work" | "critique";
      projectDirHint: string | undefined;
    }>,
  };

  class FakeChatPanelProvider {
    async launchAutoSession(
      specPath: string | undefined,
      projectDirHint: string | undefined,
    ): Promise<void> {
      state.autoSessionLaunches.push({ specPath, projectDirHint });
    }

    async launchStepSession(
      step: string,
      kind: "work" | "critique",
      projectDirHint: string | undefined,
    ): Promise<void> {
      state.stepSessionLaunches.push({ step, kind, projectDirHint });
    }

    dispose(): void {}
  }

  class FakeAutoSessionManager {
    dispose(): void {}
  }

  class FakeTerminal {
    constructor(_options: unknown) {}

    run(command: string): void {
      state.terminalRuns.push(command);
    }

    dispose(): void {}
  }

  class FakeSimFlowCli {
    constructor(private readonly options: { binary: string }) {}

    buildCommandLine(subcommand: string[]): string {
      return `${this.options.binary} ${subcommand.join(" ")}`;
    }
  }

  function reset(): void {
    state.config = new Map<string, unknown>([
      ["llm.source", "vscode"],
      ["llm.model", ""],
      ["foundationRoot", ""],
      ["binaryPath", ""],
      ["session.mode", "per-step"],
    ]);
    state.commandHandlers = new Map<string, (...args: unknown[]) => unknown>();
    state.chatOpenArgs = [];
    state.terminalRuns = [];
    state.autoSessionLaunches = [];
    state.stepSessionLaunches = [];
  }

  reset();

  return {
    state,
    reset,
    FakeChatPanelProvider,
    FakeAutoSessionManager,
    FakeTerminal,
    FakeSimFlowCli,
  };
});

vi.mock("vscode", () => ({
  workspace: {
    workspaceFolders: [],
    getConfiguration: () => ({
      get: (key: string, defaultValue?: unknown) =>
        mock.state.config.has(key) ? mock.state.config.get(key) : defaultValue,
      update: async (key: string, value: unknown) => {
        mock.state.config.set(key, value);
      },
    }),
  },
  window: {
    registerWebviewViewProvider: () => ({ dispose() {} }),
    showErrorMessage: async () => undefined,
    showWarningMessage: async () => undefined,
    showInformationMessage: async () => undefined,
  },
  commands: {
    registerCommand: (command: string, handler: (...args: unknown[]) => unknown) => {
      mock.state.commandHandlers.set(command, handler);
      return { dispose() {} };
    },
    executeCommand: async (command: string, ...args: unknown[]) => {
      if (command === "workbench.action.chat.open") {
        mock.state.chatOpenArgs.push(args[0]);
        return;
      }
      const handler = mock.state.commandHandlers.get(command);
      if (handler) {
        return await handler(...args);
      }
      return undefined;
    },
  },
  ThemeIcon: class {},
}));

vi.mock("./apiKey", () => ({
  clearApiKey: async () => undefined,
  setApiKey: async () => undefined,
}));

vi.mock("./participant", () => ({
  registerChatParticipant: () => undefined,
}));

vi.mock("./chatPanel/host", () => ({
  CHAT_PANEL_CONTAINER_ID: "sim-flow-chat-panel",
  CHAT_PANEL_VIEW_ID: "simFlow.chatPanel",
  ChatPanelProvider: mock.FakeChatPanelProvider,
}));

vi.mock("./chatPanel/autoSessionManager", () => ({
  AutoSessionManager: mock.FakeAutoSessionManager,
}));

vi.mock("./webview/host", () => ({
  DashboardHost: class {
    dispose(): void {}
  },
}));

vi.mock("./terminal", () => ({
  SimFlowTerminal: mock.FakeTerminal,
}));

vi.mock("./cli", () => ({
  bundledCandidates: [],
  bundledFrameworkDocsRoot: () => undefined,
  resolveBinary: () => "/mock/bin/sim-flow",
  setBundledRoot: () => undefined,
  SimFlowCli: mock.FakeSimFlowCli,
  SimFlowCliError: class extends Error {},
}));

vi.mock("./context", () => ({
  findProjectCandidates: async () => [],
  pickProject: async () => undefined,
  resolveProjectDir: () => undefined,
}));

async function activateExtension(): Promise<void> {
  const { activate } = await import("./extension");
  activate({
    subscriptions: [],
    extensionUri: { fsPath: "/mock/extension" },
    workspaceState: { get: () => undefined, update: async () => undefined },
    secrets: { get: async () => undefined, store: async () => undefined, delete: async () => undefined },
  } as never);
}

async function execute(command: string, ...args: unknown[]): Promise<unknown> {
  const handler = mock.state.commandHandlers.get(command);
  if (!handler) {
    throw new Error(`Command not registered: ${command}`);
  }
  return await handler(...args);
}

describe("extension routing", () => {
  beforeEach(async () => {
    vi.resetModules();
    mock.reset();
    await activateExtension();
  });

  it("routes vscode play and step commands to the built-in chat surface", async () => {
    const projectDir = "/tmp/example";
    const specPath = "/tmp/example/docs/spec.md";
    mock.state.config.set("llm.source", "vscode");
    mock.state.config.set("llm.model", "copilot/gpt-4.1");

    await execute("sim-flow.runFlow", specPath, projectDir);
    await execute("sim-flow.runStep", "DM0", projectDir);
    await execute("sim-flow.runCritique", "DM0", projectDir);

    expect(mock.state.autoSessionLaunches).toEqual([]);
    expect(mock.state.stepSessionLaunches).toEqual([]);
    expect(mock.state.terminalRuns).toEqual([]);
    expect(mock.state.chatOpenArgs).toEqual([
      { query: `@sim-flow /auto --spec ${specPath} --project ${projectDir}` },
      { query: `@sim-flow /step DM0.work --project ${projectDir}` },
      { query: `@sim-flow /step DM0.critique --project ${projectDir}` },
    ]);
  });

  it.each(["lmstudio", "ollama", "anthropic", "openai"] as const)(
    "routes %s play and step commands to the sim-flow chat panel",
    async (source) => {
      const projectDir = "/tmp/example";
      const specPath = "/tmp/example/docs/spec.md";
      mock.state.config.set("llm.source", source);
      mock.state.config.set("llm.model", `${source}-model`);

      await execute("sim-flow.runFlow", specPath, projectDir);
      await execute("sim-flow.runStep", "DM2a", projectDir);
      await execute("sim-flow.runCritique", "DM2a", projectDir);

      expect(mock.state.chatOpenArgs).toEqual([]);
      expect(mock.state.terminalRuns).toEqual([]);
      expect(mock.state.autoSessionLaunches).toEqual([
        { specPath, projectDirHint: projectDir },
      ]);
      expect(mock.state.stepSessionLaunches).toEqual([
        { step: "DM2a", kind: "work", projectDirHint: projectDir },
        { step: "DM2a", kind: "critique", projectDirHint: projectDir },
      ]);
    },
  );

  it.each([
    ["claude-cli", "claude"],
    ["codex-cli", "codex"],
    ["gh-copilot-cli", "gh-copilot"],
  ] as const)(
    "routes %s play and step commands to a terminal session",
    async (source, backend) => {
      const projectDir = "/tmp/example";
      const specPath = "/tmp/example/docs/spec.md";
      mock.state.config.set("llm.source", source);
      mock.state.config.set("llm.model", `${backend}-model`);

      await execute("sim-flow.runFlowTerminal", backend, specPath, projectDir);
      await execute("sim-flow.runStep", "DM3a", projectDir);
      await execute("sim-flow.runCritique", "DM3a", projectDir);

      expect(mock.state.chatOpenArgs).toEqual([]);
      expect(mock.state.autoSessionLaunches).toEqual([]);
      expect(mock.state.stepSessionLaunches).toEqual([]);
      expect(mock.state.terminalRuns).toEqual([
        `/mock/bin/sim-flow auto --llm-backend ${backend} --session-mode per-step --llm-model ${backend}-model --spec ${specPath}`,
        `/mock/bin/sim-flow session DM3a.work --llm-backend ${backend} --llm-model ${backend}-model`,
        `/mock/bin/sim-flow session DM3a.critique --llm-backend ${backend} --llm-model ${backend}-model`,
      ]);
    },
  );
});
