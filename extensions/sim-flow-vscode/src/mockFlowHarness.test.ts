import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const FIXTURE_ROOT = fileURLToPath(
  new URL("../testdata/mock-flow", import.meta.url),
);

const mock = vi.hoisted(() => {
  type PostedMessage = { type: string; [key: string]: unknown };
  type DirectReplyChunk =
    | string
    | {
        text: string;
        waitForSignal?: string;
      }
    | {
        throwMessage: string;
        throwKind?: string;
        throwDetail?: string;
        waitForSignal?: string;
      };
  type PumpTurn = {
    onSettle?: (renderer: {
      markdown(text: string): void;
      requestTokensEstimate?(tokens: number): void;
    }) => void | Promise<void>;
    waitForCancel?: boolean;
    cancelResult?: { status: "ended"; endReason?: string; endMessage?: string };
    result: { status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string };
  };
  type PumpSharedSession = {
    projectDir: string;
    turns: PumpTurn[];
    index: number;
    cancelled: boolean;
    cancelWaiters: Array<() => void>;
    args: string[];
    llmSource: string | undefined;
    llmModel: string | undefined;
    sentMessages: string[];
    history: Array<
      | { kind: "markdown"; text: string }
      | { kind: "requestTokensEstimate"; tokens: number }
    >;
    pendingResult:
      | { status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string }
      | null;
  };

  function createDeferred(): {
    promise: Promise<void>;
    resolve: () => void;
  } {
    let resolve = () => {};
    const promise = new Promise<void>((innerResolve) => {
      resolve = innerResolve;
    });
    return { promise, resolve };
  }

  const disposable = () => ({ dispose() {} });

  class FakeCancellationTokenSource {
    readonly token = { isCancellationRequested: false };

    cancel(): void {
      this.token.isCancellationRequested = true;
    }

    dispose(): void {}
  }

  class FakeMemento {
    private readonly values = new Map<string, unknown>();
    private nextUpdateSignal: string | undefined;

    get<T>(key: string, defaultValue?: T): T | undefined {
      if (this.values.has(key)) {
        return this.values.get(key) as T;
      }
      return defaultValue;
    }

    blockNextUpdate(signal: string): void {
      this.nextUpdateSignal = signal;
    }

    async update(key: string, value: unknown): Promise<void> {
      const signal = this.nextUpdateSignal;
      this.nextUpdateSignal = undefined;
      if (signal) {
        await waitForSignal(signal);
      }
      if (value === undefined) {
        this.values.delete(key);
      } else {
        this.values.set(key, value);
      }
    }
  }

  class FakeWebview {
    html = "";
    options: unknown;
    readonly posted: PostedMessage[] = [];
    private receiver: ((message: unknown) => void | Promise<void>) | undefined;

    asWebviewUri<T>(uri: T): T {
      return uri;
    }

    onDidReceiveMessage(
      receiver: (message: unknown) => void | Promise<void>,
    ): { dispose(): void } {
      this.receiver = receiver;
      return disposable();
    }

    async postMessage(message: PostedMessage): Promise<boolean> {
      this.posted.push(message);
      return true;
    }

    async emit(message: unknown): Promise<void> {
      await this.receiver?.(message);
    }
  }

  class FakeWebviewView {
    readonly webview = new FakeWebview();
    visible = true;
    private visibilityListener: (() => void) | undefined;

    onDidChangeVisibility(listener: () => void): { dispose(): void } {
      this.visibilityListener = listener;
      return disposable();
    }

    fireVisibility(): void {
      this.visibilityListener?.();
    }
  }

  class FakeWebviewPanel {
    readonly webview = new FakeWebview();
    visible = true;
    private disposeListener: (() => void) | undefined;
    private viewStateListener:
      | ((event: { webviewPanel: FakeWebviewPanel }) => void)
      | undefined;

    reveal(): void {
      this.visible = true;
    }

    onDidDispose(listener: () => void): { dispose(): void } {
      this.disposeListener = listener;
      return disposable();
    }

    onDidChangeViewState(
      listener: (event: { webviewPanel: FakeWebviewPanel }) => void,
    ): { dispose(): void } {
      this.viewStateListener = listener;
      return disposable();
    }

    dispose(): void {
      this.disposeListener?.();
    }

    fireViewState(): void {
      this.viewStateListener?.({ webviewPanel: this });
    }
  }

  class FakeSessionPump {
    readonly projectDir: string;
    readonly sentMessages: string[];
    readonly args: string[];
    readonly llmSource: string | undefined;
    readonly llmModel: string | undefined;
    private readonly shared: PumpSharedSession;
    private replayed = false;

    constructor(
      options: {
        cwd?: string;
        args?: string[];
        launch?: { cwd: string; args?: string[] };
        socketPath?: string;
      },
      llmConfig?: { source?: string; model?: string },
    ) {
      this.projectDir = options.launch?.cwd ?? options.cwd ?? "";
      const args = options.launch?.args ?? options.args ?? [];
      const socketPath = options.socketPath;
      const launchKey = socketPath ?? this.projectDir;
      const shouldCountAsLaunch = !!options.launch || !socketPath;
      if (shouldCountAsLaunch || !state.reconnectablePumps.has(launchKey)) {
        const shared: PumpSharedSession = {
          projectDir: this.projectDir,
          turns: state.pumpScripts.get(this.projectDir) ?? [],
          index: 0,
          cancelled: false,
          cancelWaiters: [],
          args: [...args],
          llmSource: llmConfig?.source,
          llmModel: llmConfig?.model,
          sentMessages: [],
          history: [],
          pendingResult: null,
        };
        state.reconnectablePumps.set(launchKey, shared);
        if (shouldCountAsLaunch) {
          state.pumpLaunches.push({
            projectDir: this.projectDir,
            args: [...args],
            llmSource: llmConfig?.source,
            llmModel: llmConfig?.model,
          });
        }
      }
      this.shared = state.reconnectablePumps.get(launchKey)!;
      this.sentMessages = this.shared.sentMessages;
      this.args = this.shared.args;
      this.llmSource = this.shared.llmSource;
      this.llmModel = this.shared.llmModel;
      state.pumpInstances.set(this.projectDir, this);
    }

    async ready(): Promise<void> {
      return;
    }

    async settle(renderer: {
      markdown(text: string): void;
      requestTokensEstimate?(tokens: number): void;
    }): Promise<{ status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string }> {
      if (!this.replayed) {
        for (const item of this.shared.history) {
          if (item.kind === "markdown") {
            renderer.markdown(item.text);
          } else {
            renderer.requestTokensEstimate?.(item.tokens);
          }
        }
        this.replayed = true;
      }
      if (this.shared.pendingResult) {
        return this.shared.pendingResult;
      }
      const turn = this.shared.turns[this.shared.index++] ?? { result: { status: "ended" as const } };
      const recordingRenderer = {
        markdown: (text: string) => {
          this.shared.history.push({ kind: "markdown", text });
          renderer.markdown(text);
        },
        requestTokensEstimate: (tokens: number) => {
          this.shared.history.push({ kind: "requestTokensEstimate", tokens });
          renderer.requestTokensEstimate?.(tokens);
        },
      };
      await turn.onSettle?.(recordingRenderer);
      if (turn.waitForCancel && !this.shared.cancelled) {
        await new Promise<void>((resolve) => {
          this.shared.cancelWaiters.push(resolve);
        });
      }
      if (turn.waitForCancel && this.shared.cancelled) {
        const result =
          turn.cancelResult ?? {
            status: "ended" as const,
            endReason: "cancelled",
            endMessage: "Mock session cancelled.",
          };
        this.shared.pendingResult = result;
        return result;
      }
      this.shared.pendingResult = turn.result;
      return turn.result;
    }

    sendUserMessage(text: string): void {
      this.shared.sentMessages.push(text);
      this.shared.pendingResult = null;
    }

    cancel(): void {
      this.shared.cancelled = true;
      for (const resolve of this.shared.cancelWaiters.splice(0)) {
        resolve();
      }
    }

    dispose(): void {
      this.cancel();
    }
  }

  const state = {
    currentProjectDir: undefined as string | undefined,
    workspaceFolders: [] as Array<{ uri: { fsPath: string }; name: string; index: number }>,
    config: new Map<string, unknown>(),
    projectStates: new Map<string, unknown>(),
    directReplies: new Map<string, DirectReplyChunk[]>(),
    pumpScripts: new Map<string, PumpTurn[]>(),
    reconnectablePumps: new Map<string, PumpSharedSession>(),
    pumpInstances: new Map<string, FakeSessionPump>(),
    executedCommands: [] as Array<{ command: string; args: unknown[] }>,
    lastDashboardPanel: undefined as FakeWebviewPanel | undefined,
    chatProvider: undefined as { launchAutoSession(specPath: string | undefined, projectDirHint: string | undefined): Promise<void> } | undefined,
    signals: new Map<string, ReturnType<typeof createDeferred>>(),
    configurationListeners: [] as Array<(event: { affectsConfiguration(section: string): boolean }) => void>,
    activeEditorListeners: [] as Array<() => void>,
    workspaceFolderListeners: [] as Array<() => void>,
    directReplyRequests: [] as Array<{ projectDir: string | null; source?: string; model?: string }>,
    pumpLaunches: [] as Array<{ projectDir: string; args: string[]; llmSource?: string; llmModel?: string }>,
    panelMessageSnapshots: [] as Array<{ projectDir: string | null; contents: string[] }>,
  };

  function reset(): void {
    state.currentProjectDir = undefined;
    state.workspaceFolders = [];
    state.config = new Map<string, unknown>([
      ["llm.source", "vscode"],
      ["llm.model", ""],
      ["llm.verbose", true],
      ["llm.ollama.baseUrl", "http://localhost:11434/v1"],
      ["llm.lmstudio.baseUrl", "http://localhost:1234/v1"],
      ["auto.maxWorkIterations", 6],
      ["auto.maxCritiqueIterations", 10],
      ["auto.maxCritiqueNoProgressIterations", 3],
      ["dashboard.showFullyAutomated", false],
      ["dashboard.verilogSimEnabled", false],
      ["dashboard.verilogSimulatorPath", ""],
    ]);
    state.projectStates = new Map<string, unknown>();
    state.directReplies = new Map<string, string[]>();
    state.pumpScripts = new Map<string, PumpTurn[]>();
    state.reconnectablePumps = new Map<string, PumpSharedSession>();
    state.pumpInstances = new Map<string, FakeSessionPump>();
    state.executedCommands = [];
    state.lastDashboardPanel = undefined;
    state.chatProvider = undefined;
    state.signals = new Map<string, ReturnType<typeof createDeferred>>();
    state.configurationListeners = [];
    state.activeEditorListeners = [];
    state.workspaceFolderListeners = [];
    state.directReplyRequests = [];
    state.pumpLaunches = [];
    state.panelMessageSnapshots = [];
  }

  function waitForSignal(name: string): Promise<void> {
    let signal = state.signals.get(name);
    if (!signal) {
      signal = createDeferred();
      state.signals.set(name, signal);
    }
    return signal.promise;
  }

  function resolveSignal(name: string): void {
    let signal = state.signals.get(name);
    if (!signal) {
      signal = createDeferred();
      state.signals.set(name, signal);
    }
    signal.resolve();
  }

  async function fireConfigurationChange(...keys: string[]): Promise<void> {
    const event = {
      affectsConfiguration(section: string): boolean {
        return keys.some((key) => section === `sim-flow.${key}`);
      },
    };
    for (const listener of state.configurationListeners) {
      await listener(event);
    }
  }

  async function fireActiveEditorChange(): Promise<void> {
    for (const listener of state.activeEditorListeners) {
      await listener();
    }
  }

  async function fireWorkspaceFolderChange(): Promise<void> {
    for (const listener of state.workspaceFolderListeners) {
      await listener();
    }
  }

  return {
    disposable,
    FakeCancellationTokenSource,
    FakeMemento,
    FakeWebviewView,
    FakeWebviewPanel,
    FakeSessionPump,
    state,
    reset,
    resolveSignal,
    waitForSignal,
    fireConfigurationChange,
    fireActiveEditorChange,
    fireWorkspaceFolderChange,
  };
});

