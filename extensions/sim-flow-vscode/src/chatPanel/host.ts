import * as path from "node:path";
import { randomUUID } from "node:crypto";
import * as os from "node:os";

import * as vscode from "vscode";

import {
  findProjectCandidates,
  PICK_PROJECT_NEW,
  pickProject,
  resolveContext,
  resolveProjectDir,
} from "../context";
import type { LlmSource, SecretStorage } from "../llm";
import { queryContextWindow } from "../llm/contextWindow";
import { type PumpLlmConfig } from "../session/pump";
import { clearStalePumpLockForSession } from "../session/pumpLock";
import { SocketSessionPump } from "../session/socketPump";
import {
  SimFlowCli,
  bundledCandidates,
  resolveBinary,
} from "../cli";
import { readCritique } from "../state/critiques";
import { readFlowState } from "../state/flowState";
import { readPlanProgress } from "../state/planProgress";
import {
  COVERAGE_DEFAULTS,
  LLM_DEFAULTS,
  writeCoverageSettings,
  writeLlmSettings,
} from "../state/projectConfig";
import { stepOrderFor, stepsFromOnward } from "../state/stepOrder";
import type { FlowState } from "../state/types";
import {
  cliBackendArgFor,
  isTerminalLlmSource,
  type LlmServerEntry,
  LLM_SOURCE_LABELS,
  type LlmSourceTag,
  resolveLlmSource,
} from "../webview/messages";

import {
  type ChatCustomPalette,
  type ChatPalette,
  type ChatPanelState,
  CHAT_PALETTE_NAMES,
  DEFAULT_CUSTOM_PALETTE,
  type HostMessage,
  type WebviewMessage,
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
  appendAssistantReasoningChunk,
  appendNote,
  appendOrchestratorUserEntry,
  appendUserPrompt,
  clearConversationState,
  completeAssistantReasoning,
  completeAssistantTurn,
  createConversationState,
  filterPresentationEntries,
  setEntryRequestTokensEstimate,
  stripToolCallFencesForStreaming,
  summarizeTokenEstimates,
  toStoredConversation,
  type ChatConversationState,
} from "./state";

