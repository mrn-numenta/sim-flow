import * as path from "node:path";
import { randomUUID } from "node:crypto";
import * as os from "node:os";

import * as vscode from "vscode";

import { findProjectCandidates, resolveContext, resolveProjectDir } from "../context";
import { LlmError, type LlmSource, type SecretStorage } from "../llm";
import { estimateMessagesTokens } from "../llm/tokenEstimate";
import { type PumpLlmConfig } from "../session/pump";
import { SocketSessionPump } from "../session/socketPump";
import { readFlowState } from "../state/flowState";
import {
  cliBackendArgFor,
  isTerminalLlmSource,
  LLM_SOURCE_LABELS,
  type LlmSourceTag,
} from "../webview/messages";

import type {
  ChatPanelState,
  HostMessage,
  WebviewMessage,
} from "./messages";
import {
  AutoSessionManager,
  type AutoSessionDriveDelegate,
  type ManagedAutoSessionState,
  type ManagedStepRef,
  type StoredAutoSessionRecord,
} from "./autoSessionManager";
import {
  appendAssistantChunk,
  appendAssistantPlaceholder,
  appendNote,
  appendUserPrompt,
  clearConversationState,
  completeAssistantTurn,
  createConversationState,
  filterPresentationEntries,
  setEntryRequestTokensEstimate,
  stripToolCallFencesForStreaming,
  summarizeTokenEstimates,
  toStoredConversation,
  type ChatConversationState,
} from "./state";
import { buildPanelMessages, streamPanelReply, supportsPanelTransport } from "./session";

export const CHAT_PANEL_VIEW_ID = "simFlow.chatPanel";
export const CHAT_PANEL_CONTAINER_ID = "sim-flow-chat-panel";

let pendingConversationWrites: Promise<void> = Promise.resolve();

interface DirectResponseState {
  projectDir: string | null;
  source: vscode.CancellationTokenSource;
  sourceTag: LlmSourceTag;
  model: string;
  stopRequested: boolean;
}

interface PendingAutoLaunchState {
  projectDir: string;
  launchSpecPath: string | undefined;
  sourceTag: LlmSourceTag;
  model: string;
}

export class ChatPanelProvider implements vscode.WebviewViewProvider, vscode.Disposable {
  private view: vscode.WebviewView | undefined;
  private readonly disposables: vscode.Disposable[] = [];
  private readonly conversations = new Map<string, ChatConversationState>();
  private inFlight: DirectResponseState | undefined;
  private pendingAutoLaunch: PendingAutoLaunchState | undefined;
  private disposed = false;
  private refreshing = false;
  private refreshQueued = false;
  private reconcilePromise: Promise<void> | undefined;
  private postChain: Promise<void> = Promise.resolve();