vi.mock("vscode", () => ({
  Uri: {
    file: (fsPath: string) => ({ fsPath }),
    joinPath: (base: { fsPath: string }, ...parts: string[]) => ({
      fsPath: path.join(base.fsPath, ...parts),
    }),
  },
  workspace: {
    get workspaceFolders() {
      return mock.state.workspaceFolders;
    },
    getConfiguration: () => ({
      get: (key: string, defaultValue?: unknown) =>
        mock.state.config.has(key) ? mock.state.config.get(key) : defaultValue,
      update: async (key: string, value: unknown) => {
        mock.state.config.set(key, value);
      },
    }),
    findFiles: async () => [],
    onDidChangeConfiguration: (listener: (event: { affectsConfiguration(section: string): boolean }) => void) => {
      mock.state.configurationListeners.push(listener);
      return mock.disposable();
    },
    onDidChangeWorkspaceFolders: (listener: () => void) => {
      mock.state.workspaceFolderListeners.push(listener);
      return mock.disposable();
    },
  },
  window: {
    get activeTextEditor() {
      return undefined;
    },
    onDidChangeActiveTextEditor: (listener: () => void) => {
      mock.state.activeEditorListeners.push(listener);
      return mock.disposable();
    },
    createWebviewPanel: () => {
      const panel = new mock.FakeWebviewPanel();
      mock.state.lastDashboardPanel = panel;
      return panel;
    },
    showErrorMessage: async () => undefined,
    showWarningMessage: async () => undefined,
    showInformationMessage: async () => undefined,
    showQuickPick: async () => undefined,
  },
  commands: {
    executeCommand: async (command: string, ...args: unknown[]) => {
      mock.state.executedCommands.push({ command, args });
      if (command === "sim-flow.runFlow" && mock.state.chatProvider) {
        return await mock.state.chatProvider.launchAutoSession(
          args[0] as string | undefined,
          args[1] as string | undefined,
        );
      }
      return undefined;
    },
  },
  CancellationTokenSource: mock.FakeCancellationTokenSource,
  ConfigurationTarget: {
    Workspace: 1,
  },
  ViewColumn: {
    Active: 1,
  },
}));

