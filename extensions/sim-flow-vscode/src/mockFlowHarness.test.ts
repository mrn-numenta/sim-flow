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
    readonly sentMessages: string[] = [];
    private readonly turns: PumpTurn[];
    private index = 0;
    private cancelled = false;
    private readonly cancelWaiters: Array<() => void> = [];
    readonly args: string[];
    readonly llmSource: string | undefined;
    readonly llmModel: string | undefined;

    constructor(
      options: { cwd: string; args?: string[] },
      llmConfig?: { source?: string; model?: string },
    ) {
      this.projectDir = options.cwd;
      this.turns = state.pumpScripts.get(this.projectDir) ?? [];
      this.args = options.args ?? [];
      this.llmSource = llmConfig?.source;
      this.llmModel = llmConfig?.model;
      state.pumpInstances.set(this.projectDir, this);
      state.pumpLaunches.push({
        projectDir: this.projectDir,
        args: [...this.args],
        llmSource: this.llmSource,
        llmModel: this.llmModel,
      });
    }

    async settle(renderer: {
      markdown(text: string): void;
      requestTokensEstimate?(tokens: number): void;
    }): Promise<{ status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string }> {
      const turn = this.turns[this.index++] ?? { result: { status: "ended" as const } };
      await turn.onSettle?.(renderer);
      if (turn.waitForCancel && !this.cancelled) {
        await new Promise<void>((resolve) => {
          this.cancelWaiters.push(resolve);
        });
      }
      if (turn.waitForCancel && this.cancelled) {
        return (
          turn.cancelResult ?? {
            status: "ended" as const,
            endReason: "cancelled",
            endMessage: "Mock session cancelled.",
          }
        );
      }
      return turn.result;
    }

    sendUserMessage(text: string): void {
      this.sentMessages.push(text);
    }

    cancel(): void {
      this.cancelled = true;
      for (const resolve of this.cancelWaiters.splice(0)) {
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
      ["auto.maxWorkIterations", 3],
      ["auto.maxCritiqueIterations", 3],
      ["dashboard.showFullyAutomated", false],
      ["dashboard.verilogSimEnabled", false],
      ["dashboard.verilogSimulatorPath", ""],
    ]);
    state.projectStates = new Map<string, unknown>();
    state.directReplies = new Map<string, string[]>();
    state.pumpScripts = new Map<string, PumpTurn[]>();
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

vi.mock("./chatPanel/session", () => ({
  supportsPanelTransport: () => true,
  buildPanelMessages: (
    context: { projectDir: string | null; currentStep: string | null },
    transcript: Array<{ kind: string; body: string }>,
    verbose: boolean,
  ) => [
    {
      role: "system",
      content: `${context.projectDir ?? ""} ${context.currentStep ?? ""} ${verbose ? "verbose" : "concise"}`,
    },
    ...transcript
      .filter((entry) => entry.kind === "assistant" || entry.kind === "user")
      .map((entry) => ({
        role: entry.kind,
        content: entry.body,
      })),
  ],
  streamPanelReply: async function* (
    config: { source?: string; model?: string },
    context: { projectDir: string | null },
    token: { isCancellationRequested: boolean },
  ): AsyncIterable<string> {
    mock.state.directReplyRequests.push({
      projectDir: context.projectDir,
      source: config.source,
      model: config.model,
    });
    const reply = mock.state.directReplies.get(context.projectDir ?? "__workspace__") ?? [
      "Mock reply.",
    ];
    for (const chunk of reply) {
      if (token.isCancellationRequested) {
        return;
      }
      if (typeof chunk === "string") {
        yield chunk;
        continue;
      }
      if (chunk.waitForSignal) {
        await mock.waitForSignal(chunk.waitForSignal);
        if (token.isCancellationRequested) {
          return;
        }
      }
      yield chunk.text;
    }
  },
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
        notice: string;
        canStop: boolean;
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

  it("switches chat panel state between projects without mixing transcripts", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    const secondDir = createProjectFromFixture(tmpRoot, "other-project");
    mock.state.directReplies.set(exampleDir, ["Example", " reply."]);
    mock.state.directReplies.set(secondDir, ["Other", " reply."]);
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
      { uri: { fsPath: secondDir }, name: "other-project", index: 1 },
    ];
    mock.state.currentProjectDir = exampleDir;

    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      new mock.FakeMemento() as never,
      { get: async () => undefined },
    );
    const view = new mock.FakeWebviewView();
    await provider.resolveWebviewView(view as never, {} as never, {} as never);

    await view.webview.emit({ type: "send-prompt", prompt: "Hello from example" });
    await flushAsyncWork();
    let state = latestState(view);
    expect(state?.projectLabel).toBe("example");
    expect(transcriptBodies(state!)).toContain("Example reply.");

    mock.state.currentProjectDir = secondDir;
    await view.webview.emit({ type: "refresh" });
    await flushAsyncWork();
    state = latestState(view);
    expect(state?.projectLabel).toBe("other-project");
    expect(state?.currentStep).toBe("DM2a");
    expect(state?.transcript).toEqual([]);

    await view.webview.emit({ type: "send-prompt", prompt: "Hello from other project" });
    await flushAsyncWork();
    state = latestState(view);
    expect(state?.projectLabel).toBe("other-project");
    expect(transcriptBodies(state!)).toContain("Other reply.");

    mock.state.currentProjectDir = exampleDir;
    await view.webview.emit({ type: "refresh" });
    await flushAsyncWork();
    state = latestState(view);
    expect(state?.projectLabel).toBe("example");
    expect(transcriptBodies(state!)).toContain("Example reply.");
    expect(transcriptBodies(state!)).not.toContain("Other reply.");
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

  it("stops an in-flight direct panel reply without losing the stop note", async () => {
    const exampleDir = createProjectFromFixture(tmpRoot, "example");
    mock.state.currentProjectDir = exampleDir;
    mock.state.workspaceFolders = [
      { uri: { fsPath: exampleDir }, name: "example", index: 0 },
    ];
    mock.state.directReplies.set(exampleDir, [
      "First chunk.",
      {
        text: " Second chunk that should never be rendered.",
        waitForSignal: "release-direct-stop",
      },
    ]);

    const provider = new ChatPanelProvider(
      { fsPath: "/extension" } as never,
      new mock.FakeMemento() as never,
      { get: async () => undefined },
    );
    const view = new mock.FakeWebviewView();
    await provider.resolveWebviewView(view as never, {} as never, {} as never);

    const sendPromise = view.webview.emit({
      type: "send-prompt",
      prompt: "Summarize the example project.",
    });
    await flushAsyncWork();

    let state = latestState(view);
    expect(state?.canStop).toBe(true);
    expect(transcriptBodies(state!)).toContain("First chunk.");

    const stopPromise = view.webview.emit({ type: "stop-conversation" });
    await flushAsyncWork();
    mock.resolveSignal("release-direct-stop");
    await sendPromise;
    await stopPromise;
    await flushAsyncWork();

    state = latestState(view);
    expect(state?.canStop).toBe(false);
    expect(transcriptBodies(state!)).toContain("Cancellation requested for the current model response.");
    expect(transcriptBodies(state!)).not.toContain("Second chunk that should never be rendered.");
  });
});