  private get activePump(): ManagedAutoSessionState | undefined {
    return this.autoSessions.getActiveSession();
  }

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly workspaceState: vscode.Memento,
    private readonly secrets: SecretStorage,
    private readonly autoSessions: AutoSessionManager = new AutoSessionManager(workspaceState),
  ) {
    this.disposables.push(
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (
          event.affectsConfiguration("sim-flow.llm.source") ||
          event.affectsConfiguration("sim-flow.llm.model") ||
          event.affectsConfiguration("sim-flow.llm.verbose") ||
          event.affectsConfiguration("sim-flow.llm.ollama.baseUrl") ||
          event.affectsConfiguration("sim-flow.llm.lmstudio.baseUrl")
        ) {
          void this.refresh();
        }
      }),
      vscode.window.onDidChangeActiveTextEditor(() => {
        void this.refresh();
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        void this.refresh();
      }),
    );
  }

  dispose(): void {
    this.disposed = true;
    this.refreshQueued = false;
    if (this.inFlight) {
      const projectDir = this.inFlight.projectDir;
      const conversation = appendNote(
        this.readConversation(projectDir),
        "Session interrupted",
        "Stopped the current response because the chat panel was reloaded or closed.",
      );
      void this.persistConversation(projectDir, conversation);
    }
    if (this.inFlight) {
      this.inFlight.source.cancel();
      this.inFlight = undefined;
    }
    this.view = undefined;
    for (const d of this.disposables) {
      d.dispose();
    }
    this.disposables.length = 0;
  }

  async resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext<unknown>,
    _token: vscode.CancellationToken,
  ): Promise<void> {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.extensionUri, "dist", "webview"),
        vscode.Uri.joinPath(this.extensionUri, "media"),
      ],
    };
    webviewView.webview.html = this.renderHtml(webviewView.webview);
    webviewView.webview.onDidReceiveMessage(
      (msg: WebviewMessage) => {
        void this.onMessage(msg);
      },
      undefined,
      this.disposables,
    );
    webviewView.onDidChangeVisibility(
      () => {
        if (webviewView.visible) {
          void this.refresh();
        }
      },
      undefined,
      this.disposables,
    );
    await this.refresh();
  }

  private async onMessage(msg: WebviewMessage): Promise<void> {
    switch (msg.type) {
      case "ready":
      case "refresh":
        await this.refresh();
        return;
      case "send-prompt":
        await this.sendPrompt(msg.prompt);
        return;
      case "clear-transcript":
        await this.clearTranscript();
        return;
      case "stop-conversation":
        await this.stopConversation();
        return;
      default:
        return;
    }
  }

  private async refresh(): Promise<void> {
    if (!this.view || this.disposed) {
      return;
    }
    if (this.refreshing) {
      this.refreshQueued = true;
      return;
    }
    this.refreshing = true;
    try {
      await waitForPendingConversationWrites();
      if (!this.view || this.disposed) {
        return;
      }
      await this.restoreActiveAutoSessionIfNeeded();
      await this.reconcileModeSwitches();
      const context = await this.readPanelContext();
      await this.postState(context, this.readConversation(context.projectDir));
    } finally {
      this.refreshing = false;
      if (this.refreshQueued && this.view && !this.disposed) {
        this.refreshQueued = false;
        void this.refresh();
      }
    }
  }

  private async sendPrompt(promptRaw: string): Promise<void> {
    const prompt = promptRaw.trim();
    if (prompt.length === 0) {
      return;
    }
    await this.reconcileModeSwitches();
    if (this.inFlight) {
      return;
    }
    const context = await this.readPanelContext();
    if (
      this.pendingAutoLaunch &&
      this.pendingAutoLaunch.projectDir === context.projectDir
    ) {
      return;
    }
    if (this.activePump) {
      if (
        this.activePump.projectDir === context.projectDir &&
        this.activePump.awaitingInput
      ) {
        await this.sendPumpPrompt(context, prompt);
      }
      return;
    }
    let conversation = this.readConversation(context.projectDir);
    if (hasInterruptedAutoSessionTranscript(conversation.transcript)) {
      conversation = appendNote(
        conversation,
        "Session no longer live",
        "Relaunch the flow from the dashboard or clear the transcript to start a fresh direct chat.",
      );
      await this.persistConversation(context.projectDir, conversation);
      await this.postState(context, conversation);
      return;
    }

    if (!supportsPanelTransport(context.source)) {
      conversation = appendNote(
        conversation,
        "Unsupported source",
        'This panel only drives API backends. Switch `sim-flow.llm.source` to `lmstudio`, `ollama`, `openai`, `anthropic`, or `vscode` to send prompts here.',
        "error",
      );
      await this.persistConversation(context.projectDir, conversation);
      await this.postState(context, conversation);
      return;
    }

    const requestTokensEstimate = estimateMessagesTokens(
      buildPanelMessages(
        context,
        [
          ...conversation.transcript,
          {
            id: "preview-user",
            kind: "user",
            title: "You",
            body: prompt,
            meta: userMeta(context),
          },
        ],
        context.verbose,
      ),
    );
    const { state: started, assistantId } = appendUserPrompt(
      conversation,
      prompt,
      userMeta(context),
      assistantMeta(context),
      requestTokensEstimate,
    );
    conversation = started;
    await this.persistConversation(context.projectDir, conversation);

    const source = new vscode.CancellationTokenSource();
    this.inFlight = {
      projectDir: context.projectDir,
      source,
      sourceTag: context.source,
      model: context.model,
      stopRequested: false,
    };
    await this.postState(context, conversation);

    try {
      for await (const chunk of streamPanelReply(
        {
          source: context.source,
          model: context.model,
          verbose: context.verbose,
          ollamaBaseUrl: context.ollamaBaseUrl,
          lmstudioBaseUrl: context.lmstudioBaseUrl,
          secrets: this.secrets,
        },
        {
          projectDir: context.projectDir,
          currentStep: context.currentStep,
          transcript: conversation.transcript,
        },
        source.token,
      )) {
        conversation = appendAssistantChunk(conversation, assistantId, chunk);
        this.rememberConversation(context.projectDir, conversation);
        await this.postState(context, conversation);
      }
      conversation = completeAssistantTurn(
        this.readConversation(context.projectDir),
        assistantId,
      );
    } catch (error) {
      conversation = this.readConversation(context.projectDir);
      conversation = completeAssistantTurn(
        conversation,
        assistantId,
        "The request failed before the model returned any text.",
      );
      if (error instanceof LlmError && error.kind === "cancelled") {
        conversation = appendNote(
          conversation,
          "Response stopped",
          "Stopped the current response at the user's request.",
        );
      } else {
        conversation = appendNote(
          conversation,
          `${context.sourceLabel} error`,
          formatChatError(error),
          "error",
        );
      }
    } finally {
      const settledProjectDir = context.projectDir;
      this.inFlight = undefined;
      await this.persistConversation(settledProjectDir, conversation);
      const latestContext = await this.readPanelContext();
      await this.postState(latestContext, this.readConversation(latestContext.projectDir));
    }
  }

  private async clearTranscript(): Promise<void> {
    if (this.inFlight || this.activePump) {
      return;
    }
    const context = await this.readPanelContext();
    const conversation = clearConversationState();
    await this.persistConversation(context.projectDir, conversation);
    await this.postState(context, conversation);
  }

  private async stopConversation(): Promise<void> {
    const context = await this.readPanelContext();
    let conversation = this.readConversation(context.projectDir);

    if (this.inFlight?.projectDir === context.projectDir) {
      if (this.inFlight.stopRequested) {
        return;
      }
      this.inFlight.stopRequested = true;
      this.inFlight.source.cancel();
      conversation = appendNote(
        conversation,
        "Stopping response",
        "Cancellation requested for the current model response.",
      );
      await this.persistConversation(context.projectDir, conversation);
      await this.postState(context, conversation);
      return;
    }

    if (this.activePump?.projectDir === context.projectDir) {
      if (this.activePump.stopRequested) {
        return;
      }
      this.activePump.stopRequested = true;
      conversation = appendNote(
        conversation,
        "Stopping session",
        "Cancellation requested for the running sim-flow session.",
      );
      await this.persistConversation(context.projectDir, conversation);
      await this.postState(context, conversation);
      await this.autoSessions.cancel(this.activePump);
    }
  }

  private async readPanelContext(): Promise<PanelContext> {
    const projectDir = await resolveProjectDirForPanel();
    const settings = readPanelSettings();
    const currentStep = projectDir ? await readCurrentStepSafe(projectDir) : null;
    const projectLabel =
      projectDir !== null
        ? path.basename(projectDir)
        : vscode.workspace.workspaceFolders?.[0]?.name ?? "No project selected";

    return {
      projectLabel,
      projectDir,
      currentStep,
      source: settings.source,
      sourceLabel: settings.sourceLabel,
      model: settings.model,
      verbose: settings.verbose,
      ollamaBaseUrl: settings.ollamaBaseUrl,
      lmstudioBaseUrl: settings.lmstudioBaseUrl,
      ...describePanelSession(projectDir, currentStep, settings.sourceLabel, this.activePump),
    };
  }

  async launchAutoSession(
    specPath: string | undefined,
    projectDirHint: string | undefined,
  ): Promise<void> {
    await vscode.commands.executeCommand(
      `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
    );
    const ctx = await resolveContext({
      projectDir: projectDirHint,
      showErrors: true,
    });
    if (!ctx) {
      return;
    }

    const settings = readPanelSettings();
    const trimmedSpec = normalizeSpecPath(specPath);
    if (
      this.activePump &&
      this.activePump.projectDir === ctx.projectDir &&
      this.activePump.sessionMode === "auto" &&
      this.activePump.launchSpecPath === trimmedSpec &&
      this.activePump.sourceTag === settings.source &&
      this.activePump.model === settings.model
    ) {
      return;
    }
    if (
      this.pendingAutoLaunch &&
      this.pendingAutoLaunch.projectDir === ctx.projectDir &&
      this.pendingAutoLaunch.launchSpecPath === trimmedSpec &&
      this.pendingAutoLaunch.sourceTag === settings.source &&
      this.pendingAutoLaunch.model === settings.model
    ) {
      return;
    }

    if (this.inFlight) {
      await this.stopDirectResponse(
        this.inFlight,
        "Launching flow",
        "Stopped the current response to launch a sim-flow session from the dashboard.",
      );
    }
    if (this.activePump) {
      await this.stopActivePumpSession(
        this.activePump,
        "Launching new flow",
        "Stopped the running sim-flow session to launch a new flow.",
      );
    }

    await this.startAutoSession(
      ctx,
      trimmedSpec,
      { resetConversation: true },
    );
  }

  async launchStepSession(
    step: string,
    kind: "work" | "critique",
    projectDirHint: string | undefined,
  ): Promise<void> {
    await vscode.commands.executeCommand(
      `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
    );
    const ctx = await resolveContext({
      projectDir: projectDirHint,
      showErrors: true,
    });
    if (!ctx) {
      return;
    }

    const settings = readPanelSettings();
    const stepRef: ManagedStepRef = { step, kind };
    if (
      this.activePump &&
      this.activePump.projectDir === ctx.projectDir &&
      this.activePump.sessionMode === "step" &&
      this.activePump.stepRef?.step === step &&
      this.activePump.stepRef?.kind === kind &&
      this.activePump.sourceTag === settings.source &&
      this.activePump.model === settings.model
    ) {
      return;
    }

    if (this.inFlight) {
      await this.stopDirectResponse(
        this.inFlight,
        "Launching step session",
        `Stopped the current response to launch \`${step}.${kind}\` from the dashboard.`,
      );
    }
    if (this.activePump) {
      await this.stopActivePumpSession(
        this.activePump,
        "Launching new session",
        `Stopped the running sim-flow session to launch \`${step}.${kind}\`.`,
      );
    }

    await this.startStepSession(ctx, stepRef, { resetConversation: true });
  }

  private async startAutoSession(
    ctx: { projectDir: string; cli: { binary: string; foundationRoot?: string } },
    trimmedSpec: string | undefined,
    options: { resetConversation: boolean; launchTitle?: string; launchBody?: string },
  ): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const llmConfig = buildPumpLlmConfig(ctx, this.secrets, config);
    const maxWorkIters = config.get<number>("auto.maxWorkIterations") ?? 3;
    const maxCritiqueIters = config.get<number>("auto.maxCritiqueIterations") ?? 3;

    const sessionId = randomUUID();
    const socketPath = reconnectableSocketPath(sessionId);
    const args = ["auto", "--transport-socket", socketPath];
    if (ctx.cli.foundationRoot) {
      args.push("--foundation-root", ctx.cli.foundationRoot);
    }
    args.push("--project", ctx.projectDir);
    args.push("--llm-backend", llmConfig.source);
    if (llmConfig.model) {
      args.push("--llm-model", llmConfig.model);
    }
    args.push("--max-auto-iters", String(maxWorkIters));
    args.push("--max-critique-iters", String(maxCritiqueIters));
    if (trimmedSpec) {
      args.push("--spec", trimmedSpec);
    } else {
      args.push("--dm0-interactive");
    }

    const context = await this.readPanelContextForProject(ctx.projectDir);
    let conversation = options.resetConversation
      ? clearConversationState()
      : this.readConversation(ctx.projectDir);
    conversation = appendNote(
      conversation,
      options.launchTitle ?? "Flow launched from dashboard",
      options.launchBody ??
        (trimmedSpec
          ? `Started sim-flow auto for \`${path.basename(ctx.projectDir)}\` with spec \`${trimmedSpec}\`.`
          : "Started sim-flow auto without a spec; DM0 will stop for input before the rest of the flow continues."),
    );
    await this.persistConversation(ctx.projectDir, conversation);
    await this.postState(context, conversation);

    this.pendingAutoLaunch = {
      projectDir: ctx.projectDir,
      launchSpecPath: trimmedSpec,
      sourceTag: llmConfig.source as LlmSourceTag,
      model: llmConfig.model ?? "",
    };
    try {
      const pump = new SocketSessionPump(
        {
          sessionId,
          socketPath,
          launch: {
            binary: ctx.cli.binary,
            args,
            cwd: ctx.projectDir,
          },
        },
        llmConfig,
      );
      await pump.ready();
      await this.autoSessions.launch(
        {
          sessionId,
          socketPath,
          projectDir: ctx.projectDir,
          pump,
          sourceTag: llmConfig.source as LlmSourceTag,
          model: llmConfig.model ?? "",
          sessionMode: "auto",
          stepRef: null,
          launchSpecPath: trimmedSpec,
        },
        this.autoSessionDelegate(),
      );
    } finally {
      if (
        this.pendingAutoLaunch?.projectDir === ctx.projectDir &&
        this.pendingAutoLaunch.launchSpecPath === trimmedSpec
      ) {
        this.pendingAutoLaunch = undefined;
      }
    }
  }

  private async startStepSession(
    ctx: { projectDir: string; cli: { binary: string; foundationRoot?: string } },
    stepRef: ManagedStepRef,
    options: { resetConversation: boolean; launchTitle?: string; launchBody?: string },
  ): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const llmConfig = buildPumpLlmConfig(ctx, this.secrets, config);

    const sessionId = randomUUID();
    const socketPath = reconnectableSocketPath(sessionId);
    const args = ["session", `${stepRef.step}.${stepRef.kind}`, "--transport-socket", socketPath];
    if (ctx.cli.foundationRoot) {
      args.push("--foundation-root", ctx.cli.foundationRoot);
    }
    args.push("--project", ctx.projectDir);
    args.push("--llm-backend", llmConfig.source);
    if (llmConfig.model) {
      args.push("--llm-model", llmConfig.model);
    }

    const context = await this.readPanelContextForProject(ctx.projectDir);
    let conversation = options.resetConversation
      ? clearConversationState()
      : this.readConversation(ctx.projectDir);
    conversation = appendNote(
      conversation,
      options.launchTitle ?? "Step session launched",
      options.launchBody ??
        `Started sim-flow session \`${stepRef.step}.${stepRef.kind}\` for \`${path.basename(ctx.projectDir)}\`.`,
    );
    await this.persistConversation(ctx.projectDir, conversation);
    await this.postState(context, conversation);

    const pump = new SocketSessionPump(
      {
        sessionId,
        socketPath,
        launch: {
          binary: ctx.cli.binary,
          args,
          cwd: ctx.projectDir,
        },
      },
      llmConfig,
    );
    await pump.ready();
    await this.autoSessions.launch(
      {
        sessionId,
        socketPath,
        projectDir: ctx.projectDir,
        pump,
        sourceTag: llmConfig.source as LlmSourceTag,
        model: llmConfig.model ?? "",
        sessionMode: "step",
        stepRef,
        launchSpecPath: undefined,
      },
      this.autoSessionDelegate(),
    );
  }

  private buildState(
    context: PanelContext,
    conversation: ChatConversationState,
  ): ChatPanelState {
    const tokenTotals = summarizeTokenEstimates(conversation.transcript);
    const hasInterruptedAutoSession =
      this.activePump?.projectDir !== context.projectDir &&
      hasInterruptedAutoSessionTranscript(conversation.transcript);
    const awaitingPumpInput =
      !!this.activePump &&
      this.activePump.projectDir === context.projectDir &&
      this.activePump.awaitingInput;
    const isStreaming =
      this.inFlight?.projectDir === context.projectDir ||
      (!!this.pendingAutoLaunch &&
        this.pendingAutoLaunch.projectDir === context.projectDir) ||
      (!!this.activePump &&
        this.activePump.projectDir === context.projectDir &&
        !this.activePump.awaitingInput);
    const supportsPromptEntry =
      (!!this.activePump &&
        this.activePump.projectDir === context.projectDir &&
        this.activePump.awaitingInput) ||
      (!isTerminalLlmSource(context.source) && !hasInterruptedAutoSession);
    return {
      mode: "live",
      projectLabel: context.projectLabel,
      projectDir: context.projectDir,
      currentStep: context.currentStep,
      currentPhase:
        this.activePump?.projectDir === context.projectDir
          ? this.activePump.currentPhase
          : null,
      currentTool:
        this.activePump?.projectDir === context.projectDir
          ? this.activePump.currentTool
          : null,
      currentArtifact:
        this.activePump?.projectDir === context.projectDir
          ? this.activePump.currentArtifact
          : null,
      source: context.source,
      sourceLabel: context.sourceLabel,
      model: context.model,
      verbose: context.verbose,
      sessionLabel: context.sessionLabel,
      statusLine: context.statusLine,
      notice: awaitingPumpInput
        ? "sim-flow is waiting for your next reply in this session."
        : hasInterruptedAutoSession
          ? "This restored sim-flow session is no longer live. Relaunch the flow from the dashboard or clear the transcript to start a fresh direct chat."
          : buildNotice(context, isStreaming),
      totalInputTokensEstimate: tokenTotals.input,
      totalOutputTokensEstimate: tokenTotals.output,
      transcript: filterPresentationEntries(conversation.transcript),
      isStreaming,
      supportsPromptEntry,
      canStop:
        !!this.inFlight ||
        !!this.activePump ||
        (!!this.pendingAutoLaunch &&
          this.pendingAutoLaunch.projectDir === context.projectDir),
    };
  }

  private readConversation(projectDir: string | null): ChatConversationState {
    const key = conversationStorageKey(projectDir);
    const cached = this.conversations.get(key);
    if (cached) {
      return cached;
    }
    const stored = this.workspaceState.get<ReturnType<typeof toStoredConversation>>(key);
    const conversation = createConversationState(stored);
    this.conversations.set(key, conversation);
    return conversation;
  }

  private rememberConversation(
    projectDir: string | null,
    conversation: ChatConversationState,
  ): void {
    this.conversations.set(conversationStorageKey(projectDir), conversation);
  }

  private async persistConversation(
    projectDir: string | null,
    conversation: ChatConversationState,
  ): Promise<void> {
    const key = conversationStorageKey(projectDir);
    const stored = toStoredConversation(conversation);
    this.conversations.set(key, conversation);
    await queueConversationWrite(async () => {
      await this.workspaceState.update(key, stored);
    });
  }

  private async postState(
    context: PanelContext,
    conversation: ChatConversationState,
  ): Promise<void> {
    await this.post({
      type: "state-update",
      state: this.buildState(context, conversation),
    });
  }

  private async post(message: HostMessage): Promise<void> {
    await this.enqueuePost(async () => {
      await this.view?.webview.postMessage(message);
    });
  }

  private async sendPumpPrompt(
    context: PanelContext,
    prompt: string,
  ): Promise<void> {
    const session = this.activePump;
    if (!session || session.projectDir !== context.projectDir) {
      return;
    }
    await this.autoSessions.waitForDrive(session);
    if (!this.activePump || this.activePump !== session || session.projectDir !== context.projectDir) {
      return;
    }
    const started = appendUserPrompt(
      this.readConversation(context.projectDir),
      prompt,
      userMeta(context),
      assistantMeta(context),
    );
    session.assistantId = started.assistantId;
    session.pendingPromptEntryId = started.userId;
    session.pendingRequestTokensEstimate = null;
    session.awaitingInput = false;
    await this.persistConversation(context.projectDir, started.state);
    await this.postState(context, started.state);
    await this.autoSessions.resumeWithPrompt(
      session,
      prompt,
      this.autoSessionDelegate(),
    );
  }

  private async onManagedSessionSettled(
    session: ManagedAutoSessionState,
    result: { status: "awaiting-input" | "ended"; endReason?: string; endMessage?: string },
  ): Promise<void> {
    let conversation = this.readConversation(session.projectDir);
    if (session.assistantId) {
      conversation = completeAssistantTurn(
        conversation,
        session.assistantId,
        "No response received.",
      );
      session.assistantId = null;
      session.pendingRequestTokensEstimate = null;
    }

    if (result.status === "awaiting-input") {
      await this.autoSessions.markAwaitingInput(session);
      session.stopRequested = false;
      session.pendingPromptEntryId = null;
      await this.persistConversation(session.projectDir, conversation);
      await this.postState(await this.readPanelContextForProject(session.projectDir), conversation);
      return;
    }

    session.awaitingInput = false;
    session.stopRequested = false;
    session.pendingPromptEntryId = null;
    session.pendingRequestTokensEstimate = null;
    session.currentTool = null;
    session.currentArtifact = null;
    if (result.endReason === "cancelled") {
      conversation = appendNote(
        conversation,
        "Session stopped",
        "Stopped the running sim-flow session.",
      );
    } else if (result.endMessage && result.endMessage.trim().length > 0) {
      conversation = appendNote(
        conversation,
        "Session ended",
        result.endMessage,
      );
    }
    await this.autoSessions.clearIfActive(session);
    await this.persistConversation(session.projectDir, conversation);
    await this.postState(await this.readPanelContextForProject(session.projectDir), conversation);
  }

  private autoSessionDelegate(): AutoSessionDriveDelegate {
    return {
      markdown: (session, text) => {
        this.appendPumpMarkdown(session, text);
      },
      requestTokensEstimate: (session, tokens) => {
        this.recordPumpRequestTokensEstimate(session, tokens);
      },
      settled: async (session, result) => {
        await this.onManagedSessionSettled(session, result);
      },
    };
  }

  private appendPumpMarkdown(
    session: ManagedAutoSessionState,
    text: string,
  ): void {
    if (!this.activePump || this.activePump !== session || text.length === 0) {
      return;
    }
    let conversation = this.readConversation(session.projectDir);
    const classified = classifyPumpMarkdown(text);
    if (classified.kind === "ignore") {
      return;
    }
    if (classified.kind === "phase-sequence") {
      session.currentPhase = classified.currentPhase;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "phase-changed") {
      session.currentPhase = classified.currentPhase;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "tool-activity") {
      session.currentTool = classified.summary;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "artifact-activity") {
      session.currentArtifact = classified.summary;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "note") {
      conversation = appendNote(
        conversation,
        classified.title,
        classified.body,
        classified.tone,
      );
      this.rememberConversation(session.projectDir, conversation);
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (!session.assistantId) {
      const started = appendAssistantPlaceholder(
        conversation,
        "sim-flow",
        "orchestrator",
        session.pendingRequestTokensEstimate ?? undefined,
      );
      session.assistantId = started.assistantId;
      session.pendingRequestTokensEstimate = null;
      conversation = started.state;
    }
    conversation = appendAssistantChunk(
      conversation,
      session.assistantId,
      classified.text,
    );
    this.rememberConversation(session.projectDir, conversation);
    void this.postStateForProject(session.projectDir, conversation);
  }

  private recordPumpRequestTokensEstimate(
    session: ManagedAutoSessionState,
    tokens: number,
  ): void {
    if (!this.activePump || this.activePump !== session) {
      return;
    }
    let conversation = this.readConversation(session.projectDir);
    if (session.pendingPromptEntryId) {
      conversation = setEntryRequestTokensEstimate(
        conversation,
        session.pendingPromptEntryId,
        tokens,
      );
      session.pendingPromptEntryId = null;
    } else if (session.assistantId) {
      conversation = setEntryRequestTokensEstimate(
        conversation,
        session.assistantId,
        tokens,
      );
    } else {
      session.pendingRequestTokensEstimate = tokens;
      return;
    }
    this.rememberConversation(session.projectDir, conversation);
    void this.postStateForProject(session.projectDir, conversation);
  }

  private async readPanelContextForProject(projectDir: string): Promise<PanelContext> {
    const base = await this.readPanelContext();
    if (base.projectDir === projectDir) {
      return base;
    }
    const currentStep = await readCurrentStepSafe(projectDir);
    return {
      ...base,
      projectDir,
      projectLabel: path.basename(projectDir),
      currentStep,
      ...describePanelSession(projectDir, currentStep, base.sourceLabel, this.activePump),
    };
  }

  private async postStateForProject(
    projectDir: string,
    conversation: ChatConversationState,
  ): Promise<void> {
    await this.postState(await this.readPanelContextForProject(projectDir), conversation);
  }

  private async restoreActiveAutoSessionIfNeeded(): Promise<void> {
    if (this.activePump || this.pendingAutoLaunch) {
      return;
    }
    const projectDir = await resolveProjectDirForPanel();
    if (!projectDir) {
      return;
    }
    const record = this.autoSessions.readStoredRecord(projectDir);
    if (!record || isTerminalLlmSource(record.sourceTag)) {
      return;
    }
    const ctx = await resolveContext({
      projectDir,
      showErrors: false,
    });
    if (!ctx) {
      await this.autoSessions.forgetStoredRecord(projectDir);
      return;
    }
    const priorConversation = this.readConversation(projectDir);
    const pump = new SocketSessionPump(
      {
        sessionId: record.sessionId,
        socketPath: record.socketPath,
      },
      buildReconnectableLlmConfig(ctx, this.secrets, vscode.workspace.getConfiguration("sim-flow"), record),
    );
    try {
      await pump.ready();
    } catch {
      const conversation = appendNote(
        priorConversation,
        "Session no longer live",
        "The previous sim-flow session could not be reattached. Relaunch it from the dashboard to continue.",
      );
      await this.autoSessions.forgetStoredRecord(projectDir);
      await this.persistConversation(projectDir, conversation);
      return;
    }
    await this.persistConversation(projectDir, clearConversationState());
    await this.autoSessions.attach(
      record,
      pump,
      this.autoSessionDelegate(),
    );
  }

  private async reconcileModeSwitches(): Promise<void> {
    if (this.reconcilePromise) {
      await this.reconcilePromise;
      return;
    }
    const reconcile = this.reconcileModeSwitchesInner().finally(() => {
      if (this.reconcilePromise === reconcile) {
        this.reconcilePromise = undefined;
      }
    });
    this.reconcilePromise = reconcile;
    await reconcile;
  }

  private async reconcileModeSwitchesInner(): Promise<void> {
    const requestedProjectDir = await resolveProjectDirForPanel();
    const settings = readPanelSettings();

    if (
      this.inFlight &&
      requestedProjectDir !== this.inFlight.projectDir
    ) {
      await this.stopDirectResponse(
        this.inFlight,
        "Project switched",
        `Stopped the current response because the active project changed${requestedProjectDir ? ` to \`${path.basename(requestedProjectDir)}\`` : ""}.`,
      );
    } else if (
      this.inFlight &&
      requestedProjectDir === this.inFlight.projectDir &&
      (this.inFlight.sourceTag !== settings.source || this.inFlight.model !== settings.model)
    ) {
      await this.stopDirectResponse(
        this.inFlight,
        "LLM source switched",
        `Stopped the current response because the LLM source changed to \`${settings.sourceLabel}\`.`,
      );
    }

    if (
      this.pendingAutoLaunch &&
      requestedProjectDir === this.pendingAutoLaunch.projectDir &&
      this.pendingAutoLaunch.sourceTag === settings.source &&
      this.pendingAutoLaunch.model === settings.model
    ) {
      return;
    }

    if (
      this.activePump &&
      requestedProjectDir !== this.activePump.projectDir
    ) {
      await this.stopActivePumpSession(
        this.activePump,
        "Project switched",
        `Stopped the running sim-flow session because the active project changed${requestedProjectDir ? ` to \`${path.basename(requestedProjectDir)}\`` : ""}.`,
      );
      return;
    }

    if (
      this.activePump &&
      requestedProjectDir === this.activePump.projectDir &&
      (this.activePump.sourceTag !== settings.source || this.activePump.model !== settings.model)
    ) {
      const relaunch = {
        projectDir: this.activePump.projectDir,
        sessionMode: this.activePump.sessionMode,
        stepRef: this.activePump.stepRef,
        launchSpecPath: this.activePump.launchSpecPath,
      };
      await this.stopActivePumpSession(
        this.activePump,
        "LLM source switched",
        `Stopped the running sim-flow session because the LLM source changed to \`${settings.sourceLabel}\`. Relaunching on the new source.`,
      );
      if (isTerminalLlmSource(settings.source)) {
        const terminalRelaunchBody =
          relaunch.sessionMode === "step" && relaunch.stepRef
            ? `Relaunched sim-flow session \`${relaunch.stepRef.step}.${relaunch.stepRef.kind}\` in the terminal on the new source \`${settings.sourceLabel}\`.`
            : `Relaunched sim-flow auto in the terminal on the new source \`${settings.sourceLabel}\`.`;
        const conversation = appendNote(
          this.readConversation(relaunch.projectDir),
          "LLM source switched",
          terminalRelaunchBody,
        );
        await this.persistConversation(relaunch.projectDir, conversation);
        if (relaunch.sessionMode === "step" && relaunch.stepRef) {
          await vscode.commands.executeCommand(
            relaunch.stepRef.kind === "work" ? "sim-flow.runStep" : "sim-flow.runCritique",
            relaunch.stepRef.step,
            relaunch.projectDir,
          );
        } else {
          await vscode.commands.executeCommand(
            "sim-flow.runFlowTerminal",
            cliBackendArgFor(settings.source),
            relaunch.launchSpecPath ?? "",
            relaunch.projectDir,
          );
        }
        return;
      }
      const ctx = await resolveContext({
        projectDir: relaunch.projectDir,
        showErrors: true,
      });
      if (!ctx) {
        return;
      }
      if (relaunch.sessionMode === "step" && relaunch.stepRef) {
        await this.startStepSession(ctx, relaunch.stepRef, {
          resetConversation: false,
          launchTitle: "LLM source switched",
          launchBody: `Relaunched sim-flow session \`${relaunch.stepRef.step}.${relaunch.stepRef.kind}\` on the new source \`${settings.sourceLabel}\`.`,
        });
        return;
      }
      await this.startAutoSession(
        ctx,
        relaunch.launchSpecPath,
        {
          resetConversation: false,
          launchTitle: "LLM source switched",
          launchBody: `Relaunched sim-flow auto on the new source \`${settings.sourceLabel}\`.`,
        },
      );
    }
  }

  private async stopDirectResponse(
    inFlight: DirectResponseState,
    title: string,
    body: string,
  ): Promise<void> {
    inFlight.source.cancel();
    this.inFlight = undefined;
    const conversation = appendNote(
      this.readConversation(inFlight.projectDir),
      title,
      body,
    );
    await this.persistConversation(inFlight.projectDir, conversation);
  }

  private async stopActivePumpSession(
    session: ManagedAutoSessionState,
    title: string,
    body: string,
  ): Promise<void> {
    await this.autoSessions.clearIfActive(session);
    session.awaitingInput = false;
    session.stopRequested = true;
    session.pump.cancel();
    const conversation = appendNote(
      this.readConversation(session.projectDir),
      title,
      body,
    );
    await this.persistConversation(session.projectDir, conversation);
  }

  private async enqueuePost(task: () => Promise<void>): Promise<void> {
    const next = this.postChain.catch(() => undefined).then(async () => {
      if (this.disposed || !this.view) {
        return;
      }
      await task();
    });
    this.postChain = next.catch(() => undefined);
    await next;
  }

  private renderHtml(webview: vscode.Webview): string {
    const nonce = randomNonce();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "dist", "webview", "chatPanel", "panel.js"),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "chat-panel.css"),
    );
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} data:`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
      `font-src ${webview.cspSource}`,
    ].join("; ");

    return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="stylesheet" href="${styleUri}" />
    <title>sim-flow Chat</title>
  </head>
  <body>
    <main id="app"></main>
    <script nonce="${nonce}" src="${scriptUri}"></script>
  </body>
</html>`;
  }
}