vi.mock("./context", () => ({
  resolveProjectDir: () => mock.state.currentProjectDir,
  findProjectCandidates: async () =>
    mock.state.currentProjectDir ? [mock.state.currentProjectDir] : [],
  resolveContext: async (options: { projectDir?: string } = {}) => {
    const projectDir = options.projectDir ?? mock.state.currentProjectDir;
    if (!projectDir) {
      return null;
    }
    return {
      projectDir,
      cli: {
        binary: "/mock/bin/sim-flow",
        foundationRoot: "/mock/foundation",
      },
    };
  },
}));

vi.mock("./state/flowState", () => ({
  readFlowState: async (projectDir: string) => {
    const value = mock.state.projectStates.get(projectDir);
    if (!value) {
      throw new Error(`No mocked flow state for ${projectDir}`);
    }
    return structuredClone(value);
  },
}));

vi.mock("./state/watcher", () => ({
  createStateWatcher: () => ({
    onDidChange: () => mock.disposable(),
    dispose: () => {},
  }),
}));

vi.mock("./session/pump", () => ({
  BREVITY_DIRECTIVE: "Be concise.",
  SessionPump: mock.FakeSessionPump,
}));

vi.mock("./session/socketPump", () => ({
  SocketSessionPump: mock.FakeSessionPump,
}));

const { ChatPanelProvider } = await import("./chatPanel/host");
const { DashboardHost } = await import("./webview/host");

const DMF_ORDER = [
  "DM0",
  "DM1",
  "DM2a",
  "DM2b",
  "DM2c",
  "DM2d",
  "DM3a",
  "DM3b",
  "DM3c",
  "DM4a",
  "DM4b",
] as const;

function makeFlowState(currentStep: string): {
  flow: "direct-modeling";
  current_step: string;
  started: null;
  gates: Record<string, { passed: boolean; timestamp: string | null; candidates: Record<string, never> }>;
  archived_gates: Record<string, never>;
} {
  return {
    flow: "direct-modeling",
    current_step: currentStep,
    started: null,
    gates: {},
    archived_gates: {},
  };
}

