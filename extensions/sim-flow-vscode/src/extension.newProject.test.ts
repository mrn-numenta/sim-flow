import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mock = vi.hoisted(() => {
  const state = {
    workspaceFolders: [] as Array<{ uri: { fsPath: string }; name: string; index: number }>,
    inputResponses: [] as string[],
    inputPrompts: [] as Array<{ title?: string; prompt?: string }>,
    infoMessages: [] as string[],
    errorMessages: [] as string[],
    cliCalls: [] as Array<{ name: string; destination: string }>,
    openedProjects: [] as string[],
    commandHandlers: new Map<string, (...args: unknown[]) => unknown>(),
  };

  function reset(): void {
    state.workspaceFolders = [];
    state.inputResponses = [];
    state.inputPrompts = [];
    state.infoMessages = [];
    state.errorMessages = [];
    state.cliCalls = [];
    state.openedProjects = [];
    state.commandHandlers = new Map<string, (...args: unknown[]) => unknown>();
  }

  reset();

  return {
    state,
    reset,
  };
});

vi.mock("vscode", () => ({
  workspace: {
    get workspaceFolders() {
      return mock.state.workspaceFolders;
    },
    getConfiguration: () => ({
      get: (_key: string, defaultValue?: unknown) => defaultValue,
      update: async () => undefined,
    }),
  },
  window: {
    registerWebviewViewProvider: () => ({ dispose() {} }),
    showErrorMessage: async (msg: string) => {
      mock.state.errorMessages.push(msg);
      return undefined;
    },
    showInformationMessage: async (msg: string) => {
      mock.state.infoMessages.push(msg);
      return undefined;
    },
    showWarningMessage: async () => undefined,
    showInputBox: async (opts?: { title?: string; prompt?: string }) => {
      mock.state.inputPrompts.push({ title: opts?.title, prompt: opts?.prompt });
      return mock.state.inputResponses.shift();
    },
    withProgress: async (
      _opts: unknown,
      task: (progress: { report: (_value: unknown) => void }) => Promise<unknown>,
    ) => await task({ report: () => undefined }),
  },
  commands: {
    registerCommand: (command: string, handler: (...args: unknown[]) => unknown) => {
      mock.state.commandHandlers.set(command, handler);
      return { dispose() {} };
    },
    executeCommand: async (command: string, ...args: unknown[]) => {
      const handler = mock.state.commandHandlers.get(command);
      if (handler) {
        return await handler(...args);
      }
      return undefined;
    },
  },
  ThemeIcon: class {},
  ProgressLocation: {
    Notification: 15,
  },
  ConfigurationTarget: {
    Workspace: 2,
  },
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
  ChatPanelProvider: class {
    dispose(): void {}
  },
}));

vi.mock("./chatPanel/autoSessionManager", () => ({
  AutoSessionManager: class {
    dispose(): void {}
  },
}));

vi.mock("./webview/host", () => ({
  DashboardHost: class {
    constructor(opts: { projectDir: string }) {
      mock.state.openedProjects.push(opts.projectDir);
    }
    async open(): Promise<void> {}
    dispose(): void {}
  },
}));

vi.mock("./perfPanel/host", () => ({
  PerfPanelHost: class {
    async open(): Promise<void> {}
    dispose(): void {}
  },
}));

vi.mock("./terminal", () => ({
  SimFlowTerminal: class {
    dispose(): void {}
  },
}));

vi.mock("./session/processRegistry", () => ({
  cleanupStalePidsAsync: async () => undefined,
}));

vi.mock("./extension/attachWatcher", () => ({
  attachWatcherCommand: async () => undefined,
}));

vi.mock("./extension/lmStudio", () => ({
  dumpAvailableLmModels: async () => undefined,
  testLmModel: async () => undefined,
}));

vi.mock("./extension/stepRunner", () => ({
  runFlowChatCommand: async () => undefined,
  runFlowInTerminal: async () => undefined,
  runFullyAutomatedInTerminal: async () => undefined,
  runStepCommand: async () => undefined,
}));

vi.mock("./context", () => ({
  PICK_PROJECT_NEW: "::sim-flow-pick::new",
  findProjectCandidates: async () => [],
  pickProject: async () => undefined,
  resolveProjectDir: () => undefined,
}));

vi.mock("./cli", () => ({
  bundledCandidates: [],
  bundledFrameworkDocsRoot: () => undefined,
  resolveBinary: () => "/mock/bin/sim-flow",
  setBundledRoot: () => undefined,
  SimFlowCli: class {
    constructor(_opts: { binary: string; projectDir: string; foundationRoot?: string }) {}
    async newModel(opts: { name: string; destination: string }): Promise<{ project_dir: string }> {
      mock.state.cliCalls.push(opts);
      return { project_dir: path.join(opts.destination, opts.name) };
    }
  },
  SimFlowCliError: class extends Error {},
}));

async function activateExtension(): Promise<void> {
  const { activate } = await import("./extension");
  activate({
    subscriptions: [],
    extensionUri: { fsPath: "/mock/extension" },
    workspaceState: { get: () => undefined, update: async () => undefined },
    secrets: {
      get: async () => undefined,
      store: async () => undefined,
      delete: async () => undefined,
    },
  } as never);
}

async function execute(command: string, ...args: unknown[]): Promise<unknown> {
  const handler = mock.state.commandHandlers.get(command);
  if (!handler) {
    throw new Error(`Command not registered: ${command}`);
  }
  return await handler(...args);
}

let tmpRoot: string;
let originalUser: string | undefined;

beforeEach(async () => {
  vi.resetModules();
  mock.reset();
  tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-new-project-"));
  originalUser = process.env.USER;
  process.env.USER = "tester";

  const simModelsRoot = path.join(tmpRoot, "sim-models");
  fs.mkdirSync(path.join(simModelsRoot, "docs", "modeling-guide"), { recursive: true });
  fs.mkdirSync(path.join(simModelsRoot, "examples"), { recursive: true });
  mock.state.workspaceFolders = [{ uri: { fsPath: simModelsRoot }, name: "sim-models", index: 0 }];

  await activateExtension();
});

afterEach(() => {
  if (originalUser === undefined) {
    delete process.env.USER;
  } else {
    process.env.USER = originalUser;
  }
  fs.rmSync(tmpRoot, { recursive: true, force: true });
});

describe("sim-flow.newProject", () => {
  it("creates projects under sim-models/users/<user> using only a name prompt", async () => {
    const simModelsRoot = mock.state.workspaceFolders[0]!.uri.fsPath;
    mock.state.inputResponses = ["demo-model"];

    await execute("sim-flow.newProject");

    expect(mock.state.inputPrompts).toHaveLength(1);
    expect(mock.state.cliCalls).toEqual([
      {
        name: "demo-model",
        destination: path.join(simModelsRoot, "users", "tester"),
      },
    ]);
    expect(mock.state.openedProjects).toEqual([
      path.join(simModelsRoot, "users", "tester", "demo-model"),
    ]);
  });
});