interface PanelContext {
  projectLabel: string;
  projectDir: string | null;
  currentStep: string | null;
  source: LlmSourceTag;
  sourceLabel: string;
  model: string;
  verbose: boolean;
  ollamaBaseUrl: string;
  lmstudioBaseUrl: string;
  sessionLabel: string;
  statusLine: string;
}

function describePanelSession(
  projectDir: string | null,
  currentStep: string | null,
  sourceLabel: string,
  activeSession: ManagedAutoSessionState | undefined,
): { sessionLabel: string; statusLine: string } {
  if (
    projectDir &&
    activeSession &&
    activeSession.projectDir === projectDir &&
    activeSession.sessionMode === "step" &&
    activeSession.stepRef
  ) {
    const { step, kind } = activeSession.stepRef;
    return {
      sessionLabel: `${step}.${kind}`,
      statusLine: `Step ${step} ${kind} session with ${sourceLabel}.`,
    };
  }
  return {
    sessionLabel: currentStep ? `${currentStep}.work` : "General chat",
    statusLine: currentStep
      ? `Chat with ${sourceLabel} while working on ${currentStep}.`
      : `Direct chat panel backed by ${sourceLabel}.`,
  };
}

function readPanelSettings(): {
  source: LlmSourceTag;
  sourceLabel: string;
  model: string;
  verbose: boolean;
  ollamaBaseUrl: string;
  lmstudioBaseUrl: string;
} {
  const config = vscode.workspace.getConfiguration("sim-flow");
  const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
  return {
    source,
    sourceLabel: LLM_SOURCE_LABELS[source] ?? source,
    model: (config.get<string>("llm.model") ?? "").trim(),
    verbose: config.get<boolean>("llm.verbose") ?? true,
    ollamaBaseUrl: (config.get<string>("llm.ollama.baseUrl") ?? "").trim(),
    lmstudioBaseUrl: (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim(),
  };
}

function normalizeSpecPath(specPath: string | undefined): string | undefined {
  const trimmed = specPath?.trim() ?? "";
  return trimmed.length > 0 ? trimmed : undefined;
}

function queueConversationWrite(task: () => Promise<void>): Promise<void> {
  const write = pendingConversationWrites.catch(() => undefined).then(task);
  pendingConversationWrites = write.catch(() => undefined);
  return write;
}

async function waitForPendingConversationWrites(): Promise<void> {
  await pendingConversationWrites;
}

async function resolveProjectDirForPanel(): Promise<string | null> {
  const direct = resolveProjectDir();
  if (direct) {
    return direct;
  }
  const candidates = await findProjectCandidates();
  return candidates[0] ?? null;
}

async function readCurrentStepSafe(projectDir: string): Promise<string | null> {
  try {
    const state = await readFlowState(projectDir);
    return state.current_step;
  } catch {
    return null;
  }
}

function buildNotice(context: PanelContext, isStreaming: boolean): string {
  if (isStreaming) {
    return `${context.sourceLabel} is responding. The transcript updates in place as chunks arrive.`;
  }
  if (isTerminalLlmSource(context.source)) {
    return 'This panel does not drive terminal-only backends. Switch `sim-flow.llm.source` to `lmstudio` or another API backend to chat here.';
  }
  if (context.source === "lmstudio") {
    const modelDetail = context.model.length > 0 ? `model \`${context.model}\`` : "the currently loaded model";
    const baseUrl = context.lmstudioBaseUrl || "http://localhost:1234/v1";
    return `LM Studio chat is ready at \`${baseUrl}\`, using ${modelDetail}.`;
  }
  if (context.source === "ollama") {
    const baseUrl = context.ollamaBaseUrl || "http://localhost:11434/v1";
    return `Ollama chat is ready at \`${baseUrl}\`.`;
  }
  return `${context.sourceLabel} chat is ready. Enter a prompt below to start the conversation.`;
}

function hasInterruptedAutoSessionTranscript(
  transcript: ChatConversationState["transcript"],
): boolean {
  return transcript.some(
    (entry) =>
      entry.kind === "note" &&
      entry.body.includes(
        "Stopped the running sim-flow session because the chat panel was reloaded or closed.",
      ),
  );
}

type PumpChunk =
  | { kind: "assistant"; text: string }
  | { kind: "note"; title: string; body: string; tone: "info" | "error" }
  | { kind: "phase-sequence"; currentPhase: string | null }
  | { kind: "phase-changed"; currentPhase: string }
  | { kind: "tool-activity"; summary: string }
  | { kind: "artifact-activity"; summary: string }
  | { kind: "ignore" };

function classifyPumpMarkdown(text: string): PumpChunk {
  const trimmed = text.trim();
  const lines = trimmed.split("\n").map((line) => line.trim()).filter((line) => line.length > 0);
  if (trimmed.length === 0) {
    return { kind: "ignore" };
  }
  if (
    trimmed === "<details>" ||
    trimmed === "</details>" ||
    trimmed.startsWith("<summary>")
  ) {
    return { kind: "ignore" };
  }
  if (lines.length === 1 && trimmed.startsWith("**Step `")) {
    return noteChunk("Session started", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("_Phases:_")) {
    return {
      kind: "phase-sequence",
      currentPhase: firstPhaseFromSequence(trimmed),
    };
  }
  if (lines.length === 1 && trimmed.startsWith("_Tool `") && trimmed.endsWith("._")) {
    return {
      kind: "tool-activity",
      summary: toolSummary(trimmed),
    };
  }
  if (lines.length === 1 && trimmed.startsWith("_Wrote `") && trimmed.endsWith("._")) {
    return {
      kind: "artifact-activity",
      summary: artifactSummary(trimmed),
    };
  }
  if (lines.length === 1 && trimmed.startsWith("**Phase:**")) {
    const currentPhase = phaseFromPhaseChanged(trimmed);
    return currentPhase
      ? { kind: "phase-changed", currentPhase }
      : { kind: "ignore" };
  }
  if (lines.length === 1 && /^\*\*`.+`\*\* exited with status /.test(trimmed)) {
    return noteChunk("Build output", trimmed);
  }
  if (
    trimmed.startsWith("**Gate `") &&
    lines.slice(1).every((line) => line.startsWith("- "))
  ) {
    return noteChunk("Gate result", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("**Advanced past `")) {
    return noteChunk("State advanced", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("_Suggested next:")) {
    return noteChunk("Suggested next", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("_LLM source switched:")) {
    return noteChunk("LLM source switched", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("**Error**:")) {
    return noteChunk("Session error", trimmed, "error");
  }
  if (lines.length === 1 && trimmed.startsWith("**Warning**:")) {
    return noteChunk("Session warning", trimmed);
  }
  if (lines.length === 1 && trimmed.startsWith("**Info**:")) {
    return noteChunk("Session info", trimmed);
  }
  const visible = stripToolCallFencesForStreaming(text);
  if (visible.length === 0) {
    return { kind: "ignore" };
  }
  return { kind: "assistant", text: visible };
}

function noteChunk(
  title: string,
  body: string,
  tone: "info" | "error" = "info",
): PumpChunk {
  return { kind: "note", title, body, tone };
}

function firstPhaseFromSequence(text: string): string | null {
  const matches = Array.from(text.matchAll(/`([^`]+)`/g));
  return matches.length > 0 ? matches[0]?.[1] ?? null : null;
}

function phaseFromPhaseChanged(text: string): string | null {
  const match = /\*\*Phase:\*\*\s*`([^`]+)`/.exec(text);
  return match?.[1] ?? null;
}

function toolSummary(text: string): string {
  const match = /^_Tool `([^`]+)`(?: \(([^)]+)\))? -> ([^ ]+) \((\d+) ms\)\._$/.exec(text);
  if (!match) {
    return text.replace(/^_+|_+$/g, "");
  }
  const [, name, argsSummary, status, durationMs] = match;
  const detail = argsSummary ? ` ${argsSummary}` : "";
  return `${name}${detail} -> ${status} (${durationMs} ms)`;
}

function artifactSummary(text: string): string {
  const match = /^_Wrote `([^`]+)` \((\d+) bytes\)\._$/.exec(text);
  if (!match) {
    return text.replace(/^_+|_+$/g, "");
  }
  const [, artifactPath, bytes] = match;
  return `${artifactPath} (${bytes} bytes)`;
}

function conversationStorageKey(projectDir: string | null): string {
  return `sim-flow.chatPanel.conversation.${projectDir ?? "__workspace__"}`;
}

function userMeta(context: PanelContext): string | undefined {
  if (context.currentStep) {
    return `${context.projectLabel} • ${context.currentStep}`;
  }
  if (context.projectLabel.length > 0) {
    return context.projectLabel;
  }
  return undefined;
}

function assistantMeta(context: PanelContext): string {
  return context.model.length > 0
    ? `${context.sourceLabel} • ${context.model}`
    : context.sourceLabel;
}

function formatChatError(error: unknown): string {
  const baseMessage = error instanceof Error ? error.message : String(error);
  if (error instanceof LlmError && error.detail && error.detail.length > 0) {
    return `${baseMessage}\n\n${error.detail.slice(0, 512)}`;
  }
  return baseMessage;
}

function buildPumpLlmConfig(
  ctx: { projectDir: string; cli: { binary: string } },
  secrets: SecretStorage,
  config: vscode.WorkspaceConfiguration,
): PumpLlmConfig {
  const source = (config.get<LlmSource>("llm.source") ?? "vscode") as LlmSource;
  const model = (config.get<string>("llm.model") ?? "").trim() || undefined;
  return buildResolvedPumpLlmConfig(ctx, secrets, config, source, model);
}

function buildReconnectableLlmConfig(
  ctx: { projectDir: string; cli: { binary: string } },
  secrets: SecretStorage,
  config: vscode.WorkspaceConfiguration,
  record: StoredAutoSessionRecord,
): PumpLlmConfig {
  return buildResolvedPumpLlmConfig(
    ctx,
    secrets,
    config,
    record.sourceTag as LlmSource,
    record.model.trim() || undefined,
  );
}

function buildResolvedPumpLlmConfig(
  ctx: { projectDir: string; cli: { binary: string } },
  secrets: SecretStorage,
  config: vscode.WorkspaceConfiguration,
  source: LlmSource,
  model: string | undefined,
): PumpLlmConfig {
  const ollamaBaseUrl = (config.get<string>("llm.ollama.baseUrl") ?? "").trim() || undefined;
  const lmstudioBaseUrl =
    (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim() || undefined;
  const settingTokens = (config.get<string[]>("debug") ?? []).join(",");
  const envTokens = (process.env["SIM_FOUNDATION_DEBUG"] ?? "").trim();
  const debugTokens = settingTokens.length > 0 ? settingTokens : envTokens;
  return {
    source,
    model,
    ollamaBaseUrl,
    lmstudioBaseUrl,
    secrets,
    projectDir: ctx.projectDir,
    binary: ctx.cli.binary,
    debugTokens,
  };
}

function reconnectableSocketPath(sessionId: string): string {
  return path.join(os.tmpdir(), `sim-flow-${sessionId}.sock`);
}

function randomNonce(): string {
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let out = "";
  for (let i = 0; i < 16; i += 1) {
    out += alphabet[Math.floor(Math.random() * alphabet.length)];
  }
  return out;
}