function markPassed(projectDir: string, step: string, nextStep?: string): void {
  const current = structuredClone(mock.state.projectStates.get(projectDir)) as ReturnType<
    typeof makeFlowState
  >;
  current.gates[step] = {
    passed: true,
    timestamp: `${step}-passed`,
    candidates: {},
  };
  if (nextStep) {
    current.current_step = nextStep;
  }
  mock.state.projectStates.set(projectDir, current);
}

function fixtureStateFile(fixtureName: string): string {
  return path.join(FIXTURE_ROOT, fixtureName, ".sim-flow", "state.toml");
}

function fixtureCurrentStep(fixtureName: string): string {
  const text = fs.readFileSync(fixtureStateFile(fixtureName), "utf8");
  const match = text.match(/^current_step\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error(`Fixture ${fixtureName} is missing current_step in state.toml`);
  }
  return match[1];
}

function createProjectFromFixture(root: string, fixtureName: string): string {
  const fixtureDir = path.join(FIXTURE_ROOT, fixtureName);
  const projectDir = path.join(root, fixtureName);
  fs.cpSync(fixtureDir, projectDir, { recursive: true });
  mock.state.projectStates.set(projectDir, makeFlowState(fixtureCurrentStep(fixtureName)));
  return projectDir;
}

function latestState(view: InstanceType<typeof mock.FakeWebviewView>) {
  const stateMessages = view.webview.posted.filter((message) => message.type === "state-update");
  const last = stateMessages.at(-1) as { type: "state-update"; state: unknown } | undefined;
  return last?.state as
    | {
        projectLabel: string;
        currentStep: string | null;
        currentPhase: string | null;
        currentTool: string | null;
        currentArtifact: string | null;
        notice: string;
        canStop: boolean;
        supportsPromptEntry: boolean;
        sourceLabel: string;
        totalInputTokensEstimate: number;
        totalOutputTokensEstimate: number;
        transcript: Array<{ kind: string; title?: string; body?: string }>;
      }
    | undefined;
}

function transcriptBodies(state: { transcript: Array<{ kind: string; body?: string }> }): string {
  return state.transcript
    .filter((entry) => entry.kind === "assistant" || entry.kind === "user" || entry.kind === "note")
    .map((entry) => entry.body ?? "")
    .join("\n");
}

function countOccurrences(text: string, needle: string): number {
  return text.split(needle).length - 1;
}