export const CHAT_PANEL_VIEW_ID = "simFlow.chatPanel";
export const CHAT_PANEL_CONTAINER_ID = "sim-flow-chat-panel";

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
  private pendingAutoLaunch: PendingAutoLaunchState | undefined;
  private disposed = false;
  private refreshing = false;
  private refreshQueued = false;
  private reconcilePromise: Promise<void> | undefined;
  private postChain: Promise<void> = Promise.resolve();
  /**
   * Per-provider serialized chain of conversation-state writes.
   * Previously module-scope; multiple ChatPanelProvider instances
   * (the integration-test harness mostly, but also any future plugin-
   * host scenario that constructs more than one provider) shared the
   * same promise and waitForPendingConversationWrites() therefore
   * blocked on unrelated writes from another panel. Per-instance
   * here so writes for one panel don't fence writes for another.
   * See chat-panel audit #12 (2026-05-16).
   */
  private pendingConversationWrites: Promise<void> = Promise.resolve();
  /**
   * Opening / revealing the panel also calls `refresh()`, but that is
   * not itself a project switch. We only want the expensive and
   * destructive "stop the old session because the active project
   * changed" path to run after an actual editor/workspace context
   * change signalled by VS Code.
   */
  private projectSwitchPending = false;

  /**
   * Project the host has just decided to auto-relaunch (because the
   * previously-stored auto-session record was dead -- the
   * orchestrator child got killed by Developer: Reload Window). Set
   * synchronously inside `restoreActiveAutoSessionIfNeeded`'s catch
   * BEFORE the fire-and-forget `launchAutoSession` so this refresh's
   * `postState` anchors the panel to the right project and renders
   * the "Launching…" indicator instead of flashing the empty-state
   * "Start session" button. Cleared when the launch's
   * `pendingAutoLaunch` takes over (its first awaits resolve and it
   * sets its own anchor) or when the launch fails entirely.
   */
  private pendingRelaunchAnchor: string | null = null;

  private get activePump(): ManagedAutoSessionState | undefined {
    return this.autoSessions.getActiveSession();
  }

  /**
   * Subscription to the active pump's `onSubSessionChanged`. The chat
   * panel needs to react to a NEW sub-session opening under it: that
   * happens when the dashboard's Run Step / Run Critique / Run Gate /
   * Advance click goes through `AutoHost`'s cancel-and-dispatch path
   * (ends the parked critique, opens a fresh work bracket) WITHOUT
   * the user typing anything in the chat panel. The previous drive
   * cycle resolved with `awaiting-input`, so `currentRenderer` is
   * `null` and the pump silently queues the new sub-session's
   * `request-llm-response`. We re-attach the drive here the moment
   * the bracket flips back to busy.
   */
  private subSessionListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onRequestUserInput`. When the
   * orchestrator parks a sub-session asking for human guidance, this
   * carries the prompt + placeholder text to the chat panel so it
   * can render the question above the composer. Without this the
   * user only sees "Waiting on user to select the next step" with
   * no indication of *what* the orchestrator is actually asking.
   */
  private requestUserInputListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onFollowup`. Followup events
   * carry the label + action for clickable quick-replies the
   * orchestrator suggested (`/retry`, `/end-session`, course-
   * correction). Without this, the chips never appear and the
   * actions fall back to "user must read the suggestion and type
   * the literal command."
   */
  private followupListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onStepModeChanged`. The
   * orchestrator echoes its current StepMode (auto/manual) on every
   * transition, including the initial echo of the launch-time
   * `--step-mode` flag. The chat panel uses this to keep the
   * toolbar toggle visually in sync with what the orchestrator is
   * actually doing.
   */
  private stepModeListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onNextActionHint`. Each
   * manual-mode park, the orchestrator emits a hint describing what
   * `ContinueFlow` would do next; the chat panel uses the label
   * verbatim on its Continue button.
   */
  private nextActionHintListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onContextEvicted`. Each event
   * carries the ids the orchestrator just evicted from its prompt
   * stack + the reason; we stash the (id -> reason) entries on the
   * session so `ChatPanelState.evictedMessages` can mark matching
   * bubbles when `showContextState` is on.
   */
  private contextEvictedListenerDispose: (() => void) | null = null;
  /**
   * Subscription to the active pump's `onStateAdvanced`. Fires when
   * the orchestrator's `current_step` moves (forward via Advance,
   * backward via Reset). We trigger `refresh()` so the next
   * `readPanelContext` re-reads `state.toml` and the step rail
   * repaints. Without this, a context-menu Reset leaves the rail
   * pinned to the pre-reset step until some other event happens to
   * trigger a refresh.
   */
  private stateAdvancedListenerDispose: (() => void) | null = null;

  /**
   * workspaceState key for the most recently launched project dir.
   * `startSession` consults this so a user who closes and reopens
   * VS Code can resume the same project with a single click on
   * "Start session" -- no picker prompt unless the remembered
   * dir is no longer in the candidate set. The value is replaced
   * (not appended to) on every launch.
   */
  private static readonly LAST_PROJECT_KEY = "sim-flow.chatPanel.lastProjectDir";
  private static readonly PALETTE_KEY = "sim-flow.chatPanel.palette";
  private static readonly CUSTOM_PALETTE_KEY = "sim-flow.chatPanel.customPalette";

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly workspaceState: vscode.Memento,
    private readonly secrets: SecretStorage,
    private readonly autoSessions: AutoSessionManager = new AutoSessionManager(workspaceState),
  ) {
    this.disposables.push(
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("sim-flow.dashboard.experimentalUi")) {
          // Swap the webview HTML so the experimental / standard
          // chat assets are reloaded. The full state-update fires
          // afterwards via refresh().
          if (this.view) {
            this.view.webview.html = this.renderHtml(this.view.webview);
          }
          void this.refresh();
          return;
        }
        const llmSettingChanged =
          event.affectsConfiguration("sim-flow.llm.source") ||
          event.affectsConfiguration("sim-flow.llm.model") ||
          event.affectsConfiguration("sim-flow.llm.modelFamily") ||
          event.affectsConfiguration("sim-flow.llm.runtimeProfile") ||
          event.affectsConfiguration("sim-flow.llm.servers") ||
          event.affectsConfiguration("sim-flow.llm.verbose") ||
          event.affectsConfiguration("sim-flow.llm.ollama.baseUrl") ||
          event.affectsConfiguration("sim-flow.llm.lmstudio.baseUrl");
        if (llmSettingChanged) {
          void this.refresh();
          // The orchestrator child was spawned with the old CLI argv
          // and stays bound to it -- changing the setting in the
          // dashboard won't reach the live process. Prompt the user
          // to reconnect so a fresh orchestrator picks up the new
          // value. No-op when nothing's connected.
          void this.promptReconnectIfLive(
            "LLM settings changed.",
          );
        }
        if (
          event.affectsConfiguration("sim-flow.coverage.thresholdPct") ||
          event.affectsConfiguration("sim-flow.coverage.level")
        ) {
          void this.pushCoverageSettingToActiveProject();
        }
        if (event.affectsConfiguration("sim-flow.llm.maxParallelRequests")) {
          void this.pushLlmSettingToActiveProject();
        }
        if (event.affectsConfiguration("sim-flow.verilog.enabled")) {
          // The chat panel's SV rail + DM4b -> SV0 transition both
          // depend on this flag; a refresh repaints with the new
          // value without waiting for the next state-update.
          void this.refresh();
        }
        if (
          event.affectsConfiguration("sim-flow.chatPanel.showContextState")
        ) {
          // Toggling the indicator should flip immediately, not at
          // the next file-watcher tick.
          void this.refresh();
        }
      }),
      vscode.window.onDidChangeActiveTextEditor(() => {
        this.projectSwitchPending = true;
        void this.refresh();
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        this.projectSwitchPending = true;
        void this.refresh();
      }),
    );
    // Re-subscribe to the active pump's bracket transitions whenever
    // the active session rotates (Connect / Disconnect / step
    // session swap). The inner attach* helpers manage their own
    // disposers (each replaces the previous pump's listener so it
    // doesn't outlive its session). The OUTER onActiveSessionChanged
    // subscription also returns a disposer, which used to be
    // discarded -- after ChatPanelProvider.dispose() ran, this
    // closure would keep firing on every active-session change,
    // calling attachSubSessionListener etc. on a disposed provider
    // and pinning it via the closure. Capture the disposer in
    // this.disposables so dispose() tears it down too. See
    // chat-panel audit #2 (2026-05-16).
    const activeSessionDisposer = this.autoSessions.onActiveSessionChanged(
      (session) => {
        this.attachSubSessionListener(session);
        this.attachRequestUserInputListener(session);
        this.attachFollowupListener(session);
        this.attachStepModeListener(session);
        this.attachNextActionHintListener(session);
        this.attachContextEvictedListener(session);
        this.attachStateAdvancedListener(session);
        // A newly attached/replaced session changes the panel's anchor
        // immediately; refresh so OFFLINE flips to VIEWING/STREAMING
        // without waiting for the next pump event. We intentionally do
        // NOT auto-refresh on clear: the settle/stop paths already post
        // an explicit final state for the session's project, and a
        // follow-up refresh here can snap the panel back to whichever
        // project the editor currently points at.
        if (session) {
          void this.refresh();
        }
      },
    );
    this.disposables.push({ dispose: () => activeSessionDisposer() });
  }

  private attachStepModeListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.stepModeListenerDispose) {
      this.stepModeListenerDispose();
      this.stepModeListenerDispose = null;
    }
    if (!session || typeof session.pump.onStepModeChanged !== "function") {
      return;
    }
    this.stepModeListenerDispose = session.pump.onStepModeChanged(() => {
      // The mode value lives on the pump itself; refresh recomputes
      // ChatPanelState which reads it. Coalescing-via-refresh is
      // fine because StepModeChanged is low-frequency (echoed on
      // launch + once per user toggle).
      void this.refresh();
    });
  }

  private attachFollowupListener(session: ManagedAutoSessionState | undefined): void {
    if (this.followupListenerDispose) {
      this.followupListenerDispose();
      this.followupListenerDispose = null;
    }
    if (!session || typeof session.pump.onFollowup !== "function") {
      return;
    }
    this.followupListenerDispose = session.pump.onFollowup(({ label, action }) => {
      const current = this.activePump;
      if (!current) {
        return;
      }
      this.autoSessions.appendFollowup(current, { label, action });
      // Repaint so the new chip surfaces immediately. Followups
      // typically arrive in clusters just before a
      // `request-user-input`; coalescing the refreshes would
      // delay all but the last chip by one tick.
      void this.refresh();
    });
  }

  private attachNextActionHintListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.nextActionHintListenerDispose) {
      this.nextActionHintListenerDispose();
      this.nextActionHintListenerDispose = null;
    }
    if (!session || typeof session.pump.onNextActionHint !== "function") {
      return;
    }
    this.nextActionHintListenerDispose = session.pump.onNextActionHint(
      ({ label }) => {
        const current = this.activePump;
        if (!current) {
          return;
        }
        this.autoSessions.setNextActionHint(current, label);
        // Repaint so the Continue button label updates before the
        // user clicks. Arrives right before each `request-user-input`
        // in manual mode.
        void this.refresh();
      },
    );
  }

  private attachContextEvictedListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.contextEvictedListenerDispose) {
      this.contextEvictedListenerDispose();
      this.contextEvictedListenerDispose = null;
    }
    if (!session || typeof session.pump.onContextEvicted !== "function") {
      return;
    }
    this.contextEvictedListenerDispose = session.pump.onContextEvicted(
      ({ ids, reason }) => {
        const current = this.activePump;
        if (!current) {
          return;
        }
        for (const id of ids) {
          current.evictedMessageIds.set(id, reason);
        }
        // Repaint so the ✗ + tooltip appears immediately. The chat
        // panel toggle (`showContextState`) gates whether the user
        // actually sees them; the host always tracks evictions.
        void this.refresh();
      },
    );
  }

  private attachStateAdvancedListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.stateAdvancedListenerDispose) {
      this.stateAdvancedListenerDispose();
      this.stateAdvancedListenerDispose = null;
    }
    if (!session || typeof session.pump.onStateAdvanced !== "function") {
      return;
    }
    this.stateAdvancedListenerDispose = session.pump.onStateAdvanced(() => {
      // `current_step` moved on disk (forward Advance or backward
      // Reset). Refresh so `readPanelContext` -> `readFlowStateSafe`
      // re-reads `state.toml` and the step rail repaints. Without
      // this, the rail stays on the pre-move step until some
      // unrelated event happens to trigger a refresh -- which the
      // user noticed after a context-menu Reset: the popup
      // confirmed, the orchestrator did the work, but the rail
      // didn't follow.
      if (!this.activePump) {
        return;
      }
      void this.refresh();
    });
  }

  private attachRequestUserInputListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.requestUserInputListenerDispose) {
      this.requestUserInputListenerDispose();
      this.requestUserInputListenerDispose = null;
    }
    if (!session || typeof session.pump.onRequestUserInput !== "function") {
      return;
    }
    this.requestUserInputListenerDispose = session.pump.onRequestUserInput(
      ({ prompt, placeholder }) => {
        const current = this.activePump;
        if (!current) {
          return;
        }
        this.autoSessions.setPendingPrompt(current, prompt, placeholder);
        // Repaint so the banner appears above the composer the moment
        // the orchestrator parks -- otherwise the user sees the
        // generic notice for one tick before the prompt lands.
        void this.refresh();
      },
    );
  }

  private attachSubSessionListener(
    session: ManagedAutoSessionState | undefined,
  ): void {
    if (this.subSessionListenerDispose) {
      this.subSessionListenerDispose();
      this.subSessionListenerDispose = null;
    }
    if (!session || typeof session.pump.onSubSessionChanged !== "function") {
      return;
    }
    this.subSessionListenerDispose = session.pump.onSubSessionChanged(
      (inSubSession) => {
        // We only care about transitions INTO busy. The pump's
        // effective `inSubSession` flips to true on
        // `sub-session-started` (and on resume from a parked state
        // via the next active-work event). If the chat panel was
        // sitting in `awaiting-input` -- meaning the previous drive
        // resolved and `currentRenderer` is null -- a new sub-session
        // would otherwise queue events and stall the orchestrator.
        if (!inSubSession) {
          return;
        }
        const current = this.activePump;
        if (!current) {
          return;
        }
        // Fresh sub-session: any tool / artifact context from the
        // prior session is stale. Clear before posting state so
        // the indicator doesn't carry "Tool: read_file" from the
        // last critique into the new work session. Same for the
        // parked-prompt context -- if a new sub-session is starting,
        // the orchestrator's earlier "what should I do?" question
        // has been superseded.
        current.currentTool = null;
        current.currentArtifact = null;
        current.currentPrompt = null;
        current.currentPlaceholder = null;
        // Followups belonged to the previous parked state; the
        // new sub-session may or may not produce more.
        current.pendingFollowups = [];
        // Drop the session-level "awaiting input" flag synchronously.
        // This listener is dispatched synchronously from
        // `handleEvent("sub-session-started")`, but the microtask that
        // sets `awaitingInput=true` (via `onManagedSessionSettled` ->
        // `markAwaitingInput`) for the PREVIOUS park can still be
        // pending: when `RequestUserInput` and `SubSessionStarted`
        // arrive in the same wire chunk, settle's `onSettled` listener
        // synchronously resolves the drive promise, but
        // `delegate.settled` only runs as a microtask -- AFTER this
        // listener executes. Without a synchronous clear, that pending
        // microtask sets `awaitingInput=true` while `pump.inSubSession`
        // is also true (the bracket flags were just flipped by the
        // event), and the next postState surfaces the "WAITING ON YOU"
        // pill + Stop button + disabled Play stuck state. The previous
        // `resumeDriveOnly` call could not cover this because its own
        // `if (session.drivePromise) return` early-exit also fires in
        // the microtask-gap window (the `.finally()` that clears
        // `drivePromise` is itself a microtask).
        const wasAwaiting = current.awaitingInput;
        current.awaitingInput = false;
        if (!wasAwaiting) {
          // Drive is already running (e.g. the session never
          // parked); the cleared fields will surface on the next
          // postState. Nothing else to do.
          return;
        }
        void this.autoSessions.resumeDriveOnly(current, this.autoSessionDelegate());
      },
    );
  }

  dispose(): void {
    this.disposed = true;
    this.refreshQueued = false;
    this.view = undefined;
    // Flush any scheduled-but-not-yet-fired conversation persist
    // timers BEFORE we drop disposables -- otherwise an extension-
    // host shutdown that interrupts the 250ms debounce loses
    // whatever chat events arrived in that window.
    this.flushPendingPersistsSync();
    if (this.subSessionListenerDispose) {
      this.subSessionListenerDispose();
      this.subSessionListenerDispose = null;
    }
    if (this.requestUserInputListenerDispose) {
      this.requestUserInputListenerDispose();
      this.requestUserInputListenerDispose = null;
    }
    if (this.followupListenerDispose) {
      this.followupListenerDispose();
      this.followupListenerDispose = null;
    }
    if (this.stepModeListenerDispose) {
      this.stepModeListenerDispose();
      this.stepModeListenerDispose = null;
    }
    if (this.nextActionHintListenerDispose) {
      this.nextActionHintListenerDispose();
      this.nextActionHintListenerDispose = null;
    }
    if (this.contextEvictedListenerDispose) {
      this.contextEvictedListenerDispose();
      this.contextEvictedListenerDispose = null;
    }
    if (this.stateAdvancedListenerDispose) {
      this.stateAdvancedListenerDispose();
      this.stateAdvancedListenerDispose = null;
    }
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
    // `retainContextWhenHidden` is configured on the
    // `registerWebviewViewProvider` call in extension.ts so the
    // panel's DOM + JS state survives an editor tab stealing focus.
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
        await this.refresh();
        // Auto-resume the last project (if we remember one) in
        // Manual mode so the user lands on a familiar conversation
        // ready to be advanced via Continue. No picker fires here
        // -- if there's no remembered project, the empty-state
        // "Start session" button takes over.
        void this.tryAutoResume();
        return;
      case "refresh":
        await this.refresh();
        return;
      case "send-prompt":
        await this.sendPrompt(msg.prompt);
        return;
      case "followup-selected":
        // Quick-action chip click: treat the action string as the
        // user's message (e.g. "/retry"). The literal action is
        // what the orchestrator expects to receive verbatim --
        // sendPrompt routes through the same path a typed message
        // takes, so the orchestrator sees identical input shape.
        await this.sendPrompt(msg.action);
        return;
      case "clear-transcript":
        await this.clearTranscript();
        return;
      case "stop-conversation":
        await this.stopConversation();
        return;
      case "set-step-mode":
        await this.handleSetStepMode(msg.mode);
        return;
      case "pick-file":
        await this.pickFile();
        return;
      case "continue-flow":
        this.continueFlow();
        return;
      case "switch-project":
        await this.switchProject();
        return;
      case "start-session":
        await this.startSession();
        return;
      case "end-session":
        await this.endSession();
        return;
      case "reset-step":
        await this.resetCurrentStep();
        return;
      case "open-dashboard":
        await vscode.commands.executeCommand("sim-flow.openDashboard");
        return;
      case "reset-step-pick":
        await this.resetFromEarlierStep();
        return;
      case "open-critique-popup":
        await this.openCritiquePopup(msg.step);
        return;
      case "reset-from-step":
        await this.resetFromStepId(msg.step);
        return;
      case "open-file":
        await this.openFileInEditor(msg.path);
        return;
      case "set-palette":
        await this.persistPaletteChoice(msg.palette, msg.customPalette);
        return;
      case "set-verilog-enabled":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update(
            "verilog.enabled",
            msg.enabled,
            vscode.ConfigurationTarget.Workspace,
          );
        return;
      case "set-show-context-state":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update(
            "chatPanel.showContextState",
            msg.enabled,
            vscode.ConfigurationTarget.Workspace,
          );
        return;
      case "convert-to-sv":
        await this.convertActiveProjectToSv();
        return;
      default:
        return;
    }
  }

  /**
   * Read the latest critique file for `step` and post a
   * `critique-data` HostMessage back to the webview so its popup can
   * render. Lazy: the file is only read on click, not preloaded into
   * `ChatPanelState`. Errors and missing files resolve to `data: null`
   * (popup shows an empty state).
   */
  private async openCritiquePopup(step: string): Promise<void> {
    if (!this.view || this.disposed) {
      return;
    }
    const projectDir = this.activePump?.projectDir ?? this.anchoredProjectDir();
    let data: { findings: import("../state/types").Finding[]; hasBlocking: boolean } | null = null;
    if (projectDir) {
      try {
        const file = await readCritique(projectDir, step);
        if (file) {
          data = { findings: file.findings, hasBlocking: file.hasBlocking };
        }
      } catch {
        data = null;
      }
    }
    await this.view.webview.postMessage({
      type: "critique-data",
      step,
      data,
    });
  }

  /**
   * Open a file in a VS Code editor tab. The path comes from the
   * chat panel's transcript linkifier; resolve relative paths
   * against the anchored project (or workspace folder if no
   * project is anchored). Errors -- a stale path from old
   * transcript text, an unreadable file, no project to anchor
   * against -- surface as a non-modal warning so a misclick
   * doesn't crash the panel.
   */
  private async openFileInEditor(rawPath: string): Promise<void> {
    const rel = rawPath.trim();
    if (rel.length === 0) {
      return;
    }
    // Refuse `..` traversal anywhere in the path. The file-path
    // linkifier scans LLM output and the LLM is untrusted: a
    // transcript like `../../../etc/passwd` would otherwise let a
    // single misclick open a file far outside the anchored project.
    // Also refuse UNC prefixes on Windows (`\\server\share`) which
    // vscode.Uri.file accepts but the relative-path branch below
    // doesn't gate. See chat-panel audit #9 (2026-05-16).
    if (rel.includes("..") || rel.startsWith("\\\\") || rel.startsWith("//")) {
      void vscode.window.showWarningMessage(
        `sim-flow: refusing to open ${rel} -- path traversal or network shares are blocked from transcript links.`,
      );
      return;
    }
    const anchor =
      this.activePump?.projectDir ?? this.anchoredProjectDir();
    let uri: vscode.Uri;
    if (rel.startsWith("/") || /^[A-Za-z]:[\\/]/.test(rel)) {
      // Absolute path on POSIX or Windows.
      uri = vscode.Uri.file(rel);
    } else if (anchor) {
      const joined = path.normalize(path.join(anchor, rel));
      // After normalize, joined must still be inside `anchor` (a
      // crafted path like `foo/../../etc/passwd` would have escaped
      // before normalization; the `..` check above already rejected
      // those, but this is defense in depth).
      const anchorNorm = path.normalize(anchor);
      const anchorWithSep = anchorNorm.endsWith(path.sep)
        ? anchorNorm
        : anchorNorm + path.sep;
      if (!joined.startsWith(anchorWithSep) && joined !== anchorNorm) {
        void vscode.window.showWarningMessage(
          `sim-flow: refusing to open ${rel} -- resolves outside the anchored project.`,
        );
        return;
      }
      uri = vscode.Uri.file(joined);
    } else {
      void vscode.window.showWarningMessage(
        `sim-flow: cannot open ${rel} -- no project anchored to resolve the relative path.`,
      );
      return;
    }
    try {
      const doc = await vscode.workspace.openTextDocument(uri);
      await vscode.window.showTextDocument(doc, { preview: true });
    } catch (err) {
      void vscode.window.showWarningMessage(
        `sim-flow: failed to open ${uri.fsPath}: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  /**
   * Run `sim-flow convert-sv` against the anchored project and
   * reconnect the pump so the orchestrator picks up the post-flip
   * state.toml (flow = systemverilog-convert, current_step = SV0).
   * No-op when no project is anchored. Errors surface as a
   * non-modal warning.
   */
  private async convertActiveProjectToSv(): Promise<void> {
    const session = this.activePump;
    const projectDir =
      session?.projectDir ?? this.anchoredProjectDir();
    if (!projectDir) {
      void vscode.window.showWarningMessage(
        "sim-flow: no anchored project to convert to SystemVerilog.",
      );
      return;
    }
    const config = vscode.workspace.getConfiguration("sim-flow");
    const setting = config.get<string>("binaryPath");
    const foundationRoot = config.get<string>("foundationRoot");
    let binary: string;
    try {
      binary = resolveBinary({ settingOverride: setting, bundledCandidates });
    } catch (err) {
      void vscode.window.showErrorMessage(
        `sim-flow: cannot resolve sim-flow binary: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
      return;
    }
    const cli = new SimFlowCli({
      binary,
      projectDir,
      foundationRoot: foundationRoot ?? "",
    });
    try {
      await cli.convertSv(false);
    } catch (err) {
      void vscode.window.showWarningMessage(
        `sim-flow: convert-sv failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
      return;
    }
    if (session) {
      // Reuse the standard reconnect helper so the pump re-reads
      // state.toml and picks up `flow = systemverilog-convert,
      // current_step = SV0`. Force manual mode: convert-sv flips
      // the project into a new flow and the user should review
      // SV0 before any auto turns fire. Without this override the
      // reconnect picked up the workspace's `sim-flow.flow.stepMode`
      // setting, so users running on "auto" would skip into SV0
      // work unattended. See chat-panel audit #10 (2026-05-16).
      await this.reconnectActivePump(session, { forceStepMode: "manual" });
    } else {
      void this.refresh();
    }
  }

  /**
   * Read the user's saved palette choice from workspaceState. Falls
   * back to "default" when nothing's been saved -- newly-installed
   * extensions land on the same starting palette every webview
   * would otherwise have ended up at via its local persisted state.
   */
  private readSavedPalette(): ChatPalette {
    const raw = this.workspaceState.get<string>(
      ChatPanelProvider.PALETTE_KEY,
    );
    return CHAT_PALETTE_NAMES.find((p) => p === raw) ?? "default";
  }

  /**
   * Read the user's saved Custom palette colours from workspaceState.
   * Always returns a full set -- missing or partially-malformed
   * stored values fall back to `DEFAULT_CUSTOM_PALETTE` per slot, so
   * the four pickers in the settings popover always have something
   * to bind to.
   */
  private readSavedCustomPalette(): ChatCustomPalette {
    const raw = this.workspaceState.get<Partial<ChatCustomPalette>>(
      ChatPanelProvider.CUSTOM_PALETTE_KEY,
    );
    const isHex = (value: unknown): value is string =>
      typeof value === "string" && /^#[0-9a-fA-F]{3,8}$/.test(value);
    return {
      input: isHex(raw?.input) ? raw.input : DEFAULT_CUSTOM_PALETTE.input,
      tool: isHex(raw?.tool) ? raw.tool : DEFAULT_CUSTOM_PALETTE.tool,
      output: isHex(raw?.output) ? raw.output : DEFAULT_CUSTOM_PALETTE.output,
      accent: isHex(raw?.accent) ? raw.accent : DEFAULT_CUSTOM_PALETTE.accent,
    };
  }

  /**
   * Persist the palette dropdown + the four Custom colours across
   * VS Code restarts. Sent by the webview's settings popover; the
   * webview applies the palette locally for snappy feedback so we
   * don't echo a state-update from here unless the values changed.
   */
  private async persistPaletteChoice(
    palette: ChatPalette,
    custom: ChatCustomPalette,
  ): Promise<void> {
    if (!CHAT_PALETTE_NAMES.includes(palette)) {
      return;
    }
    await this.workspaceState.update(
      ChatPanelProvider.PALETTE_KEY,
      palette,
    );
    await this.workspaceState.update(
      ChatPanelProvider.CUSTOM_PALETTE_KEY,
      custom,
    );
  }

  /**
   * User-initiated project switch from the chat panel toolbar.
   * Always shows the QuickPick (no remembered-project shortcut --
   * the whole point is to pick a different one). If a project is
   * selected, `launchAutoSession` stops the active pump (if any)
   * and starts a fresh session against the chosen project; the
   * `LAST_PROJECT_KEY` workspaceState entry gets overwritten by
   * `launchAutoSession` itself, so the next cold start lands on
   * the new project.
   */
  private async switchProject(): Promise<void> {
    const candidates = await findProjectCandidates();
    if (candidates.length === 0) {
      await vscode.commands.executeCommand("sim-flow.newProject");
      return;
    }
    const picked = await pickProject(candidates, { allowNew: true });
    if (picked === undefined) {
      return;
    }
    if (picked === PICK_PROJECT_NEW) {
      await vscode.commands.executeCommand("sim-flow.newProject");
      return;
    }
    if (this.activePump?.projectDir === picked) {
      // No-op: user re-picked the project that's already anchored.
      // Avoid the relaunch (which would stop + restart the pump for
      // no reason).
      return;
    }
    await this.launchAutoSession(undefined, picked);
  }

  /**
   * Reset the orchestrator's current step: discard its work
   * artifacts + critique state + gate flag so it can be re-run.
   * Modal confirm before the destructive action lands. Reads
   * `current_step` from state.toml at click time so the reset
   * always targets whatever step the orchestrator says it is on,
   * not whatever was current at render time.
   */
  private async resetCurrentStep(): Promise<void> {
    const session = this.activePump;
    if (!session) {
      return;
    }
    if (typeof session.pump.reset !== "function") {
      return;
    }
    const flowState = await readFlowStateSafe(session.projectDir);
    if (!flowState?.current_step) {
      return;
    }
    const targets = stepsFromOnward(flowState.flow, flowState.current_step);
    if (targets.length === 0) {
      return;
    }
    const ok = await confirmReset(targets, flowState.current_step);
    if (!ok) {
      return;
    }
    // Single reset event with the user's target step; the Rust side
    // (`State::reset` + `clear_step_collateral_forward`) already
    // cascades the gate flags + collateral deletion to every
    // downstream step. We used to loop `pump.reset(target)` for
    // each step in `targets`, but each successive Reset overwrote
    // `current_step` with the *latest* step in the cascade, so a
    // reset-from-DM0 would land state.toml at DM4b instead of DM0.
    // `targets` stays valuable for `confirmReset`'s modal copy.
    session.pump.reset(flowState.current_step);
  }

  /**
   * Reset a specific step plus every step after it in the flow's
   * canonical order. Same behaviour as `resetFromEarlierStep` once
   * the user has picked a step -- skips the QuickPick because the
   * rail-tile right-click already named the target.
   */
  private async resetFromStepId(step: string): Promise<void> {
    const session = this.activePump;
    if (!session) {
      return;
    }
    if (typeof session.pump.reset !== "function") {
      return;
    }
    const flowState = await readFlowStateSafe(session.projectDir);
    if (!flowState) {
      return;
    }
    const targets = stepsFromOnward(flowState.flow, step);
    if (targets.length === 0) {
      return;
    }
    const ok = await confirmReset(targets, step);
    if (!ok) {
      return;
    }
    // See `resetCurrentStep`: single reset event, Rust side
    // cascades. Looping `pump.reset(target)` walks current_step
    // forward to the last step in `targets`.
    session.pump.reset(step);
  }

  /**
   * Open a QuickPick of previously-completed steps and reset the
   * chosen one + every step after it in the flow order. Modal
   * confirm precedes the destructive action.
   */
  private async resetFromEarlierStep(): Promise<void> {
    const session = this.activePump;
    if (!session) {
      return;
    }
    if (typeof session.pump.reset !== "function") {
      return;
    }
    const flowState = await readFlowStateSafe(session.projectDir);
    if (!flowState) {
      return;
    }
    const order = stepOrderFor(flowState.flow);
    const passed = order.filter((step) => flowState.gates[step]?.passed === true);
    if (passed.length === 0) {
      await vscode.window.showInformationMessage(
        "No completed steps to reset from. The current step is the earliest unfinished one.",
      );
      return;
    }
    const picked = await vscode.window.showQuickPick(
      passed.map((step) => ({
        label: step,
        description: `Reset \`${step}\` and every step after it`,
      })),
      {
        placeHolder: "Reset which step? This and every later step will be discarded.",
      },
    );
    if (!picked) {
      return;
    }
    const targets = stepsFromOnward(flowState.flow, picked.label);
    if (targets.length === 0) {
      return;
    }
    const ok = await confirmReset(targets, picked.label);
    if (!ok) {
      return;
    }
    // See `resetCurrentStep`: single reset event, Rust side
    // cascades. Looping `pump.reset(target)` walks current_step
    // forward to the last step in `targets`.
    session.pump.reset(picked.label);
  }

  /**
   * Forward the Continue intent to the orchestrator. The orchestrator
   * owns the manual-mode state machine (work -> critique -> advance,
   * with critique outcome driving the work-vs-advance branch); the
   * chat panel just signals intent. Sending over the live pump keeps
   * the orchestrator's accumulated context intact.
   */
  private continueFlow(): void {
    const session = this.activePump;
    if (!session) {
      return;
    }
    session.pump.continueFlow?.();
  }

  /**
   * Open the native picker so the user can drop a path into the
   * composer textarea. Accepts spec-shaped files (markdown, plain
   * text, PDF) and directories (DM0 supports the paginated
   * `docs/spec/` layout). The filter restricts the files tab so
   * the user can't accidentally drop a binary into the spec slot.
   */
  private async pickFile(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      canSelectFiles: true,
      canSelectFolders: true,
      canSelectMany: false,
      openLabel: "Insert path",
      filters: {
        "Spec (markdown, text, PDF)": [
          "md",
          "markdown",
          "txt",
          "text",
          "pdf",
        ],
      },
    });
    if (!picked || picked.length === 0) {
      return;
    }
    if (!this.view || this.disposed) {
      return;
    }
    await this.view.webview.postMessage({
      type: "file-picked",
      path: picked[0]!.fsPath,
    });
  }

  /**
   * Apply a live step-mode change. Mirrors the dashboard's
   * `handleSetStepMode`: persist the config so the next launch
   * starts in this mode, then send `SetStepMode` over the pump if
   * one is alive. The orchestrator echoes the new value via
   * `StepModeChanged`, which our listener turns into a refresh so
   * the toolbar toggle ends up reflecting the orchestrator's
   * truth.
   */
  private async handleSetStepMode(mode: "auto" | "manual"): Promise<void> {
    await vscode.workspace
      .getConfiguration("sim-flow")
      .update("flow.stepMode", mode, vscode.ConfigurationTarget.Workspace);
    const pump = this.activePump?.pump;
    if (pump && typeof pump.setStepMode === "function") {
      pump.setStepMode(mode);
      return;
    }
    // No live pump: optimistic refresh so the toggle reflects the
    // persisted value immediately.
    await this.refresh();
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
      await this.waitForPendingConversationWrites();
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
    const context = await this.readPanelContext();
    if (
      this.pendingAutoLaunch &&
      this.pendingAutoLaunch.projectDir === context.projectDir
    ) {
      return;
    }
    if (this.activePump) {
      // Route to the pump whenever it's anchored to this project --
      // not only when `awaitingInput` is true. In manual mode the
      // orchestrator parks at `wait_for_command` between sub-sessions
      // WITHOUT emitting `RequestUserInput`, so awaitingInput stays
      // false there even though `UserMessage` is the right thing to
      // send (the orchestrator dispatches it as a Q&A turn). The
      // composer is already disabled while `isStreaming` (i.e.
      // mid-sub-session), so this widened path doesn't admit racy
      // sends during active work.
      if (this.activePump.projectDir === context.projectDir) {
        await this.sendPumpPrompt(context, prompt);
      }
      return;
    }
    // No live orchestrator session: the chat panel used to dispatch
    // directly against the configured LLM here, but the TS-side LLM
    // clients were removed once the Rust orchestrator absorbed all
    // dispatch. Without an active pump the panel is display-only;
    // surface a note so the user knows where to start the flow.
    let conversation = this.readConversation(context.projectDir);
    if (hasInterruptedAutoSessionTranscript(conversation.transcript)) {
      conversation = appendNote(
        conversation,
        "Session no longer live",
        "Relaunch the flow from the dashboard or clear the transcript to start a fresh chat.",
      );
    } else {
      conversation = appendNote(
        conversation,
        "No active session",
        "Start a flow from the dashboard's Run / Connect button to chat with the orchestrator.",
      );
    }
    await this.persistConversation(context.projectDir, conversation);
    await this.postState(context, conversation);
  }

  private async clearTranscript(): Promise<void> {
    if (this.activePump) {
      return;
    }
    const context = await this.readPanelContext();
    const conversation = clearConversationState();
    await this.persistConversation(context.projectDir, conversation);
    await this.postState(context, conversation);
  }

  /**
   * Terminate the active session: cancel, shutdown, escalate to
   * SIGTERM/SIGKILL if the orchestrator doesn't exit cleanly,
   * then clear the active-session pin. The chat panel becomes
   * idle and the toolbar's project button reverts to "Start
   * session". Distinct from `stopConversation`, which only
   * cancels the current activity without killing the pump.
   */
  private async endSession(): Promise<void> {
    const context = await this.readPanelContext();
    let conversation = this.readConversation(context.projectDir);

    if (this.activePump?.projectDir !== context.projectDir) {
      return;
    }
    if (this.activePump.stopRequested) {
      return;
    }
    const session = this.activePump;
    session.stopRequested = true;
    session.awaitingInput = false;
    conversation = appendNote(
      conversation,
      "Ending session",
      "Terminating the running sim-flow session.",
    );
    await this.persistConversation(context.projectDir, conversation);
    await this.postState(context, conversation);

    // Send `cancel` first so the orchestrator can finish cleanly
    // if it's parked. Then escalate to shutdown -> SIGTERM ->
    // SIGKILL via `disconnectWithEscalation` -- the soft cancel
    // doesn't reach a child blocked inside a synchronous LLM
    // dispatch (the wire reader only services events between
    // turns), so the only reliable way to interrupt a mid-LLM-call
    // run is to terminate the process.
    session.pump.cancel();
    let outcome: "clean" | "sigterm" | "sigkill" | "already-gone" | undefined;
    try {
      outcome = await session.pump.disconnectWithEscalation?.(1_000, 1_000);
    } catch (err) {
      console.error(
        `sim-flow: chat-panel end-session escalation failed: ${(err as Error).message ?? String(err)}`,
      );
    }
    if (this.activePump !== session) {
      return;
    }
    await this.autoSessions.clearIfActive(session);
    let finalConversation = this.readConversation(context.projectDir);
    finalConversation = appendNote(
      finalConversation,
      "Session ended",
      outcome === "sigkill"
        ? "The orchestrator did not exit after shutdown / SIGTERM and was killed (SIGKILL)."
        : "Stopped the running sim-flow session.",
    );
    await this.persistConversation(context.projectDir, finalConversation);
    await this.postState(context, finalConversation);
  }

  private async stopConversation(): Promise<void> {
    const context = await this.readPanelContext();
    let conversation = this.readConversation(context.projectDir);

    if (this.activePump?.projectDir !== context.projectDir) {
      return;
    }
    if (this.activePump.stopRequested) {
      // Rapid double-clicks: a cancel is already in flight. Don't
      // re-send (idempotent on the wire anyway) and don't append a
      // duplicate note. The flag clears in onManagedSessionSettled
      // when the orchestrator confirms it's parked.
      return;
    }
    const session = this.activePump;
    session.stopRequested = true;

    // The Stop button's intent is "halt the current activity but
    // keep the session alive": cancel the in-flight sub-session
    // and drop the orchestrator into manual mode so it parks at
    // `wait_for_command` instead of plowing into the next bracket.
    // No process termination, no escalation -- the session stays
    // attached and the user can resume with the Continue button or
    // typed input.
    //
    // Order: flip to manual first so the auto loop's post-
    // sub-session mode check returns `FlippedToManual` after the
    // cancel lands; otherwise auto could schedule the next
    // sub-session before the mode flag arrives.
    if (typeof session.pump.setStepMode === "function") {
      session.pump.setStepMode("manual");
    }
    session.pump.cancel();

    // Persist the operator-visible note. The cancel byte is in the
    // socket buffer NOW but the orchestrator only services wire
    // events between LLM turns, so a mid-dispatch cancel waits for
    // the current `dispatch_with_tools` call to return -- which can
    // take seconds for a long tool-loop turn. Word the note honestly
    // so the user understands "Stop" is a request, not a synchronous
    // halt; the second `request-user-input` from the manual loop is
    // the signal that the cancel has actually landed (the panel
    // re-enables the Stop button at that point via stopRequested
    // clearing in onManagedSessionSettled).
    conversation = appendNote(
      conversation,
      "Stopping",
      "Cancel requested and step mode set to Manual. The orchestrator finishes the in-flight LLM turn before it parks, so this can take a few seconds; the chat indicator will switch to \"Waiting on you\" once the cancel lands.",
    );
    await this.persistConversation(context.projectDir, conversation);
    await this.postState(context, conversation);
  }

  /**
   * Project the chat panel should be displaying RIGHT NOW. Anchors to
   * the live session's project (active pump > pending launch >
   * relaunch anchor) so the panel doesn't auto-follow the user's
   * active text editor when there's already a session attached.
   * Without this anchor, switching files between sim-flow projects
   * in the workspace flips the panel's transcript out from under a
   * running session.
   *
   * The `pendingRelaunchAnchor` slot is dedicated to the
   * Reload-Window auto-recovery path:
   * `restoreActiveAutoSessionIfNeeded`'s catch sets it synchronously
   * BEFORE the fire-and-forget `launchAutoSession`, so this refresh's
   * `postState` anchors to the right project from the very first
   * paint. Without it the toolbar would briefly read "No project
   * selected" during the window between catch firing and
   * `launchAutoSession` setting its own `pendingAutoLaunch`.
   */
  private anchoredProjectDir(): string | null {
    return (
      this.activePump?.projectDir ??
      this.pendingAutoLaunch?.projectDir ??
      this.pendingRelaunchAnchor ??
      null
    );
  }

  /**
   * True while a fresh orchestrator is being spawned for the panel
   * (the `pendingAutoLaunch` window OR the brief window between
   * `restoreActiveAutoSessionIfNeeded`'s catch firing the relaunch
   * and `launchAutoSession` setting its own anchor). The webview
   * reads this to render a "Launching…" indicator instead of the
   * empty-state "Start session" button so the cold-start auto-
   * resume reads as progress rather than as "no project anchored."
   */
  private isSessionLaunching(): boolean {
    return (
      this.pendingAutoLaunch !== undefined || this.pendingRelaunchAnchor !== null
    );
  }

  private async readPanelContext(): Promise<PanelContext> {
    const projectDir = this.anchoredProjectDir() ?? (await resolveProjectDirForPanel());
    const settings = readPanelSettings();
    const flowState = projectDir ? await readFlowStateSafe(projectDir) : null;
    const currentStep = flowState?.current_step ?? null;
    const gates = flowState?.gates ?? {};
    const flow = flowState?.flow ?? null;
    const currentMilestone =
      projectDir && currentStep
        ? await readCurrentMilestoneSafe(projectDir, currentStep)
        : null;
    const projectLabel =
      projectDir !== null
        ? path.basename(projectDir)
        : vscode.workspace.workspaceFolders?.[0]?.name ?? "No project selected";

    return {
      projectLabel,
      projectDir,
      currentStep,
      flow,
      gates,
      currentMilestone,
      source: settings.source,
      rawSource: settings.rawSource,
      baseUrl: settings.baseUrl,
      modelFamilyId: settings.modelFamilyId,
      runtimeProfileId: settings.runtimeProfileId,
      unresolvedServer: settings.unresolvedServer,
      sourceLabel: settings.sourceLabel,
      model: settings.model,
      verbose: settings.verbose,
      ollamaBaseUrl: settings.ollamaBaseUrl,
      lmstudioBaseUrl: settings.lmstudioBaseUrl,
      ...describePanelSession(projectDir, currentStep, settings.sourceLabel, this.activePump),
    };
  }

  /**
   * Auto-resume the previously-active project when the chat panel
   * mounts. Silent and side-effect-light: no picker, no errors --
   * the goal is "the panel opens on the last project, sitting idle
   * in manual mode, so the user clicks Play to start work." When no
   * candidate project can be inferred, falls through to the
   * empty-state "Start session" button so the user can pick one.
   *
   * Forces Manual mode regardless of the workspace
   * `sim-flow.flow.stepMode` setting so the orchestrator parks at
   * `wait_for_command` instead of plowing forward unattended.
   *
   * Two responsibilities are kept separate:
   *   - When the previous orchestrator process is dead (Reload
   *     Window), `restoreActiveAutoSessionIfNeeded`'s catch fires
   *     its own auto-relaunch for the same project. tryAutoResume
   *     doesn't need to handle that case -- by the time it runs,
   *     `pendingAutoLaunch` or `pendingRelaunchAnchor` is set and
   *     the first-line guard short-circuits.
   *   - When NO auto-session record exists at all (fresh extension
   *     install / workspaceState cleared / very first chat-panel
   *     use), tryAutoResume picks the remembered project and
   *     launches it. When the record exists and is non-terminal,
   *     restoreActiveAutoSessionIfNeeded owned that path and either
   *     succeeded or kicked its own relaunch -- in either case
   *     activePump or the relaunch anchor is set and we skip.
   */
  private async tryAutoResume(): Promise<void> {
    if (
      this.activePump ||
      this.pendingAutoLaunch ||
      this.pendingRelaunchAnchor
    ) {
      return;
    }
    const remembered = this.workspaceState.get<string>(
      ChatPanelProvider.LAST_PROJECT_KEY,
    );
    if (!remembered) {
      return;
    }
    const existingRecord = this.autoSessions.readStoredRecord(remembered);
    // When the record is still on disk and non-terminal, the restore
    // path inside `refresh()` owned this project -- either it
    // attached (activePump set, gate above caught it) or its catch
    // launched a fresh pump (pendingRelaunchAnchor / pendingAutoLaunch
    // set, gate above caught it). Skip so we don't race a second
    // pump alongside the restored one, and don't override an active
    // user-driven project switch (a workspace switch since the
    // previous run should keep the existing "follow the workspace"
    // semantics). See chat-panel audit #7 (2026-05-16) for the
    // original race-mitigation rationale; the workspace-switch
    // requirement is pinned by the
    // `restores the newly active project and source after reload`
    // test in mockFlowHarness.
    if (existingRecord && !isTerminalLlmSource(existingRecord.sourceTag)) {
      return;
    }
    const candidates = await findProjectCandidates();
    if (!candidates.includes(remembered)) {
      return;
    }
    await this.launchAutoSession(undefined, remembered, {
      forceStepMode: "manual",
    });
  }

  /**
   * Explicit "Start session" action from the chat panel toolbar.
   * Fires only when the user clicks; window reload, tab focus,
   * etc. no longer trigger it. If a previous project is remembered
   * and still on disk, the orchestrator launches directly with no
   * picker; otherwise the standard QuickPick appears (single-
   * candidate cases also auto-resolve). Picking "+ New project..."
   * dispatches the existing `sim-flow.newProject` command. The
   * spec path is left undefined so DM0 parks at its
   * `RequestUserInput` (the chat panel surfaces that through the
   * existing `currentPrompt` channel).
   *
   * Step-mode is forced to `manual` regardless of the workspace
   * `sim-flow.flow.stepMode` setting: the click is "open the chat
   * tab on this project so I can poke around," not "start grinding
   * the next gate." A user who wants Auto can flip the toggle in
   * the toolbar once the panel is up. Mirrors `tryAutoResume`'s
   * `forceStepMode: "manual"` for the same reason.
   */
  private async startSession(): Promise<void> {
    if (this.activePump) {
      // A session is already live for the chat panel -- the
      // "Start session" button shouldn't have been clickable, but
      // be defensive: don't double-launch.
      return;
    }
    const candidates = await findProjectCandidates();
    if (candidates.length === 0) {
      // No initialized projects in the workspace -- mirror
      // `switchProjectCommand` and short-circuit to the new-project
      // flow. `pickProject` returns undefined for an empty list
      // regardless of `allowNew`, so we have to branch here.
      await vscode.commands.executeCommand("sim-flow.newProject");
      return;
    }
    // If we previously remembered a project that's still on disk,
    // skip the picker entirely so "Start session" is a single
    // click. Stale entries (project deleted, renamed) fall through
    // to the picker.
    const remembered = this.workspaceState.get<string>(
      ChatPanelProvider.LAST_PROJECT_KEY,
    );
    if (remembered && candidates.includes(remembered)) {
      await this.launchAutoSession(undefined, remembered, {
        forceStepMode: "manual",
      });
      return;
    }
    const picked = await pickProject(candidates, { allowNew: true });
    if (picked === undefined) {
      // User cancelled the picker; honour silently.
      return;
    }
    if (picked === PICK_PROJECT_NEW) {
      await vscode.commands.executeCommand("sim-flow.newProject");
      return;
    }
    await this.launchAutoSession(undefined, picked, {
      forceStepMode: "manual",
    });
  }

  async launchAutoSession(
    specPath: string | undefined,
    projectDirHint: string | undefined,
    options: {
      forceStepMode?: "auto" | "manual";
      /**
       * Keep the existing chat transcript instead of clearing it
       * before the new orchestrator's startup note. Used by the
       * Reload-Window auto-relaunch path so the user comes back to
       * the same conversation they were looking at before the
       * window reload.
       */
      preserveConversation?: boolean;
      /**
       * Suppress the "Flow launched from dashboard" startup note.
       * Pair with `preserveConversation: true` so consecutive
       * Reload-Window auto-relaunches don't pile duplicate launch
       * notes onto the preserved transcript.
       */
      skipLaunchNote?: boolean;
    } = {},
  ): Promise<void> {
    // Resolve the target project BEFORE revealing the chat view so we
    // can anchor the panel and pre-clear its transcript cache. The
    // visibility hook fires `refresh()` async on reveal; without
    // pre-anchoring it resolves to whatever `resolveProjectDir()`
    // picks from the active editor, which can be a different
    // sim-flow project than the one we're launching against and
    // produces a brief flash of the editor-project's prior
    // transcript before our `postState` overwrites it.
    const ctx = await resolveContext({
      projectDir: projectDirHint,
      showErrors: true,
    });
    if (!ctx) {
      return;
    }
    // Persist the resolved project dir so the next cold-start auto-
    // launch can skip the picker. We record on every launch (not
    // just the auto-launch path) so dashboard-driven Connect clicks
    // also seed the memory.
    await this.workspaceState.update(
      ChatPanelProvider.LAST_PROJECT_KEY,
      ctx.projectDir,
    );

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
      await vscode.commands.executeCommand(
        `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
      );
      return;
    }
    if (
      this.pendingAutoLaunch &&
      this.pendingAutoLaunch.projectDir === ctx.projectDir &&
      this.pendingAutoLaunch.launchSpecPath === trimmedSpec &&
      this.pendingAutoLaunch.sourceTag === settings.source &&
      this.pendingAutoLaunch.model === settings.model
    ) {
      await vscode.commands.executeCommand(
        `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
      );
      return;
    }

    if (this.activePump) {
      await this.stopActivePumpSession(
        this.activePump,
        "Launching new flow",
        "Stopped the running sim-flow session to launch a new flow.",
      );
    }

    // Anchor + reset the conversation cache for ctx.projectDir, then
    // reveal the view. `startAutoSession` will overwrite this cache
    // entry with the launch note + post the full state.
    //
    // `preserveConversation` skips the cache wipe so the auto-
    // relaunch on Reload Window doesn't blow away the prior session's
    // transcript -- the launch note is appended to whatever's already
    // there.
    this.pendingAutoLaunch = {
      projectDir: ctx.projectDir,
      launchSpecPath: trimmedSpec,
      sourceTag: settings.source,
      model: settings.model,
    };
    if (!options.preserveConversation) {
      this.rememberConversation(ctx.projectDir, clearConversationState());
    }

    await vscode.commands.executeCommand(
      `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
    );

    await this.startAutoSession(ctx, trimmedSpec, {
      resetConversation: !options.preserveConversation,
      forceStepMode: options.forceStepMode,
      skipLaunchNote: options.skipLaunchNote,
    });
  }

  /**
   * Attach as a read-only observer to a `--watch-socket` tap. The
   * orchestrator's primary host (e2e_manual / another dashboard /
   * an external script) keeps driving; we just receive the JSONL
   * event stream, render it the same way the live pump does, and
   * disable the user's command surfaces (composer, per-step
   * buttons, Stop). Reveals the chat panel and refreshes the
   * dashboard via the existing onActiveSessionChanged hook.
   */
  async attachWatcherSession(args: {
    socketPath: string;
    projectDir: string;
    pid: number;
    llmBackend: string;
    llmModel: string | null;
  }): Promise<void> {
    if (this.disposed) {
      return;
    }
    if (this.activePump && this.activePump.projectDir === args.projectDir) {
      if (this.activePump.pump.isViewer) {
        // Existing pump is also a viewer (e.g. user re-attaching
        // to a watcher after the orchestrator restarted, or just
        // refreshing). Tear it down and re-attach. No-op if it
        // was disposed already.
        try {
          this.activePump.pump.dispose();
        } catch {
          // Ignore: the new attach proceeds regardless.
        }
        await this.autoSessions.clearIfActive(this.activePump);
      } else {
        // Existing pump is DRIVING. Don't silently steal -- the
        // user might be in the middle of work. Surface a notice
        // and let them Disconnect first.
        void vscode.window.showWarningMessage(
          `sim-flow: a driving session is already attached to ${args.projectDir}. Disconnect first, then attach as viewer.`,
        );
        return;
      }
    }
    const sessionId = `viewer-${args.pid}-${Date.now()}`;
    // Tag the source as best-effort; LlmSourceTag is a closed
    // enum and watchers come from arbitrary backends, so map to
    // a rough match (the viewer never dispatches LLM calls --
    // dispatchLlm is skipped in viewer mode -- so the tag is
    // purely cosmetic for the chat-panel header).
    const sourceTag = mapBackendToSourceTag(args.llmBackend);
    const record: StoredAutoSessionRecord = {
      sessionId,
      socketPath: args.socketPath,
      projectDir: args.projectDir,
      awaitingInput: false,
      sourceTag,
      model: args.llmModel ?? "",
      sessionMode: "auto",
      stepRef: null,
      launchSpecPath: undefined,
      updatedAtMs: Date.now(),
    };
    const ctx = await resolveContext({
      projectDir: args.projectDir,
      showErrors: false,
    });
    if (!ctx) {
      void vscode.window.showErrorMessage(
        "sim-flow: cannot resolve sim-flow binary; viewer attach aborted.",
      );
      return;
    }
    // Viewer pumps never dispatch LLM calls (see
    // `SocketSessionPump.dispatchLlm` skip in viewer mode), so the
    // LLM-config fields are placeholders. Keep the same shape the
    // pump expects so we don't widen the interface for one
    // call site.
    const llmConfig: PumpLlmConfig = {
      source: sourceTag as unknown as PumpLlmConfig["source"],
      model: args.llmModel ?? undefined,
      secrets: this.secrets,
      projectDir: args.projectDir,
      binary: ctx.cli.binary,
      debugTokens: "",
    };
    const pump = new SocketSessionPump(
      {
        sessionId,
        socketPath: args.socketPath,
        viewer: true,
      },
      llmConfig,
    );
    try {
      await pump.ready();
    } catch (err) {
      void vscode.window.showErrorMessage(
        `sim-flow: failed to attach viewer to ${args.socketPath}: ${(err as Error).message ?? String(err)}`,
      );
      pump.dispose();
      return;
    }
    this.rememberConversation(args.projectDir, clearConversationState());
    // Attach BEFORE revealing so the visibility-triggered refresh
    // sees `activeSession` populated and renders VIEWING immediately
    // -- otherwise the reveal's async `refresh()` can race ahead of
    // attach and leave the pill stuck on OFFLINE when the user's
    // active editor is in a different sim-flow project than the
    // watcher's. The constructor's `onActiveSessionChanged` listener
    // also calls `refresh()` to belt-and-suspenders this.
    await this.autoSessions.attach(record, pump, this.autoSessionDelegate());
    await vscode.commands.executeCommand(
      `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
    );
    // Surface attach success so the user knows the viewer is live.
    // Without this, a successful attach is silent and is easy to
    // confuse with the failure path (where we DO show a notice) --
    // especially when the chat panel pill takes a moment to switch
    // from OFFLINE to VIEWING while history replays.
    void vscode.window.showInformationMessage(
      `sim-flow: attached as viewer to ${path.basename(args.projectDir)} (pid ${args.pid}).`,
    );
  }

  async launchStepSession(
    step: string,
    kind: "work" | "critique",
    projectDirHint: string | undefined,
  ): Promise<void> {
    // Resolve the project before revealing so we can pre-anchor the
    // panel to the launching project's transcript. See the matching
    // comment in `launchAutoSession` for the rationale.
    const ctx = await resolveContext({
      projectDir: projectDirHint,
      showErrors: true,
    });
    if (!ctx) {
      return;
    }
    await this.workspaceState.update(
      ChatPanelProvider.LAST_PROJECT_KEY,
      ctx.projectDir,
    );

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
      await vscode.commands.executeCommand(
        `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
      );
      return;
    }

    if (this.activePump) {
      await this.stopActivePumpSession(
        this.activePump,
        "Launching new session",
        `Stopped the running sim-flow session to launch \`${step}.${kind}\`.`,
      );
    }

    // Anchor the panel to ctx.projectDir before revealing so the
    // visibility-triggered refresh resolves to this project (not the
    // active editor's), and pre-clear the cache so it doesn't render
    // the prior session's transcript briefly. `startStepSession`
    // doesn't manage `pendingAutoLaunch` itself (it's primarily an
    // auto-launch concept), so clean it up explicitly once the
    // session is up.
    const launchAnchor: PendingAutoLaunchState = {
      projectDir: ctx.projectDir,
      launchSpecPath: undefined,
      sourceTag: settings.source,
      model: settings.model,
    };
    this.pendingAutoLaunch = launchAnchor;
    this.rememberConversation(ctx.projectDir, clearConversationState());

    await vscode.commands.executeCommand(
      `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`,
    );

    try {
      await this.startStepSession(ctx, stepRef, { resetConversation: true });
    } finally {
      if (this.pendingAutoLaunch === launchAnchor) {
        this.pendingAutoLaunch = undefined;
      }
    }
  }

  private async startAutoSession(
    ctx: { projectDir: string; cli: { binary: string; foundationRoot?: string } },
    trimmedSpec: string | undefined,
    options: {
      resetConversation: boolean;
      launchTitle?: string;
      launchBody?: string;
      /** Force a specific step mode regardless of the workspace
       * `sim-flow.flow.stepMode` setting. Used by auto-resume to
       * land the orchestrator in Manual park on cold start. */
      forceStepMode?: "auto" | "manual";
      /** Suppress the "Flow launched from dashboard" startup note.
       * The Reload-Window auto-relaunch path uses this together with
       * `resetConversation: false` so consecutive reloads don't pile
       * duplicate launch notes onto the preserved transcript -- the
       * orchestrator's own "Session started" line still appears and
       * marks the resume cleanly. */
      skipLaunchNote?: boolean;
    },
  ): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const llmConfig = buildPumpLlmConfig(ctx, this.secrets, config);
    const maxWorkIters = config.get<number>("auto.maxWorkIterations") ?? 6;
    const maxCritiqueIters = config.get<number>("auto.maxCritiqueIterations") ?? 10;
    const maxCritiqueNoProgressIters =
      config.get<number>("auto.maxCritiqueNoProgressIterations") ?? 3;
    // `forceStepMode` overrides the workspace setting -- used by
    // the on-ready auto-resume path so a session that was running
    // in Auto when the window closed comes back parked at manual
    // (so it doesn't immediately resume work without the user's
    // explicit say-so).
    const stepMode = options.forceStepMode ?? readStepModeSetting(config);

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
    if (llmConfig.baseUrl) {
      // Custom `server:<name>` entries from `sim-flow.llm.servers`
      // set this -- without forwarding, the orchestrator falls back
      // to the backend's conventional default port (e.g. vLLM 8000)
      // even though the user picked a different host:port row in the
      // dashboard's Source dropdown.
      args.push("--llm-base-url", llmConfig.baseUrl);
    }
    args.push("--max-auto-iters", String(maxWorkIters));
    args.push("--max-critique-iters", String(maxCritiqueIters));
    args.push(
      "--max-critique-no-progress-iters",
      String(maxCritiqueNoProgressIters),
    );
    args.push("--step-mode", stepMode);
    if (trimmedSpec) {
      args.push("--spec", trimmedSpec);
    } else {
      args.push("--dm0-interactive");
    }

    const context = await this.readPanelContextForProject(ctx.projectDir);
    let conversation = options.resetConversation
      ? clearConversationState()
      : this.readConversation(ctx.projectDir);
    if (!options.skipLaunchNote) {
      conversation = appendNote(
        conversation,
        options.launchTitle ?? "Flow launched from dashboard",
        options.launchBody ??
          (trimmedSpec
            ? `Started sim-flow auto for \`${path.basename(ctx.projectDir)}\` with spec \`${trimmedSpec}\`.`
            : "Started sim-flow auto without a spec; DM0 will stop for input before the rest of the flow continues."),
      );
    }
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
      // Fire-and-forget query the backend for its actual context
      // window so the chat panel's "% context used" pie reflects
      // reality (vLLM `max_model_len`, LM Studio
      // `loaded_context_length`, Ollama `<arch>.context_length`).
      // No-op for sources that don't expose it; the webview falls
      // back to its cosmetic default in that case.
      void this.kickContextWindowQuery(ctx.projectDir, llmConfig);
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

    // Side step-session launches happen only when the dashboard
    // doesn't already have an orchestrator attached. When a manual-
    // mode pump IS alive, the dashboard's per-step buttons route the
    // request as a `RunStep` HostEvent instead of spawning a fresh
    // `sim-flow session ...` process — see `routeManualCommand` in
    // `webview/host.ts`. The fallback path below keeps the
    // legacy "open a chat tab from cold" behavior.
    if (this.autoSessions.getActiveSession()?.projectDir === ctx.projectDir) {
      // Defensive: callers should suppress this path when a session
      // is already live, but if they don't we surface the issue as a
      // diagnostic note rather than launching a duplicate orchestrator
      // that would race over the project's `.sim-flow/state.toml`.
      const conversation = this.readConversation(ctx.projectDir);
      const message =
        "An orchestrator is already attached for this project; the dashboard's per-step controls send manual-mode commands directly. " +
        "Disconnect first if you want to re-launch a fresh side session.";
      const note = appendNote(conversation, "Step session skipped", message);
      await this.persistConversation(ctx.projectDir, note);
      const context = await this.readPanelContextForProject(ctx.projectDir);
      await this.postState(context, note);
      return;
    }

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
    if (llmConfig.baseUrl) {
      args.push("--llm-base-url", llmConfig.baseUrl);
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
    // "Streaming" here means "the orchestrator is actively working;
    // disable user input and show the stop icon." Originally derived
    // from `!awaitingInput`, but that flag stays false in manual
    // mode's `wait_for_command` park -- so the UI thought the pump
    // was busy when it was actually idle, which gated Continue and
    // kept the stop glyph visible. `pump.inSubSession` is the
    // accurate "orchestrator is mid-work" signal; `pendingAutoLaunch`
    // covers the pre-helloAck launching window.
    const isStreaming =
      (!!this.pendingAutoLaunch &&
        this.pendingAutoLaunch.projectDir === context.projectDir) ||
      (!!this.activePump &&
        this.activePump.projectDir === context.projectDir &&
        this.activePump.pump.inSubSession === true);
    const isViewer =
      !!this.activePump &&
      this.activePump.projectDir === context.projectDir &&
      !!this.activePump.pump.isViewer;
    const supportsPromptEntry =
      !isViewer &&
      ((!!this.activePump &&
        this.activePump.projectDir === context.projectDir &&
        this.activePump.awaitingInput) ||
        (!isTerminalLlmSource(context.source) && !hasInterruptedAutoSession));
    return {
      mode: "live",
      projectLabel: context.projectLabel,
      projectDir: context.projectDir,
      flow: context.flow,
      passedSteps: Object.entries(context.gates)
        .filter(([, gate]) => gate?.passed === true)
        .map(([step]) => step),
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
      awaitingUserInput: awaitingPumpInput,
      currentPrompt:
        this.activePump?.projectDir === context.projectDir
          ? (this.activePump.currentPrompt ?? null)
          : null,
      currentPlaceholder:
        this.activePump?.projectDir === context.projectDir
          ? (this.activePump.currentPlaceholder ?? null)
          : null,
      pendingFollowups:
        this.activePump?.projectDir === context.projectDir
          ? this.activePump.pendingFollowups
          : [],
      idleQaHint:
        // Show the idle Q&A helper when there's a live pump for
        // this project AND it's not currently in a sub-session and
        // not parked at request-user-input. In those two states the
        // notice / currentPrompt banner is more useful and the
        // helper would be redundant. Viewers don't drive, so omit.
        this.activePump?.projectDir === context.projectDir &&
        !this.activePump.pump.isViewer &&
        !this.activePump.pump.inSubSession &&
        !this.activePump.awaitingInput
          ? "Side-conversation Q&A: ask anything about this project. Click a step command on the right to end this conversation and run that step."
          : null,
      isViewer,
      sessionStep:
        this.activePump?.projectDir === context.projectDir
          ? (this.activePump.pump.session?.step ?? null)
          : null,
      sessionKind:
        this.activePump?.projectDir === context.projectDir
          ? (this.activePump.pump.session?.kind ?? null)
          : null,
      supportsPromptEntry,
      // The Stop button is "available" while there's a live (or
      // launching) session to stop AND we haven't already shipped a
      // cancel. Once `stopRequested` is true the orchestrator has
      // received the cancel byte on the wire but the cancel might
      // not take effect for seconds (the wire reader only services
      // events between LLM turns, so a mid-dispatch cancel waits
      // for the current `dispatch_with_tools` to return). Disabling
      // here gives honest visual feedback that the click was
      // registered; the flag clears in `onManagedSessionSettled`
      // when the orchestrator confirms it's parked.
      canStop:
        (!!this.activePump &&
          this.activePump.projectDir === context.projectDir &&
          !this.activePump.stopRequested) ||
        (!!this.pendingAutoLaunch &&
          this.pendingAutoLaunch.projectDir === context.projectDir),
      currentStepMode:
        this.activePump?.projectDir === context.projectDir
          ? (this.activePump.pump.stepMode ?? null)
          : null,
      nextActionHint:
        this.activePump?.projectDir === context.projectDir &&
        this.activePump.nextActionHint !== undefined
          ? { label: this.activePump.nextActionHint }
          : null,
      sessionActive:
        !!this.activePump && this.activePump.projectDir === context.projectDir,
      sessionLaunching:
        this.isSessionLaunching() &&
        (this.pendingAutoLaunch?.projectDir === context.projectDir ||
          this.pendingRelaunchAnchor === context.projectDir),
      currentMilestone: context.currentMilestone,
      verilogEnabled:
        vscode.workspace
          .getConfiguration("sim-flow")
          .get<boolean>("verilog.enabled") === true,
      showContextState:
        vscode.workspace
          .getConfiguration("sim-flow")
          .get<boolean>("chatPanel.showContextState") === true,
      evictedMessages:
        this.activePump?.projectDir === context.projectDir
          ? Array.from(this.activePump.evictedMessageIds.entries())
          : [],
      contextWindow:
        this.activePump?.projectDir === context.projectDir
          ? this.activePump.contextWindow
          : null,
      palette: this.readSavedPalette(),
      customPalette: this.readSavedCustomPalette(),
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
    const key = conversationStorageKey(projectDir);
    this.conversations.set(key, conversation);
    // Schedule a debounced persist so streaming chat events survive an
    // unexpected extension-host death (the typical Developer: Reload
    // Window case). Without this, mid-session events live only in
    // memory: `onManagedSessionSettled` does persist at the park /
    // end boundary, but the user's transcript-up-to-the-park is gone
    // if they reload before then.
    //
    // Debouncing (one persist per ~250ms regardless of event volume)
    // keeps the workspaceState writes coarse-grained even when the
    // orchestrator streams hundreds of per-token assistant chunks.
    // The flush-on-deactivate hook in `dispose` covers the rare case
    // where the timer hasn't fired yet when the extension is being
    // torn down.
    this.schedulePersist(projectDir);
  }

  private pendingPersistTimers: Map<string, NodeJS.Timeout> = new Map();

  private schedulePersist(projectDir: string | null): void {
    const key = conversationStorageKey(projectDir);
    const existing = this.pendingPersistTimers.get(key);
    if (existing) {
      clearTimeout(existing);
    }
    const timer = setTimeout(() => {
      this.pendingPersistTimers.delete(key);
      const cached = this.conversations.get(key);
      if (cached) {
        void this.persistConversation(projectDir, cached);
      }
    }, 250);
    this.pendingPersistTimers.set(key, timer);
  }

  private flushPendingPersistsSync(): void {
    // Best-effort: drain any scheduled timers immediately. Used by
    // `dispose` so a reload that hits within the 250ms debounce window
    // still persists the latest in-memory conversation. The persist
    // itself is async (queued via `queueConversationWrite`); we can't
    // synchronously block here, so the dispose path also awaits the
    // queue separately via `waitForPendingConversationWrites`.
    for (const [key, timer] of this.pendingPersistTimers) {
      clearTimeout(timer);
      const cached = this.conversations.get(key);
      if (cached) {
        const projectDir = key.replace(
          /^sim-flow\.chatPanel\.conversation\./,
          "",
        );
        const resolvedDir = projectDir === "__workspace__" ? null : projectDir;
        void this.persistConversation(resolvedDir, cached);
      }
    }
    this.pendingPersistTimers.clear();
  }

  private async persistConversation(
    projectDir: string | null,
    conversation: ChatConversationState,
  ): Promise<void> {
    const key = conversationStorageKey(projectDir);
    const stored = toStoredConversation(conversation);
    this.conversations.set(key, conversation);
    await this.queueConversationWrite(async () => {
      await this.workspaceState.update(key, stored);
    });
  }

  private async postState(
    context: PanelContext,
    conversation: ChatConversationState,
  ): Promise<void> {
    const state = this.buildState(context, conversation);
    this.assertChatPanelStateInvariants(state);
    await this.post({
      type: "state-update",
      state,
    });
  }

  /**
   * Soft-assert: log when a `ChatPanelState` carries a logically-
   * impossible combination of flags. These pairs are mutually
   * exclusive in the orchestrator's contract (a session is either
   * parked-waiting-on-input OR mid-work, never both; neither flag
   * makes sense without an active session), so a violation here
   * means a state-update slipped through with the kind of drift
   * that produces user-visible stuck UIs (e.g. the "WAITING ON YOU"
   * pill rendered next to a Stop button + disabled Play button --
   * 2026-05-17 regression). Pure observation: we still post the
   * offending state so the underlying bug stays visible rather than
   * silently papered over. Logged via `console.warn` so the message
   * surfaces in the Output channel / Developer Tools console with
   * enough context to trace which postState path produced it.
   */
  private assertChatPanelStateInvariants(state: ChatPanelState): void {
    const violations: string[] = [];
    if (state.awaitingUserInput && state.isStreaming) {
      violations.push(
        "awaitingUserInput=true && isStreaming=true (orchestrator can't be both parked and mid-work)",
      );
    }
    if (!state.sessionActive && state.awaitingUserInput) {
      violations.push(
        "awaitingUserInput=true && sessionActive=false (park signal without a live session)",
      );
    }
    if (!state.sessionActive && state.isStreaming && !state.sessionLaunching) {
      // `sessionLaunching` is the legitimate "we're spawning the
      // orchestrator now" window where pendingAutoLaunch is set but
      // activePump hasn't attached yet -- isStreaming reflects that,
      // sessionActive doesn't, and both are correct. The violation
      // only fires for cases where the same shape appears without a
      // launch in flight.
      violations.push(
        "isStreaming=true && sessionActive=false (streaming without a live session)",
      );
    }
    if (violations.length === 0) {
      return;
    }
    console.warn(
      `sim-flow: chat-panel state invariant violation: ${violations.join("; ")}`,
      {
        projectDir: state.projectDir,
        currentStep: state.currentStep,
        currentStepMode: state.currentStepMode,
        awaitingUserInput: state.awaitingUserInput,
        isStreaming: state.isStreaming,
        sessionActive: state.sessionActive,
        isViewer: state.isViewer,
      },
    );
  }

  private async post(message: HostMessage): Promise<void> {
    await this.enqueuePost(async () => {
      await this.view?.webview.postMessage(message);
    });
  }

  /**
   * Resolve the step a transcript entry should be tagged with when
   * it's appended. Preference order: the pump's currently-open
   * sub-session bracket (the bubble was produced inside that step's
   * Work / Critique session), then the orchestrator's overall
   * `current_step` from `state.toml` (idle entries appended between
   * brackets, e.g. user prompts the user typed while parked). Returns
   * `undefined` when neither source has a value -- the panel
   * renders such entries ungrouped at the top of the transcript.
   */
  private transcriptStepFor(
    session: ManagedAutoSessionState,
    context?: PanelContext,
  ): string | undefined {
    return (
      session.pump.subSessionStep ?? context?.currentStep ?? undefined
    );
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
      undefined,
      this.transcriptStepFor(session, context),
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
      // Parked: the orchestrator is idle waiting for input. The
      // last tool / artifact note from the just-completed turn is
      // no longer current; clear so the dashboard doesn't pin
      // "Tool: write_file" while the user is deciding what to do
      // next. (Phase still reflects where we parked, which is
      // useful context.)
      session.currentTool = null;
      session.currentArtifact = null;
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
      llmRequest: (session, args) => {
        if (!isExperimentalChatPanel()) {
          // Standard panel doesn't render the running prompt stack;
          // drop the event so we don't pollute its transcript.
          return;
        }
        this.appendOrchestratorLlmRequest(session, args);
      },
      assistantTurn: (session, args) => {
        if (!isExperimentalChatPanel()) {
          // Standard panel relies on the legacy markdown chunking
          // path; route the prose back through it so behavior is
          // unchanged for users who haven't opted into the
          // experimental UI. Tool-only turns (text === "") fall on
          // the floor here -- the standard panel never showed them
          // either.
          if (args.text.length > 0) {
            this.appendPumpMarkdown(session, args.text);
          }
          return;
        }
        this.appendOrchestratorAssistantTurn(session, args);
      },
      assistantReasoning: (session, args) => {
        // Reasoning is an experimental-panel-only render surface --
        // the standard panel has no collapsed "thinking" block, so
        // we drop deltas there. Within the experimental panel we
        // need the same placeholder discipline the prose path uses:
        // open a placeholder on the first non-empty delta so
        // subsequent reasoning chunks have an `assistantId` to
        // attach to (the prose stream may not have started yet --
        // reasoning often precedes visible text on qwen3.6).
        if (!isExperimentalChatPanel()) {
          return;
        }
        this.appendOrchestratorAssistantReasoning(session, args);
      },
      settled: async (session, result) => {
        await this.onManagedSessionSettled(session, result);
      },
    };
  }

  /**
   * Experimental: assistant-reasoning renderer. Each delta appends
   * to the current assistant turn's `reasoning` field; the close
   * event (`final_chunk: true`, empty text) flips the entry's
   * `reasoningStreaming` to false so the webview can clear the
   * "thinking..." indicator on the collapsed `<details>` block.
   *
   * If reasoning arrives before any prose has streamed (typical
   * for qwen3.6 -- the model thinks first, then answers), this
   * opens the placeholder so the reasoning has a bubble to attach
   * to. The subsequent prose stream then writes into the same
   * placeholder's `body`.
   */
  private appendOrchestratorAssistantReasoning(
    session: ManagedAutoSessionState,
    args: { text: string; finalChunk: boolean },
  ): void {
    if (!this.activePump || this.activePump !== session) {
      return;
    }
    let conversation = this.readConversation(session.projectDir);
    // Open the placeholder if reasoning arrives before prose. The
    // prose path will reuse the same `session.assistantId`.
    if (!session.assistantId && args.text.length > 0) {
      const started = appendAssistantPlaceholder(
        conversation,
        "Assistant",
        "orchestrator",
        undefined,
        this.transcriptStepFor(session),
      );
      session.assistantId = started.assistantId;
      conversation = started.state;
    }
    if (args.text.length > 0 && session.assistantId) {
      conversation = appendAssistantReasoningChunk(
        conversation,
        session.assistantId,
        args.text,
      );
    }
    if (args.finalChunk && session.assistantId) {
      conversation = completeAssistantReasoning(conversation, session.assistantId);
    }
    this.rememberConversation(session.projectDir, conversation);
    void this.postStateForProject(session.projectDir, conversation);
  }

  /**
   * Experimental: render each orchestrator-emitted prompt-stack
   * message as its own user-role bubble in the transcript. The
   * orchestrator filters out System messages on its end, so we
   * receive User and Tool roles here.
   */
  private appendOrchestratorLlmRequest(
    session: ManagedAutoSessionState,
    args: {
      role: string;
      content: string;
      turnIndex: number;
      requestId: string;
      messageId: string | null;
    },
  ): void {
    if (!this.activePump || this.activePump !== session) {
      return;
    }
    const title = labelForLlmRole(args.role);
    const meta = `orchestrator-${args.role}`;
    // Tool results, user prompts, and the system prompt all render
    // inside a collapsible <details> in the webview with the role
    // label as the <summary>; drop the inline prefix so the same
    // text isn't shown twice. Any other non-user role keeps the
    // prefix (currently unreachable -- the orchestrator only emits
    // user / tool / system -- but defensively retained).
    const body =
      args.role === "user" || args.role === "tool" || args.role === "system"
        ? args.content
        : `**${title}:**\n\n${args.content}`;
    let conversation = this.readConversation(session.projectDir);
    const { state: next } = appendOrchestratorUserEntry(
      conversation,
      body,
      title,
      meta,
      args.messageId,
      this.transcriptStepFor(session),
    );
    conversation = next;
    this.rememberConversation(session.projectDir, conversation);
    void this.postStateForProject(session.projectDir, conversation);
  }

  /**
   * Experimental: assistant-turn renderer that handles BOTH the
   * streaming chunk path AND the legacy whole-turn path.
   *
   * The orchestrator emits incremental `AssistantText { final_chunk:
   * false, text: <delta> }` events as the LLM streams its response,
   * followed by one `AssistantText { final_chunk: true, text: "",
   * tool_calls: [...] }` to close the turn. To render those as a
   * single live-updating bubble (instead of one bubble per delta),
   * we use a placeholder-and-chunk pattern:
   *
   *   - First chunk creates the placeholder via
   *     `appendAssistantPlaceholder` and records its id on
   *     `session.assistantId`.
   *   - Subsequent chunks append text via `appendAssistantChunk`,
   *     keyed off `session.assistantId`.
   *   - The final chunk (`finalChunk: true`) appends any tool-call
   *     description as a trailing block via `appendAssistantChunk`
   *     (so the user sees what the model decided to call) and then
   *     `completeAssistantTurn` flips `streaming: false`. The
   *     `assistantId` cursor is cleared so the next turn starts a
   *     fresh placeholder.
   *
   * Cancel mid-stream: when the orchestrator emits
   * `SessionEnd::Cancelled` after a partial response, the existing
   * `onManagedSessionSettled` path calls `completeAssistantTurn`
   * with the placeholder's current body -- which is whatever
   * chunks have streamed so far. That preserves the partial reply
   * in the transcript instead of replacing it with "No response
   * received."
   */
  private appendOrchestratorAssistantTurn(
    session: ManagedAutoSessionState,
    args: {
      text: string;
      finalChunk: boolean;
      toolCalls: Array<{ id?: string; name: string; argumentsJson: string }>;
    },
  ): void {
    if (!this.activePump || this.activePump !== session) {
      return;
    }
    // Drop entirely-empty events that aren't terminal markers --
    // there's no content to render and no transition to record.
    if (args.text.length === 0 && args.toolCalls.length === 0 && !args.finalChunk) {
      return;
    }
    let conversation = this.readConversation(session.projectDir);
    // Open a placeholder on the first chunk with content (or on a
    // tool-call-only final chunk so the description still gets a
    // bubble). The cursor lives on `session.assistantId` so that
    // `onManagedSessionSettled` -> `completeAssistantTurn` can
    // finalize the bubble even if the session ends mid-stream.
    if (!session.assistantId && (args.text.length > 0 || args.toolCalls.length > 0)) {
      const started = appendAssistantPlaceholder(
        conversation,
        "Assistant",
        "orchestrator",
        undefined,
        this.transcriptStepFor(session),
      );
      session.assistantId = started.assistantId;
      conversation = started.state;
    }
    if (args.text.length > 0 && session.assistantId) {
      conversation = appendAssistantChunk(
        conversation,
        session.assistantId,
        args.text,
      );
    }
    if (args.finalChunk) {
      // Tool-call lines append AFTER the prose so the model's
      // "what it decided to do" follows its "why". Skip when the
      // turn was prose-only.
      if (args.toolCalls.length > 0 && session.assistantId) {
        const toolCallBody = formatAssistantTurnBody("", args.toolCalls);
        if (toolCallBody.length > 0) {
          // formatAssistantTurnBody already adds the leading "\n\n"
          // separator only when there's preceding text; we always
          // want one here since the placeholder may have collected
          // prose. Re-prefix unconditionally.
          conversation = appendAssistantChunk(
            conversation,
            session.assistantId,
            `\n\n${toolCallBody}`,
          );
        }
      }
      // Flip streaming = false. If the body is still empty
      // (tool-only turn with no text, or an empty close after a
      // cancel), the fallback text is "No response received." --
      // for tool-only turns that's not ideal, but the tool calls
      // rendered above carry the meaningful content. Improving
      // the fallback for tool-only turns is a follow-up.
      if (session.assistantId) {
        conversation = completeAssistantTurn(conversation, session.assistantId);
        session.assistantId = null;
      }
    }
    this.rememberConversation(session.projectDir, conversation);
    void this.postStateForProject(session.projectDir, conversation);
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
      // Phase boundaries reset the per-turn tool / artifact context.
      session.currentTool = null;
      session.currentArtifact = null;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "phase-changed") {
      session.currentPhase = classified.currentPhase;
      session.currentTool = null;
      session.currentArtifact = null;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "tool-activity") {
      session.currentTool = classified.summary;
      // A tool just completed; any stale artifact-write note from
      // an earlier turn is no longer the live action.
      session.currentArtifact = null;
      void this.postStateForProject(session.projectDir, conversation);
      return;
    }
    if (classified.kind === "artifact-activity") {
      session.currentArtifact = classified.summary;
      session.currentTool = null;
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
        this.transcriptStepFor(session),
      );
      session.assistantId = started.assistantId;
      session.pendingRequestTokensEstimate = null;
      conversation = started.state;
    }
    // Any assistant chunk arriving while a tool / artifact note is
    // pinned means the LLM has resumed streaming after the tool ran
    // -- the tool pill is no longer the live action. Without this
    // clear, the bottom-of-panel indicator and header pill keep
    // showing "Tool: read_file" while the LLM is actually streaming
    // the next response. (Cleared regardless of whether the
    // assistant placeholder is fresh -- mid-session tool calls
    // don't tear down the placeholder, so the "first chunk only"
    // clear from earlier wasn't enough.)
    if (session.currentTool !== null || session.currentArtifact !== null) {
      session.currentTool = null;
      session.currentArtifact = null;
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
    const flowState = await readFlowStateSafe(projectDir);
    const currentStep = flowState?.current_step ?? null;
    const gates = flowState?.gates ?? {};
    return {
      ...base,
      projectDir,
      projectLabel: path.basename(projectDir),
      currentStep,
      gates,
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
    // Prefer the remembered project (LAST_PROJECT_KEY) over whatever
    // `resolveProjectDirForPanel` infers from the workspace context.
    // The user's intent is "reattach to the chat panel I had open
    // before the window reload"; that's the remembered project, not
    // whichever sim-flow project happens to win the workspace scan
    // first. Falls back to the workspace project only when no
    // remembered project exists yet (very first chat-panel use).
    const remembered = this.workspaceState.get<string>(
      ChatPanelProvider.LAST_PROJECT_KEY,
    );
    const projectDir = remembered ?? (await resolveProjectDirForPanel());
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
    const config = vscode.workspace.getConfiguration("sim-flow");
    const reconnectLlm = buildReconnectableLlmConfig(
      ctx,
      this.secrets,
      config,
      record,
    );
    const pump = new SocketSessionPump(
      {
        sessionId: record.sessionId,
        socketPath: record.socketPath,
      },
      reconnectLlm,
    );
    try {
      await pump.ready();
    } catch {
      // The orchestrator child that owned this session is gone (the
      // common case after Developer: Reload Window kills the
      // extension host's children). Forget the dead record and
      // immediately launch a fresh pump for the same project in
      // manual mode -- preserves the user's project anchor and keeps
      // them in the same workflow without making them click
      // "Start session" again. `preserveConversation` keeps the
      // chat transcript from the prior session visible; the new
      // orchestrator's startup note is appended to it.
      //
      // Also clear the per-project pump lock if it still claims
      // this dead session. macOS recycles the extension-host pid
      // after a reload, so the lock's `isProcessAlive` check sees
      // some unrelated Code Helper utility process on that pid and
      // refuses to reclaim -- the new orchestrator would fail to
      // bind and `pump.ready()` would throw on the relaunch too,
      // bouncing the user back to "Start session" forever. Matching
      // on the lock's `sessionId` is race-free: only the now-dead
      // session knew that id, so we're not stealing a fresh
      // sibling's lock.
      clearStalePumpLockForSession(projectDir, record.sessionId);
      await this.autoSessions.forgetStoredRecord(projectDir);
      // Set the relaunch anchor synchronously so this refresh's
      // postState anchors the panel to the right project and renders
      // the "Launching…" indicator. The void launchAutoSession below
      // will clear the anchor in its `finally` so we never leak
      // the marker into a stuck state.
      this.pendingRelaunchAnchor = projectDir;
      void this.launchAutoSession(undefined, projectDir, {
        forceStepMode: "manual",
        preserveConversation: true,
        skipLaunchNote: true,
      }).finally(() => {
        if (this.pendingRelaunchAnchor === projectDir) {
          this.pendingRelaunchAnchor = null;
        }
      });
      return;
    }
    await this.persistConversation(projectDir, clearConversationState());
    await this.autoSessions.attach(
      {
        ...record,
        sourceTag: reconnectLlm.source as LlmSourceTag,
        model: reconnectLlm.model ?? "",
      },
      pump,
      this.autoSessionDelegate(),
    );
    void this.kickContextWindowQuery(projectDir, reconnectLlm);
  }

  /**
   * Best-effort: query the backend for its actual context window
   * once the session has attached, then stash the result on the
   * active session + refresh so the chat-panel pie picks it up.
   * Errors and missing-field responses both resolve to no-op
   * (the pie keeps its cosmetic 128k default). Anthropic is
   * skipped here because it needs an API key the orchestrator
   * already pulled but we haven't surfaced into this code path;
   * adding it later is an additive change.
   */
  private async kickContextWindowQuery(
    projectDir: string,
    llmConfig: PumpLlmConfig,
  ): Promise<void> {
    const model = (llmConfig.model ?? "").trim();
    if (model.length === 0) {
      return;
    }
    const source = llmConfig.source as LlmSourceTag;
    const baseUrl =
      llmConfig.baseUrl ??
      (source === "lmstudio"
        ? llmConfig.lmstudioBaseUrl
        : source === "ollama"
          ? llmConfig.ollamaBaseUrl
          : undefined);
    const tokens = await queryContextWindow({
      source,
      baseUrl,
      model,
    });
    if (tokens === null) {
      return;
    }
    const session = this.activePump;
    if (!session || session.projectDir !== projectDir) {
      // The user disconnected / switched projects while the query
      // was in flight. Drop the result.
      return;
    }
    session.contextWindow = tokens;
    void this.refresh();
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
    const shouldReconcileProjectSwitch = this.projectSwitchPending;
    this.projectSwitchPending = false;
    const requestedProjectDir = await resolveProjectDirForPanel();
    const settings = readPanelSettings();

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
      shouldReconcileProjectSwitch &&
      requestedProjectDir !== this.activePump.projectDir
    ) {
      // Only treat this as a real project switch when the active
      // editor itself resolves to a DIFFERENT sim-flow project.
      // `resolveProjectDirForPanel` falls back to
      // `findProjectCandidates()[0]` when the active editor is
      // undefined (e.g. the user just clicked into the chat-panel
      // webview, which makes `vscode.window.activeTextEditor` go
      // null). In a workspace with multiple sim-flow projects that
      // fallback can resolve to a project OTHER than the active
      // pump's, and the kill-the-pump branch below would tear down
      // a healthy session every time the user clicked into the chat
      // composer to type. Guard on the direct editor resolution
      // (`resolveProjectDir`) so the "switch" verb requires the
      // user to actually be inside a different project's file.
      const directProjectDir = resolveProjectDir();
      if (
        directProjectDir &&
        directProjectDir !== this.activePump.projectDir
      ) {
        await this.stopActivePumpSession(
          this.activePump,
          "Project switched",
          `Stopped the running sim-flow session because the active project changed to \`${path.basename(directProjectDir)}\`.`,
        );
        return;
      }
      // No definitive switch -- keep the existing session and let
      // `requestedProjectDir`'s panel-anchor fallback resolve to the
      // active pump on the way through `readPanelContext`. The
      // pre-existing fallthrough below handles the LLM-source-
      // changed case the same way it did before.
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

  /**
   * Coalescing guard for the reconnect prompt: the user can change
   * several settings in quick succession (or save a JSON edit that
   * touches multiple keys at once), each of which fires its own
   * `onDidChangeConfiguration` event. Without this guard we'd queue a
   * dialog per event and the user would get an avalanche of identical
   * prompts. The flag clears once the current prompt is dismissed.
   */
  private reconnectPromptInFlight = false;

  /**
   * When the user edits an LLM-related setting while an orchestrator
   * is running, the live child process keeps its old argv and stays
   * bound to the previous values. Surface a one-click "Reconnect"
   * prompt so the new settings actually take effect. No-op when
   * nothing's connected, when the current session is a read-only
   * viewer (we don't own it), or when a previous prompt is still on
   * screen.
   */
  /**
   * Push `sim-flow.coverage.*` workspace-config values into the
   * anchored project's `.sim-flow/config.toml`. The Rust orchestrator
   * reads coverage from the TOML, not from VS Code config, so the
   * webview-side setting only takes effect after this writeback.
   *
   * No-op when no project is anchored -- the next time a session
   * opens, the orchestrator falls back to whatever the existing
   * config.toml says.
   */
  private async pushCoverageSettingToActiveProject(): Promise<void> {
    const projectDir =
      this.activePump?.projectDir ?? this.pendingAutoLaunch?.projectDir;
    if (!projectDir) {
      return;
    }
    const cfg = vscode.workspace.getConfiguration("sim-flow");
    const thresholdPct = cfg.get<number>(
      "coverage.thresholdPct",
      COVERAGE_DEFAULTS.thresholdPct,
    );
    const rawLevel = cfg.get<string>("coverage.level", COVERAGE_DEFAULTS.level);
    const level: "module" | "total" =
      rawLevel === "module" ? "module" : "total";
    try {
      await writeCoverageSettings(projectDir, { thresholdPct, level });
    } catch (err) {
      void vscode.window.showWarningMessage(
        `sim-flow: failed to push coverage settings to ${projectDir}: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  /**
   * Push `sim-flow.llm.maxParallelRequests` into the anchored
   * project's `.sim-flow/config.toml::[llm]`. The Rust orchestrator
   * reads this value when constructing `AutoOptions`; the
   * webview-side setting only takes effect after this writeback +
   * a new session start (existing sessions keep their old argv).
   */
  private async pushLlmSettingToActiveProject(): Promise<void> {
    const projectDir =
      this.activePump?.projectDir ?? this.pendingAutoLaunch?.projectDir;
    if (!projectDir) {
      return;
    }
    const cfg = vscode.workspace.getConfiguration("sim-flow");
    const maxParallelRequests = cfg.get<number>(
      "llm.maxParallelRequests",
      LLM_DEFAULTS.maxParallelRequests,
    );
    try {
      await writeLlmSettings(projectDir, { maxParallelRequests });
    } catch (err) {
      void vscode.window.showWarningMessage(
        `sim-flow: failed to push LLM settings to ${projectDir}: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  private async promptReconnectIfLive(reason: string): Promise<void> {
    if (this.reconnectPromptInFlight) {
      return;
    }
    const active = this.autoSessions.getActiveSession();
    if (!active || active.pump.isViewer) {
      return;
    }
    this.reconnectPromptInFlight = true;
    try {
      const choice = await vscode.window.showInformationMessage(
        `sim-flow: ${reason} Reconnect to apply the new settings.`,
        { modal: false },
        "Reconnect",
      );
      // Re-check `active` under the same project -- the user may
      // have disconnected manually between the change firing and
      // their click.
      const stillActive = this.autoSessions.getActiveSession();
      if (
        choice !== "Reconnect" ||
        !stillActive ||
        stillActive.projectDir !== active.projectDir
      ) {
        return;
      }
      await this.reconnectActivePump(stillActive);
    } finally {
      this.reconnectPromptInFlight = false;
    }
  }

  /**
   * Stop the running orchestrator and re-launch it with the same
   * session shape (auto / step) and the same launch parameters
   * (spec path / step ref). The fresh process reads the now-current
   * `sim-flow.llm.*` configuration via `buildPumpLlmConfig`.
   */
  private async reconnectActivePump(
    session: ManagedAutoSessionState,
    options: { forceStepMode?: "auto" | "manual" } = {},
  ): Promise<void> {
    const projectDir = session.projectDir;
    const sessionMode = session.sessionMode;
    const stepRef = session.stepRef;
    const launchSpecPath = session.launchSpecPath;
    await this.stopActivePumpSession(
      session,
      "Reconnecting",
      "Stopped the running sim-flow session to apply the updated LLM settings.",
    );
    if (sessionMode === "auto") {
      await this.launchAutoSession(launchSpecPath, projectDir, {
        forceStepMode: options.forceStepMode,
      });
      return;
    }
    if (sessionMode === "step" && stepRef) {
      await this.launchStepSession(stepRef.step, stepRef.kind, projectDir);
    }
  }

  private queueConversationWrite(task: () => Promise<void>): Promise<void> {
    const write = this.pendingConversationWrites
      .catch(() => undefined)
      .then(task);
    this.pendingConversationWrites = write.catch(() => undefined);
    return write;
  }

  private async waitForPendingConversationWrites(): Promise<void> {
    await this.pendingConversationWrites;
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
    const experimental =
      vscode.workspace
        .getConfiguration("sim-flow")
        .get<boolean>("dashboard.experimentalUi") === true;
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(
        this.extensionUri,
        "dist",
        "webview",
        "chatPanel",
        experimental ? "panelExperimental.js" : "panel.js",
      ),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(
        this.extensionUri,
        "media",
        experimental ? "chat-panel-experimental.css" : "chat-panel.css",
      ),
    );
    const codiconUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "media", "codicons", "codicon.css"),
    );
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} data:`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      // `'wasm-unsafe-eval'` lets Shiki instantiate its oniguruma
      // WebAssembly regex engine. Without it, `createHighlighter`
      // rejects and code blocks render without syntax colours.
      `script-src 'nonce-${nonce}' 'wasm-unsafe-eval'`,
      `font-src ${webview.cspSource}`,
    ].join("; ");

    return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="stylesheet" href="${codiconUri}" />
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
  /**
   * Flow declared by the anchored project's `state.toml`. `null` when
   * no project is anchored or its state file is missing. Surfaced to
   * the webview so the step rail can pick the correct ordering.
   */
  flow: import("../cli/types").Flow | null;
  /**
   * Gate map from `state.toml`. Used by `buildState` to decide
   * whether a critique has earned an advance (or still needs a
   * gate run). Empty when no project is anchored.
   */
  gates: Record<string, { passed?: boolean }>;
  /**
   * Milestone the orchestrator is presently working on, plus the
   * specific pending task within it. Null when the current step
   * has no plan (DM0/DM1/DM2a/DM2b) or no pending task remains.
   */
  currentMilestone: {
    title: string;
    task: string;
    taskIndex: number | null;
    taskTotal: number | null;
  } | null;
  /** Resolved backend kind. `server:<name>` references already
   *  mapped to the entry's `kind`. */
  source: LlmSourceTag;
  /** Raw `sim-flow.llm.source` value (e.g. `server:vllm-local`). */
  rawSource: string;
  /** Resolved base URL when the source maps to a custom server. */
  baseUrl: string | undefined;
  /** Effective model-family override after server-specific resolution. */
  modelFamilyId: string | undefined;
  /** Effective runtime-profile override after server-specific resolution. */
  runtimeProfileId: string | undefined;
  /** True when `rawSource` claims `server:<name>` with no entry. */
  unresolvedServer: boolean;
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

/**
 * Read the persisted `sim-flow.flow.stepMode` setting and clamp to a
 * known value. The orchestrator's CLI rejects anything else, so a
 * stale workspace setting from an older extension version that wrote
 * a different string would fail-fast at launch. Defaulting to
 * `manual` matches the package.json schema default.
 */
function readStepModeSetting(config: vscode.WorkspaceConfiguration): "auto" | "manual" {
  const raw = (config.get<string>("flow.stepMode") ?? "manual").trim();
  return raw === "auto" ? "auto" : "manual";
}

function readPanelSettings(): {
  /** Resolved source kind. For `server:<name>` references, the
   *  matching entry's `kind`; otherwise the raw built-in value. */
  source: LlmSourceTag;
  /** Raw `sim-flow.llm.source` value. Kept for display + the
   *  `unresolved server` error path. */
  rawSource: string;
  /** When `rawSource` is `server:<name>` and the entry exists,
   *  the composed `host:port/v1` URL the agent should hit.
   *  Undefined for built-in sources. */
  baseUrl: string | undefined;
  /** Effective model-family override after server-specific resolution. */
  modelFamilyId: string | undefined;
  /** Effective runtime-profile override after server-specific resolution. */
  runtimeProfileId: string | undefined;
  /** True when `rawSource` claims `server:<name>` but no entry
   *  matches. Callers should surface a clear error rather than
   *  silently falling back to a default. */
  unresolvedServer: boolean;
  sourceLabel: string;
  model: string;
  verbose: boolean;
  ollamaBaseUrl: string;
  lmstudioBaseUrl: string;
  servers: LlmServerEntry[];
} {
  const config = vscode.workspace.getConfiguration("sim-flow");
  const rawSource = (config.get<string>("llm.source") ?? "vscode") as string;
  const globalModelFamilyId = (config.get<string>("llm.modelFamily") ?? "").trim() || undefined;
  const globalRuntimeProfileId =
    (config.get<string>("llm.runtimeProfile") ?? "").trim() || undefined;
  const servers =
    (config.get<unknown>("llm.servers") as LlmServerEntry[] | undefined) ?? [];
  const resolved = resolveLlmSource(rawSource, servers);
  const fallback: LlmSourceTag = "vscode";
  if (resolved === null) {
    return {
      source: fallback,
      rawSource,
      baseUrl: undefined,
      modelFamilyId: globalModelFamilyId,
      runtimeProfileId: globalRuntimeProfileId,
      unresolvedServer: true,
      sourceLabel: rawSource,
      model: (config.get<string>("llm.model") ?? "").trim(),
      verbose: config.get<boolean>("llm.verbose") ?? true,
      ollamaBaseUrl: (config.get<string>("llm.ollama.baseUrl") ?? "").trim(),
      lmstudioBaseUrl: (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim(),
      servers,
    };
  }
  const source = resolved.source as LlmSourceTag;
  const modelOverride = resolved.model && resolved.model.length > 0 ? resolved.model : null;
  return {
    source,
    rawSource,
    baseUrl: resolved.baseUrl,
    modelFamilyId: resolved.modelFamilyId ?? globalModelFamilyId,
    runtimeProfileId: resolved.runtimeProfileId ?? globalRuntimeProfileId,
    unresolvedServer: false,
    sourceLabel: rawSource.startsWith("server:")
      ? rawSource
      : (LLM_SOURCE_LABELS[source] ?? source),
    model: modelOverride ?? (config.get<string>("llm.model") ?? "").trim(),
    verbose: config.get<boolean>("llm.verbose") ?? true,
    ollamaBaseUrl: (config.get<string>("llm.ollama.baseUrl") ?? "").trim(),
    lmstudioBaseUrl: (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim(),
    servers,
  };
}

function normalizeSpecPath(specPath: string | undefined): string | undefined {
  const trimmed = specPath?.trim() ?? "";
  return trimmed.length > 0 ? trimmed : undefined;
}

async function resolveProjectDirForPanel(): Promise<string | null> {
  const direct = resolveProjectDir();
  if (direct) {
    return direct;
  }
  const candidates = await findProjectCandidates();
  return candidates[0] ?? null;
}

/**
 * Modal confirmation for a Reset Step click. Spells out exactly
 * which steps will be discarded and which artifacts the user is
 * about to lose. Returns true iff the user clicks the destructive
 * confirm action.
 */
async function confirmReset(
  targets: readonly string[],
  pickedStep: string,
): Promise<boolean> {
  const list = targets.join(", ");
  const detail =
    targets.length === 1
      ? `Resetting \`${pickedStep}\` will permanently delete the work artifacts, critique notes, and gate flag for that step.`
      : `Resetting from \`${pickedStep}\` will permanently delete the work artifacts, critique notes, and gate flags for: ${list}. The orchestrator will return to \`${pickedStep}\` and the listed later steps will need to be re-run from scratch.`;
  const choice = await vscode.window.showWarningMessage(
    targets.length === 1
      ? `Reset step \`${pickedStep}\`?`
      : `Reset from step \`${pickedStep}\`?`,
    {
      modal: true,
      detail,
    },
    "Reset",
  );
  return choice === "Reset";
}

/**
 * Read the full FlowState for a project, swallowing IO/parse errors.
 * Null on any failure so callers fall back to empty defaults rather
 * than crashing the panel mid-render.
 */
async function readFlowStateSafe(
  projectDir: string,
): Promise<FlowState | null> {
  try {
    return await readFlowState(projectDir);
  } catch {
    return null;
  }
}

/**
 * Find the milestone the orchestrator is presently working on plus
 * the first pending task within it, for the line under the step rail.
 * Returns null when the step has no plan, the plan directory hasn't
 * been built yet, or every milestone is complete. IO/parse failures
 * also resolve to null so a missing plan doesn't crash the chat panel.
 */
async function readCurrentMilestoneSafe(
  projectDir: string,
  currentStep: string,
): Promise<{
  title: string;
  task: string;
  taskIndex: number | null;
  taskTotal: number | null;
} | null> {
  try {
    const progress = await readPlanProgress(projectDir, currentStep);
    if (progress.kind === "none" || progress.currentTask === null) {
      return null;
    }
    const owner = progress.milestones.find(
      (m) => m.filePath === progress.currentTaskFilePath,
    );
    if (!owner) {
      return null;
    }
    return {
      title: owner.title,
      task: progress.currentTask,
      taskIndex: progress.currentTaskIndex,
      taskTotal: progress.currentTaskTotal,
    };
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

function buildPumpLlmConfig(
  ctx: { projectDir: string; cli: { binary: string } },
  secrets: SecretStorage,
  config: vscode.WorkspaceConfiguration,
): PumpLlmConfig {
  const settings = readPanelSettings();
  // Pass the RAW source ("server:<name>" when applicable) so the
  // inner resolver actually looks up the entry's host/port/model.
  // `settings.source` is already the resolved tag (e.g. "vllm"),
  // which would make the inner `resolveLlmSource` a no-op and silently
  // drop the user's custom baseUrl + per-server model overrides.
  return buildResolvedPumpLlmConfig(
    ctx,
    secrets,
    config,
    settings.rawSource as LlmSource,
    settings.model.trim() || undefined,
  );
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
  const servers =
    (config.get<unknown>("llm.servers") as LlmServerEntry[] | undefined) ?? [];
  const resolved = typeof source === "string" ? resolveLlmSource(source, servers) : null;
  const effectiveSource = (resolved?.source ?? source) as LlmSource;
  const effectiveModel = resolved?.model?.trim() || model;
  const globalModelFamilyId = (config.get<string>("llm.modelFamily") ?? "").trim() || undefined;
  const globalRuntimeProfileId =
    (config.get<string>("llm.runtimeProfile") ?? "").trim() || undefined;
  const ollamaBaseUrl = (config.get<string>("llm.ollama.baseUrl") ?? "").trim() || undefined;
  const lmstudioBaseUrl =
    (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim() || undefined;
  const settingTokens = (config.get<string[]>("debug") ?? []).join(",");
  const envTokens = (process.env["SIM_FOUNDATION_DEBUG"] ?? "").trim();
  const debugTokens = settingTokens.length > 0 ? settingTokens : envTokens;
  return {
    source: effectiveSource,
    model: effectiveModel,
    modelFamilyId: resolved?.modelFamilyId ?? globalModelFamilyId,
    runtimeProfileId: resolved?.runtimeProfileId ?? globalRuntimeProfileId,
    baseUrl: resolved?.baseUrl,
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

/**
 * Map an arbitrary `llm_backend` string from a watcher
 * registration onto the closed `LlmSourceTag` enum used by the
 * chat-panel header. Viewers never dispatch LLM calls, so this is
 * cosmetic only -- unrecognized backends fall back to "openai".
 */
function mapBackendToSourceTag(backend: string): LlmSourceTag {
  switch (backend) {
    case "vscode":
    case "anthropic":
    case "openai":
    case "ollama":
    case "lmstudio":
    case "vllm":
    case "claude-cli":
    case "codex-cli":
    case "gh-copilot-cli":
      return backend;
    default:
      return "openai";
  }
}

/** True when the user has opted into the experimental chat panel. */
function isExperimentalChatPanel(): boolean {
  return (
    vscode.workspace
      .getConfiguration("sim-flow")
      .get<boolean>("dashboard.experimentalUi") === true
  );
}

/** Human-readable label for an orchestrator-emitted prompt's role. */
function labelForLlmRole(role: string): string {
  switch (role) {
    case "user":
      return "User";
    case "tool":
      return "Tool result";
    case "system":
      return "System";
    case "assistant":
      return "Assistant (replay)";
    default:
      return role.length > 0
        ? role.slice(0, 1).toUpperCase() + role.slice(1)
        : "Prompt";
  }
}

/**
 * Format an LLM turn's prose + native tool calls into a single
 * markdown body for the experimental chat panel. Prose renders
 * as-is. Tool calls render as a bulleted list of human-readable
 * actions ("Read docs/spec.md") rather than the raw `name` + JSON
 * args block -- the verb form makes a turn's intent legible at a
 * glance, even on tool-heavy turns where there's no prose.
 */
function formatAssistantTurnBody(
  text: string,
  toolCalls: Array<{ id?: string; name: string; argumentsJson: string }>,
): string {
  const parts: string[] = [];
  if (text.length > 0) {
    parts.push(text);
  }
  if (toolCalls.length > 0) {
    const lines = toolCalls.map(
      (c) => `- ${describeToolCall(c.name, c.argumentsJson)}`,
    );
    parts.push(lines.join("\n"));
  }
  return parts.join("\n\n");
}

/**
 * Map a single tool call onto a one-line, verb-first description of
 * what the LLM is asking the orchestrator to do. The arg shapes are
 * pinned to the orchestrator's tool catalog at
 * `tools/sim-flow/src/__internal/session/tools/`; if the wire shape
 * changes, the matching arm below needs an update.
 *
 * Unknown tools fall back to `` `name` `` so the user still sees a
 * marker for the call even when we don't have a pretty form -- better
 * than swallowing it silently.
 */
function describeToolCall(name: string, argumentsJson: string): string {
  const args = parseToolArgs(argumentsJson);
  const path = stringArg(args, "path");
  switch (name) {
    case "read_file":
      return path ? `Read \`${path}\`` : "Read file";
    case "write_file":
      return path ? `Write \`${path}\`` : "Write file";
    case "edit_file":
      return path ? `Edit \`${path}\`` : "Edit file";
    case "delete_file":
      return path ? `Delete \`${path}\`` : "Delete file";
    case "list_dir":
      return path ? `List \`${path}\`` : "List directory";
    case "search": {
      const pattern = stringArg(args, "pattern");
      if (pattern && path) {
        return `Search for \`${pattern}\` in \`${path}\``;
      }
      if (pattern) {
        return `Search for \`${pattern}\``;
      }
      return "Search";
    }
    case "run_cargo": {
      const command = stringArg(args, "command");
      return command ? `Run \`cargo ${command}\`` : "Run cargo";
    }
    case "declare_hypothesis": {
      const rationale = stringArg(args, "rationale");
      return rationale
        ? `Declare hypothesis: ${truncate(rationale, 200)}`
        : "Declare hypothesis";
    }
    case "declare_fix": {
      const rationale = stringArg(args, "rationale");
      return rationale
        ? `Declare fix: ${truncate(rationale, 200)}`
        : "Declare fix";
    }
    case "log_bug": {
      const issue = stringArg(args, "issue");
      const category = stringArg(args, "category");
      if (issue && category) {
        return `Log bug (${category}): ${truncate(issue, 200)}`;
      }
      if (issue) {
        return `Log bug: ${truncate(issue, 200)}`;
      }
      return "Log bug";
    }
    case "resolve_bug": {
      const resolution = stringArg(args, "resolution");
      return resolution
        ? `Resolve bug: ${truncate(resolution, 200)}`
        : "Resolve bug";
    }
    case "record_run": {
      const description = stringArg(args, "description");
      return description ? `Record run \`${description}\`` : "Record run";
    }
    default:
      return `\`${name}\``;
  }
}

function parseToolArgs(argumentsJson: string): Record<string, unknown> {
  try {
    const parsed: unknown = JSON.parse(argumentsJson);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // ignore -- treat as empty so callers fall back to the generic
    // "no path / no args" branches below.
  }
  return {};
}

function stringArg(args: Record<string, unknown>, key: string): string | null {
  const value = args[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function truncate(text: string, max: number): string {
  if (text.length <= max) {
    return text;
  }
  return `${text.slice(0, max - 1)}…`;
}