async function flushAsyncWork(rounds = 4): Promise<void> {
  for (let i = 0; i < rounds; i += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

describe("mocked dashboard/chat harness", () => {
  let tmpRoot: string;

  beforeEach(() => {
    mock.reset();
    tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-harness-"));
  });

  afterEach(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  it("drives a mocked dashboard -> chat auto flow for the example project through DM4b", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("_Phases:_ `discover` -> `implement` -> `verify`\n\n");
          renderer.markdown("**Phase:** `discover`\n");
          renderer.requestTokensEstimate?.(264);
          renderer.markdown("_Tool `read_file` (docs/spec.md) -> ok (12 ms)._\n");
          renderer.markdown("I'll");
          renderer.markdown(" begin by reading the existing test-plan.md and the critique to understand what needs to be fixed.");
          renderer.markdown("\n\nNow");
          renderer.markdown(" I'll read the required reference files to understand what tests need to be specified.");
          renderer.markdown("\n\nNow");
          renderer.markdown(" I'll read the targets and decomposition files to understand what tests need to be created.");
          renderer.markdown("\n\nNow");
          renderer.markdown(" I'll read the decomposition and pipeline mapping files to complete my understanding.");
          for (let index = 0; index < DMF_ORDER.length - 1; index += 1) {
            const from = DMF_ORDER[index];
            const to = DMF_ORDER[index + 1];
            markPassed(exampleDir, from, to);
            renderer.markdown(`\n**Advanced past \`${from}\`; current step is now \`${to}\`.**\n`);
          }
          markPassed(exampleDir, DMF_ORDER[DMF_ORDER.length - 1]);
          renderer.markdown("\nThe grayscale conversion pipeline is fully specified through DM4b.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Mock flow completed through DM4b.",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const chatProvider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = chatProvider as never;
    const chatView = new mock.FakeWebviewView();
    await chatProvider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    expect(mock.state.lastDashboardPanel).toBeDefined();
    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(state).toBeDefined();
    expect(state!.projectLabel).toBe("example");
    expect(state!.currentStep).toBe("DM4b");
    expect(transcriptBodies(state!)).toContain(
      "I'll begin by reading the existing test-plan.md and the critique to understand what needs to be fixed.",
    );
    expect(transcriptBodies(state!)).toContain(
      "Now I'll read the required reference files to understand what tests need to be specified.",
    );
    expect(state!.transcript.some((entry) => entry.title === "Tool activity")).toBe(false);
    expect(mock.state.executedCommands).toEqual(
      expect.arrayContaining([
        {
          command: "sim-flow.runFlow",
          args: [specPath, exampleDir],
        },
      ]),
    );
  });

  it("shows a dashboard error when fully automated run is requested without a spec path", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: new mock.FakeMemento() as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto-end-to-end",
      specPath: "   ",
    });
    await flushAsyncWork();

    expect(mock.state.executedCommands).not.toEqual(
      expect.arrayContaining([
        expect.objectContaining({ command: "sim-flow.runFlow" }),
      ]),
    );
    expect(mock.state.lastDashboardPanel!.webview.posted).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: "error",
          message: "Fully-automated flow needs a spec path.",
        }),
      ]),
    );
  });

  it("switches projects during an active auto session and stops the old session", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Example session still running.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.projectLabel).toBe("example");
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).toContain("Example session still running.");

    mock.state.currentProjectDir = secondDir;
    await mock.fireActiveEditorChange();
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.projectLabel).toBe("other-project");
    expect(state?.canStop).toBe(false);
    expect(state?.transcript).toEqual([]);

    mock.state.currentProjectDir = exampleDir;
    await mock.fireActiveEditorChange();
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.projectLabel).toBe("example");
    expect(transcriptBodies(state!)).toContain("active project changed");
  });

  it("keeps the active auto session attached when the chat panel is merely revealed", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];
    mock.state.currentProjectDir = exampleDir;
    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Example session still running.\n");
        },
        waitForCancel: true,
        cancelResult: {
          status: "ended",
          endReason: "cancelled",
        },
        result: {
          status: "ended",
          endReason: "completed",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    mock.state.currentProjectDir = secondDir;
    chatView.fireVisibility();
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(state?.projectLabel).toBe("example");
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).toContain("Example session still running.");
    expect(transcriptBodies(state!)).not.toContain("active project changed");
  });

  it("switches the chat panel to project B when dashboard Play is pressed there while project A is visible", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const secondSpecPath = path.join(secondDir, "docs", "spec.md");
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];
    mock.state.currentProjectDir = exampleDir;

    mock.state.pumpScripts.set(secondDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Other project auto session launched from dashboard.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const firstDashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await firstDashboardHost.open();
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.projectLabel).toBe("example");

    const secondDashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: secondDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await secondDashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath: secondSpecPath,
    });
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.projectLabel).toBe("other-project");
    expect(mock.state.pumpLaunches.at(-1)?.projectDir).toBe(secondDir);
    expect(transcriptBodies(state!)).toContain("Other project auto session launched from dashboard.");
  });

  it("does not leak phase, tool, or artifact header state to the new project after an auto-session switch", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];

    // Script intentionally ends with the tool-activity line so
    // `currentTool` is the latched header value at the moment we
    // switch projects. `currentTool` and `currentArtifact` are
    // mutually exclusive under the new "next event clears the
    // other" semantics, so we can't have both set simultaneously
    // (and an assistant chunk would clear both as well, defeating
    // the leakage test). The artifact-write line earlier in the
    // script exercises that the artifact path doesn't permanently
    // pin the header either.
    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("_Phases:_ `discover` -> `implement`\n");
          renderer.markdown("**Phase:** `discover`\n");
          renderer.markdown("_Wrote `docs/plan.md` (128 bytes)._\n");
          renderer.markdown("_Tool `read_file` (docs/spec.md) -> ok (12 ms)._\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.currentPhase).toBe("discover");
    // Tool-activity is the LAST header-mutating line in the
    // script, so currentTool latches and currentArtifact was
    // cleared by it (next-event-clears-the-other rule).
    expect(state?.currentTool).toContain("read_file");
    expect(state?.currentArtifact).toBeNull();

    mock.state.currentProjectDir = secondDir;
    await mock.fireActiveEditorChange();
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.projectLabel).toBe("other-project");
    expect(state?.currentPhase).toBeNull();
    expect(state?.currentTool).toBeNull();
    expect(state?.currentArtifact).toBeNull();
    expect(state?.transcript).toEqual([]);
  });

  it("does not treat a custom server alias as an LLM source switch on editor-tab refresh", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "server:lmstudio-laptop");
    mock.state.config.set("llm.servers", [
      { name: "lmstudio-laptop", kind: "lmstudio", host: "127.0.0.1", port: 1234 },
    ]);

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Working through the initial grayscale decomposition.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(mock.state.pumpLaunches.at(-1)?.llmSource).toBe("lmstudio");
    expect(state?.canStop).toBe(true);

    await mock.fireActiveEditorChange();
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).not.toContain(
      "Stopped the running sim-flow session because the LLM source changed to `server:lmstudio-laptop`. Relaunching on the new source.",
    );
    expect(transcriptBodies(state!)).not.toContain("_LLM source switched:");
  });

  it("ignores duplicate Play for the same active auto session", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Session is still active.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(mock.state.pumpLaunches).toHaveLength(1);
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).not.toContain("Session already active");
  });

  it("switches llm source by stopping and relaunching the active auto session", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "vscode");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Initial source is still running.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Relaunched on Ollama.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Relaunched on the new source.",
        },
      },
    ]);
    mock.state.config.set("llm.source", "ollama");
    mock.state.config.set("llm.model", "llama3.1");

    await mock.fireConfigurationChange("llm.source", "llm.model");
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(mock.state.pumpLaunches).toHaveLength(2);
    expect(mock.state.pumpLaunches[0]?.llmSource).toBe("vscode");
    expect(mock.state.pumpLaunches[1]?.llmSource).toBe("ollama");
    expect(mock.state.pumpLaunches[1]?.llmModel).toBe("llama3.1");
    expect(transcriptBodies(state!)).toContain("new source");
    expect(transcriptBodies(state!)).toContain("Relaunched on Ollama.");
    expect(state?.canStop).toBe(false);
  });

  it("switches llm model by stopping and relaunching the active auto session on the same source", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "ollama");
    mock.state.config.set("llm.model", "llama3.1");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Initial Ollama model is still running.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Relaunched on llama3.2.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Relaunched on the new model.",
        },
      },
    ]);
    mock.state.config.set("llm.model", "llama3.2");

    await mock.fireConfigurationChange("llm.model");
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(mock.state.pumpLaunches).toHaveLength(2);
    expect(mock.state.pumpLaunches[0]?.llmSource).toBe("ollama");
    expect(mock.state.pumpLaunches[0]?.llmModel).toBe("llama3.1");
    expect(mock.state.pumpLaunches[1]?.llmSource).toBe("ollama");
    expect(mock.state.pumpLaunches[1]?.llmModel).toBe("llama3.2");
    expect(transcriptBodies(state!)).toContain("new source");
    expect(transcriptBodies(state!)).toContain("Relaunched on llama3.2.");
  });

  it("does not duplicate relaunch when llm source changes and play is pressed immediately", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "vscode");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Initial source is still running.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Relaunched on Ollama.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Relaunched on the new source.",
        },
      },
    ]);
    mock.state.config.set("llm.source", "ollama");
    mock.state.config.set("llm.model", "llama3.1");

    const sourceSwitch = mock.fireConfigurationChange("llm.source", "llm.model");
    const rerun = mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await sourceSwitch;
    await rerun;
    await flushAsyncWork();

    const state = latestState(chatView);
    const bodies = transcriptBodies(state!);
    expect(mock.state.pumpLaunches).toHaveLength(2);
    expect(countOccurrences(bodies, "Relaunched on Ollama.")).toBe(1);
    expect(countOccurrences(bodies, "Relaunched on the new source.")).toBe(1);
  });

  it("does not duplicate relaunch when the project changes and play is pressed immediately", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const secondSpecPath = path.join(secondDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Example session still running.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);
    mock.state.pumpScripts.set(secondDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Other project relaunched session.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Other project session completed.",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath: path.join(exampleDir, "docs", "spec.md"),
    });
    await flushAsyncWork();

    mock.state.currentProjectDir = secondDir;
    const projectSwitch = mock.fireActiveEditorChange();
    const rerun = provider.launchAutoSession(secondSpecPath, secondDir);
    await projectSwitch;
    await rerun;
    await flushAsyncWork();

    const state = latestState(chatView);
    const bodies = transcriptBodies(state!);
    expect(mock.state.pumpLaunches).toHaveLength(2);
    expect(mock.state.pumpLaunches[0]?.projectDir).toBe(exampleDir);
    expect(mock.state.pumpLaunches[1]?.projectDir).toBe(secondDir);
    expect(state?.projectLabel).toBe("other-project");
    expect(countOccurrences(bodies, "Other project relaunched session.")).toBe(1);
  });

  it("switches from an api source to a cli source by stopping the panel session and routing to terminal", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "vscode");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Active API-backed session.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    mock.state.config.set("llm.source", "claude-cli");
    mock.state.config.set("llm.model", "sonnet");
    await mock.fireConfigurationChange("llm.source", "llm.model");
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(mock.state.pumpLaunches).toHaveLength(1);
    expect(mock.state.executedCommands).toEqual(
      expect.arrayContaining([
        {
          command: "sim-flow.runFlowTerminal",
          args: ["claude", specPath, exampleDir],
        },
      ]),
    );
    expect(state?.sourceLabel).toContain("Claude CLI");
    expect(state?.supportsPromptEntry).toBe(false);
    expect(transcriptBodies(state!)).toContain("terminal");
  });

  it("restores the latest visible auto-session transcript after provider reload", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Partial auto-session output before reload.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(transcriptBodies(state!)).toContain("Partial auto-session output before reload.");
    expect(state?.canStop).toBe(true);

    provider.dispose();
    await flushAsyncWork();

    const restoredProvider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    const restoredView = new mock.FakeWebviewView();
    await restoredProvider.resolveWebviewView(restoredView as never, {} as never, {} as never);
    await flushAsyncWork();

    state = latestState(restoredView);
    expect(state?.projectLabel).toBe("example");
    expect(state?.canStop).toBe(false);
    expect(transcriptBodies(state!)).toContain("Partial auto-session output before reload.");
  });

  it("restores awaiting-input transcript state after provider reload", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Please choose the grayscale coefficients before continuing.\n");
        },
        result: {
          status: "awaiting-input",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.notice).toContain("waiting for your next reply");
    expect(transcriptBodies(state!)).toContain(
      "Please choose the grayscale coefficients before continuing.",
    );

    provider.dispose();
    await flushAsyncWork();

    const restoredProvider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    const restoredView = new mock.FakeWebviewView();
    await restoredProvider.resolveWebviewView(restoredView as never, {} as never, {} as never);
    await flushAsyncWork();

    state = latestState(restoredView);
    expect(state?.projectLabel).toBe("example");
    expect(state?.canStop).toBe(true);
    expect(state?.supportsPromptEntry).toBe(true);
    expect(state?.notice).toContain("waiting for your next reply");
    expect(transcriptBodies(state!)).toContain(
      "Please choose the grayscale coefficients before continuing.",
    );
  });

  it("does not treat a reply to a restored dead awaiting-input session as direct chat", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Please choose the grayscale coefficients before continuing.\n");
        },
        result: {
          status: "awaiting-input",
        },
      },
      {
        onSettle: (renderer) => {
          renderer.markdown("Resumed the restored session with Rec. 601.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Restored session completed.",
        },
      },
    ]);
    mock.state.directReplies.set(exampleDir, ["This should never be sent as direct chat."]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    provider.dispose();
    await flushAsyncWork();

    const restoredProvider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    const restoredView = new mock.FakeWebviewView();
    await restoredProvider.resolveWebviewView(restoredView as never, {} as never, {} as never);
    await flushAsyncWork();

    await restoredView.webview.emit({
      type: "send-prompt",
      prompt: "Use Rec. 601.",
    });
    await flushAsyncWork();

    const state = latestState(restoredView);
    expect(mock.state.directReplyRequests).toEqual([]);
    expect(mock.state.pumpInstances.get(exampleDir)?.sentMessages).toEqual(["Use Rec. 601."]);
    expect(transcriptBodies(state!)).toContain("Resumed the restored session with Rec. 601.");
    expect(transcriptBodies(state!)).not.toContain("This should never be sent as direct chat.");
  });

  it("routes an immediate reply back into the relaunched auto session after an llm source switch from awaiting-input", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.config.set("llm.source", "vscode");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Please choose the grayscale coefficients before continuing.\n");
        },
        result: {
          status: "awaiting-input",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.notice).toContain("waiting for your next reply");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Relaunched session is ready for your reply.\n");
        },
        result: {
          status: "awaiting-input",
        },
      },
      {
        onSettle: (renderer) => {
          renderer.markdown("Thanks. I'll use Rec. 601 after the source switch.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Resumed after the source switch.",
        },
      },
    ]);
    mock.state.config.set("llm.source", "ollama");
    mock.state.config.set("llm.model", "llama3.1");

    const sourceSwitch = mock.fireConfigurationChange("llm.source", "llm.model");
    const reply = chatView.webview.emit({
      type: "send-prompt",
      prompt: "Use Rec. 601.",
    });
    await sourceSwitch;
    await reply;
    await flushAsyncWork();

    state = latestState(chatView);
    expect(mock.state.directReplyRequests).toEqual([]);
    expect(mock.state.pumpLaunches).toHaveLength(2);
    expect(mock.state.pumpLaunches[1]?.llmSource).toBe("ollama");
    expect(mock.state.pumpInstances.get(exampleDir)?.sentMessages).toEqual(["Use Rec. 601."]);
    expect(transcriptBodies(state!)).toContain("Thanks. I'll use Rec. 601 after the source switch.");
  });

  it("restores the newly active project and source after reload instead of resurfacing the old auto session context", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];
    mock.state.config.set("llm.source", "vscode");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Example auto-session output before reload.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    provider.dispose();
    await flushAsyncWork();

    mock.state.currentProjectDir = secondDir;
    mock.state.config.set("llm.source", "ollama");
    mock.state.config.set("llm.model", "llama3.1");

    const restoredProvider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    const restoredView = new mock.FakeWebviewView();
    await restoredProvider.resolveWebviewView(restoredView as never, {} as never, {} as never);
    await flushAsyncWork();

    let state = latestState(restoredView);
    expect(state?.projectLabel).toBe("other-project");
    expect(state?.sourceLabel).toContain("Ollama");
    expect(state?.transcript).toEqual([]);

    mock.state.currentProjectDir = exampleDir;
    await mock.fireActiveEditorChange();
    await flushAsyncWork();

    state = latestState(restoredView);
    expect(state?.projectLabel).toBe("example");
    expect(transcriptBodies(state!)).toContain("Example auto-session output before reload.");
  });

  it("does not launch an auto session when no active project can be resolved", async () => {
    mock.state.currentProjectDir = undefined;
    mock.state.workspaceFolders = [];

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const view = new mock.FakeWebviewView();
    await provider.resolveWebviewView(view as never, {} as never, {} as never);

    await provider.launchAutoSession(undefined, undefined);
    await flushAsyncWork();

    const state = latestState(view);
    expect(state?.projectLabel).toBe("No project selected");
    expect(mock.state.pumpLaunches).toEqual([]);
    expect(state?.transcript).toEqual([]);
  });

  it("records session completion cleanly when the orchestrator ends without visible assistant content", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Mock session ended without model output.",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(state?.canStop).toBe(false);
    expect(transcriptBodies(state!)).toContain("Started sim-flow auto");
    expect(transcriptBodies(state!)).toContain("Mock session ended without model output.");
  });

  it("keeps unexpected orchestrator markdown visible without corrupting header state", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Unexpected orchestrator blob: <<phase=??? tool=???>>\n");
          renderer.markdown("More raw text that should remain visible.\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Unexpected markdown handled.",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(state?.currentPhase).toBeNull();
    expect(state?.currentTool).toBeNull();
    expect(state?.currentArtifact).toBeNull();
    expect(transcriptBodies(state!)).toContain("Unexpected orchestrator blob");
    expect(transcriptBodies(state!)).toContain("More raw text that should remain visible.");
  });

  it("resumes a mocked auto session after the orchestrator asks for input", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("Please confirm the grayscale coefficients before I continue.\n");
        },
        result: {
          status: "awaiting-input",
        },
      },
      {
        onSettle: (renderer) => {
          renderer.requestTokensEstimate?.(128);
          renderer.markdown(" Thanks. I'll use Rec. 601 luma coefficients and continue.\n");
          markPassed(exampleDir, "DM0", "DM1");
          renderer.markdown("\n**Advanced past `DM0`; current step is now `DM1`.**\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Mock session resumed and advanced to DM1.",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.notice).toContain("waiting for your next reply");
    expect(state?.canStop).toBe(true);

    await chatView.webview.emit({
      type: "send-prompt",
      prompt: "Use Rec. 601.",
    });
    await flushAsyncWork();

    state = latestState(chatView);
    expect(mock.state.pumpInstances.get(exampleDir)?.sentMessages).toEqual(["Use Rec. 601."]);
    expect(state?.currentStep).toBe("DM1");
    expect(state?.canStop).toBe(false);
    expect(transcriptBodies(state!)).toContain("Thanks. I'll use Rec. 601 luma coefficients");
  });

  it("does not start direct chat when a prompt arrives during an active auto session", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.directReplies.set(exampleDir, ["This direct reply should never start."]);
    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("Still working through the initial decomposition.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    await chatView.webview.emit({
      type: "send-prompt",
      prompt: "This should be blocked while the session is still running.",
    });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(mock.state.directReplyRequests).toEqual([]);
    expect(mock.state.pumpInstances.get(exampleDir)?.sentMessages).toEqual([]);
    expect(transcriptBodies(state!)).not.toContain("This direct reply should never start.");
    expect(transcriptBodies(state!)).toContain("Still working through the initial decomposition.");
  });

  it("stops a running auto session and relaunches it cleanly", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("Working through the initial grayscale decomposition.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    let state = latestState(chatView);
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).toContain("Working through the initial grayscale decomposition.");

    await chatView.webview.emit({ type: "stop-conversation" });
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.canStop).toBe(false);
    expect(state?.currentPhase).toBeNull();
    expect(state?.currentTool).toBeNull();
    expect(state?.currentArtifact).toBeNull();
    expect(transcriptBodies(state!)).toContain("Cancellation requested for the running sim-flow session.");
    expect(transcriptBodies(state!)).toContain("Stopped the running sim-flow session.");

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("Relaunched session reached DM1.\n");
          markPassed(exampleDir, "DM0", "DM1");
          renderer.markdown("\n**Advanced past `DM0`; current step is now `DM1`.**\n");
        },
        result: {
          status: "ended",
          endReason: "completed",
          endMessage: "Relaunched mock session completed.",
        },
      },
    ]);

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    state = latestState(chatView);
    expect(state?.currentStep).toBe("DM1");
    expect(transcriptBodies(state!)).toContain("Relaunched session reached DM1.");
    expect(transcriptBodies(state!)).not.toContain(
      "Working through the initial grayscale decomposition.",
    );
  });

  it("does not clear the transcript while an auto session is active", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("Working through the initial grayscale decomposition.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    await chatView.webview.emit({ type: "clear-transcript" });
    await flushAsyncWork();

    const state = latestState(chatView);
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).toContain("Working through the initial grayscale decomposition.");
    expect(transcriptBodies(state!)).toContain("Started sim-flow auto for `example`");
  });

  it("does not append duplicate stop notes when stop is pressed repeatedly during an auto session", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Step `DM0` auto session** — mock orchestrator.\n\n");
          renderer.markdown("Working through the initial grayscale decomposition.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    await chatView.webview.emit({ type: "stop-conversation" });
    await chatView.webview.emit({ type: "stop-conversation" });
    await chatView.webview.emit({ type: "refresh" });
    await flushAsyncWork();

    const state = latestState(chatView);
    const bodies = transcriptBodies(state!);
    expect(countOccurrences(bodies, "Cancellation requested for the running sim-flow session.")).toBe(1);
    expect(countOccurrences(bodies, "Stopped the running sim-flow session.")).toBe(1);
  });

  it("stops cleanly during a gate and step-transition burst", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const specPath = path.join(exampleDir, "docs", "spec.md");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];

    mock.state.pumpScripts.set(exampleDir, [
      {
        onSettle: (renderer) => {
          renderer.markdown("**Gate `DM0`: clean.**\n");
          markPassed(exampleDir, "DM0", "DM1");
          renderer.markdown("\n**Advanced past `DM0`; current step is now `DM1`.**\n");
          renderer.markdown("Preparing the DM1 follow-up work.\n");
        },
        waitForCancel: true,
        result: {
          status: "ended",
        },
      },
    ]);

    const workspaceState = new mock.FakeMemento();
    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      workspaceState as never,
      { get: async () => undefined },
    );
    mock.state.chatProvider = provider as never;
    const chatView = new mock.FakeWebviewView();
    await provider.resolveWebviewView(chatView as never, {} as never, {} as never);

    const dashboardHost = new DashboardHost({
      extensionUri: { fsPath: "/extension" } as never,
      projectDir: exampleDir,
      cli: {} as never,
      workspaceState: workspaceState as never,
    });
    await dashboardHost.open();

    await mock.state.lastDashboardPanel!.webview.emit({
      type: "run-auto",
      specPath,
    });
    await flushAsyncWork();

    await chatView.webview.emit({ type: "stop-conversation" });
    await flushAsyncWork();

    const state = latestState(chatView);
    const bodies = transcriptBodies(state!);
    expect(state?.currentStep).toBe("DM1");
    expect(bodies).toContain("Gate `DM0`: clean.");
    expect(bodies).toContain("Advanced past `DM0`; current step is now `DM1`.");
    expect(bodies).toContain("Preparing the DM1 follow-up work.");
    expect(bodies).toContain("Cancellation requested for the running sim-flow session.");
    expect(bodies).toContain("Stopped the running sim-flow session.");
  });

});
