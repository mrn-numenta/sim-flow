// Extension-side host for the Flow Dashboard webview. Owns panel
// lifecycle, loads HTML, fulfills webview messages by delegating to
// the CLI wrapper and the state readers, and broadcasts state updates
// on file-watcher events.

import * as vscode from "vscode";

import type { SimFlowCli } from "../cli/simflow";
import { createStateWatcher, type SimFlowStateWatcher } from "../state/watcher";

import { controlSocketLikelyPresent } from "../session/control-client";

import type { AutoSessionManager, ManagedAutoSessionState } from "../chatPanel/autoSessionManager";
import type { StepMode } from "../session/protocol-types";

import { aggregateDashboardState } from "./aggregate";
import {
  resolveLlmSource,
  type DashboardState,
  type HostMessage,
  type LlmSourceTag,
  type WebviewMessage,
} from "./messages";
import { randomNonce, readStepModeSetting } from "./host/helpers";
import {
  loadAllPlanProgress,
  loadBaselines,
  loadCritiques,
  loadDocuments,
  loadFlowState,
  loadPlanProgress,
  loadRuns,
  readCoverageState,
} from "./host/loaders";
import {
  onWebviewMessage as handleWebviewMessage,
  type MessageHandlerContext,
} from "./host/handlers";
import {
  advanceStep as advanceStepAction,
  generateVerilog as generateVerilogAction,
  runAutoEndToEnd as runAutoEndToEndAction,
  sendGateForStep as sendGateForStepAction,
  stopAuto as stopAutoAction,
  tryControlSocketAdvance as tryControlSocketAdvanceAction,
  tryControlSocketReset as tryControlSocketResetAction,
  tryControlSocketRunGate as tryControlSocketRunGateAction,
  tryControlSocketRunStep as tryControlSocketRunStepAction,
  type ActionsContext,
} from "./host/actions";
import {
  openAnalysisFolder as openAnalysisFolderUi,
  openCritiqueInEditor as openCritiqueInEditorUi,
  openDocumentInEditor as openDocumentInEditorUi,
  openPromptInEditor as openPromptInEditorUi,
  pickSpecFile as pickSpecFileUi,
  postBlockDiagram as postBlockDiagramUi,
  regenerateBlockDiagram as regenerateBlockDiagramUi,
  resetPromptOverride as resetPromptOverrideUi,
  sendModelList as sendModelListUi,
  sendPromptsList as sendPromptsListUi,
  type UiContext,
} from "./host/ui";

/** Cap rows streamed to the dashboard so message size stays bounded. */
export const MAX_DASHBOARD_RUNS = 200;

export interface DashboardHostOptions {
  extensionUri: vscode.Uri;
  projectDir: string;
  cli: SimFlowCli;
  /**
   * VS Code per-workspace key/value store. The dashboard uses it to
   * persist UI state that should survive a window reload but doesn't
   * belong in the project's `.sim-flow/` tree -- specifically the
   * spec-path the user typed into the Spec field. Keyed by
   * `sim-flow.specPath.<projectDir>` so each project remembers its
   * own spec independently.
   */
  workspaceState: vscode.Memento;
  /**
   * The chat panel's session registry. The dashboard reads
   * `getActiveSession()` to find the live `SocketSessionPump` for
   * this project — when one exists and is in manual step mode, the
   * per-step buttons (Run Step / Critique / Gate / Advance / Reset)
   * dispatch as `RunStep` / `RunCritique` / `RunGate` / `Advance` /
   * `Reset` host events over the transport socket instead of
   * spawning side `sim-flow session ...` processes.
   *
   * Optional so tests / older entry points that don't construct one
   * still work; absence falls through to the legacy chat-tab path.
   */
  autoSessions?: AutoSessionManager;
}

/**
 * Owns a single Flow Dashboard webview. Subsequent `open()` calls
 * reveal the existing panel rather than creating a new one.
 */
export class DashboardHost {
  private panel: vscode.WebviewPanel | undefined;
  private watcher: SimFlowStateWatcher | undefined;
  private readonly disposables: vscode.Disposable[] = [];
  private refreshing = false;
  private refreshQueued = false;
  /**
   * Debounce window for watcher-driven refreshes. The state watcher
   * fires `onDidChange` for every individual file write under
   * `.sim-flow/` and `docs/critiques/` and `docs/plan/` -- during an
   * active flow run the agent emits dozens of writes per turn
   * (artifact-write, plan checkbox flip, critique JSON, critique
   * markdown render, gate state.toml update) and each one fired a
   * separate `state-update` to the webview, which rebuilt the entire
   * `#app` DOM. Result: hover styles flickered every frame and click
   * events between mousedown and mouseup raced with the rebuild and
   * dropped.
   *
   * `requestWatcherRefresh()` coalesces a burst of file events into
   * a single `refresh()` at the trailing edge of a 200ms quiet
   * window. Button-click paths still call `refresh()` directly so
   * user-initiated UI updates feel immediate.
   */
  private watcherRefreshTimer: ReturnType<typeof setTimeout> | undefined;
  /**
   * Bookkeeping for the active pump's `StepModeChanged` listener.
   * The pump can change between refreshes (Connect → Disconnect →
   * Connect with different settings); we re-subscribe each time
   * `refresh()` runs and `disposeStepModeListener` cleans up the old
   * subscription. `null` means we have no current subscription.
   */
  private stepModeListenerDispose: (() => void) | null = null;
  private stepModeListenerSession: ManagedAutoSessionState | null = null;
  /**
   * Bookkeeping for the active pump's sub-session bracket listener
   * (`onSubSessionChanged`). Same pump-rotation lifecycle as the
   * step-mode listener above. `null` means we have no current
   * subscription.
   */
  private subSessionListenerDispose: (() => void) | null = null;
  private subSessionListenerSession: ManagedAutoSessionState | null = null;
  /**
   * Bookkeeping for the active pump's structured `gate-result`
   * listener. Same lifecycle as the sub-session listener: re-attached
   * on every `refresh()` after pump rotation. Without this, the JSONL
   * Run Gate path's `Event::GateResult` was rendered as chat-panel
   * markdown but never bridged to the dashboard's `gate-result`
   * HostMessage path -- the per-step gate cache stayed stale and the
   * "Run Gate ..." pending entry hung until the 5s failsafe fired.
   */
  private gateResultListenerDispose: (() => void) | null = null;
  private gateResultListenerSession: ManagedAutoSessionState | null = null;
  /**
   * Subscription to `AutoSessionManager.onActiveSessionChanged`, set
   * up at construction so the dashboard reacts to a pump appearing /
   * rotating / disappearing without waiting for a file-watcher tick
   * or viewState change to call `refresh()`. Without this hook, the
   * sub-session and step-mode bus listeners are only attached lazily
   * inside `refresh()`, so the first `sub-session-started` /
   * `-ended` events from a fresh pump can fire before the listener
   * is wired and the dashboard sits at `inSubSession=true` with
   * everything except Reset disabled.
   */
  private activeSessionListenerDispose: (() => void) | null = null;

  constructor(private readonly options: DashboardHostOptions) {
    this.activeSessionListenerDispose =
      this.options.autoSessions?.onActiveSessionChanged(() => {
        // refresh() rebuilds state, syncs the bus listeners against
        // the current pump (or detaches when none), and posts a
        // state-update so the per-step buttons re-evaluate. It's
        // serialized via `refreshing` / `refreshQueued` so back-to-
        // back lifecycle events coalesce.
        void this.refresh();
      }) ?? null;
  }

  /** Show the dashboard, creating it if necessary. */
  async open(column: vscode.ViewColumn = vscode.ViewColumn.Active): Promise<void> {
    if (this.panel) {
      this.panel.reveal(column);
      return;
    }

    const panel = vscode.window.createWebviewPanel(
      "simFlowDashboard",
      "sim-flow Dashboard",
      column,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [
          vscode.Uri.joinPath(this.options.extensionUri, "dist", "webview"),
          vscode.Uri.joinPath(this.options.extensionUri, "media"),
        ],
      },
    );
    this.panel = panel;

    panel.webview.html = await this.renderHtml(panel.webview);

    panel.onDidDispose(() => this.dispose(), null, this.disposables);
    panel.webview.onDidReceiveMessage(
      (msg: WebviewMessage) => {
        void this.onWebviewMessage(msg);
      },
      null,
      this.disposables,
    );
    // Refresh whenever the user brings the panel back into view; the
    // file watcher misses changes to projects outside the open
    // workspace folder, so revealing the panel is our best signal that
    // the user wants the latest state.
    panel.onDidChangeViewState(
      (e) => {
        if (e.webviewPanel.visible) {
          void this.refresh();
        }
      },
      null,
      this.disposables,
    );

    this.attachWatcher();
  }

  dispose(): void {
    if (this.watcherRefreshTimer !== undefined) {
      clearTimeout(this.watcherRefreshTimer);
      this.watcherRefreshTimer = undefined;
    }
    this.watcher?.dispose();
    this.watcher = undefined;
    this.disposeStepModeListener();
    this.disposeSubSessionListener();
    this.disposeGateResultListener();
    if (this.activeSessionListenerDispose) {
      this.activeSessionListenerDispose();
      this.activeSessionListenerDispose = null;
    }
    for (const d of this.disposables) {
      d.dispose();
    }
    this.disposables.length = 0;
    this.panel = undefined;
  }

  private disposeSubSessionListener(): void {
    if (this.subSessionListenerDispose) {
      this.subSessionListenerDispose();
      this.subSessionListenerDispose = null;
    }
    this.subSessionListenerSession = null;
  }

  /**
   * Resubscribe to the live pump's `SubSessionStarted` /
   * `SubSessionEnded` events so the dashboard refreshes whenever the
   * orchestrator transitions in/out of a sub-session. Mirrors the
   * step-mode listener lifecycle above; safe to call when no pump is
   * attached.
   */
  private syncSubSessionListener(): void {
    const session = this.activeSession();
    if (this.subSessionListenerSession === session) {
      return;
    }
    this.disposeSubSessionListener();
    if (!session || typeof session.pump.onSubSessionChanged !== "function") {
      return;
    }
    this.subSessionListenerSession = session;
    this.subSessionListenerDispose = session.pump.onSubSessionChanged(() => {
      // Bracket transition: refresh so the per-step buttons re-evaluate.
      void this.refresh();
    });
  }

  private disposeGateResultListener(): void {
    if (this.gateResultListenerDispose) {
      this.gateResultListenerDispose();
      this.gateResultListenerDispose = null;
    }
    this.gateResultListenerSession = null;
  }

  /**
   * Resubscribe to the live pump's structured `gate-result` events.
   * Mirrors the sub-session listener lifecycle. The listener posts a
   * `gate-result` HostMessage to the webview so the per-step gate
   * cache and pending-action entry settle on the structured result,
   * not just on the chat-panel markdown render.
   */
  private syncGateResultListener(): void {
    const session = this.activeSession();
    if (this.gateResultListenerSession === session) {
      return;
    }
    this.disposeGateResultListener();
    if (!session || typeof session.pump.onGateResult !== "function") {
      return;
    }
    this.gateResultListenerSession = session;
    this.gateResultListenerDispose = session.pump.onGateResult((result) => {
      void this.post({
        type: "gate-result",
        step: result.step,
        result: {
          step: result.step,
          clean: result.clean,
          failures: result.failures,
        },
      });
    });
  }

  private disposeStepModeListener(): void {
    if (this.stepModeListenerDispose) {
      this.stepModeListenerDispose();
      this.stepModeListenerDispose = null;
    }
    this.stepModeListenerSession = null;
  }

  /**
   * Resubscribe to the live pump's `StepModeChanged` events. Called
   * after each `refresh()` because the active pump may have rotated
   * (Connect → Disconnect → Connect with different LLM settings will
   * spin up a new pump). Safe to call when no pump is attached.
   */
  private syncStepModeListener(): void {
    const session = this.activeSession();
    if (this.stepModeListenerSession === session) {
      return; // already subscribed (or already idle)
    }
    this.disposeStepModeListener();
    if (!session || typeof session.pump.onStepModeChanged !== "function") {
      return;
    }
    this.stepModeListenerSession = session;
    this.stepModeListenerDispose = session.pump.onStepModeChanged(() => {
      // The orchestrator's truth changed (user toggle, cap exceeded,
      // gate failure, …). Refresh so the dashboard's toggle UI
      // matches it. Refresh is idempotent and serializes via
      // `refreshing` / `refreshQueued`.
      void this.refresh();
    });
  }

  // -------------------------------------------------------------
  // Message handlers
  // -------------------------------------------------------------

  private async onWebviewMessage(msg: WebviewMessage): Promise<void> {
    // The dispatch context interface (handlers.ts) mirrors this
    // class's private methods exactly; the cast bridges TS's nominal
    // privacy check while preserving structural compatibility at
    // runtime.
    await handleWebviewMessage(this as unknown as MessageHandlerContext, msg);
  }

  /**
   * Toggle change from the dashboard. When a manual-mode pump is live
   * for this project, fire `SetStepMode` over the transport socket so
   * the orchestrator flips its flag and emits `StepModeChanged` (which
   * we'll observe via the pump's listener and refresh the dashboard).
   * When no pump is alive, just persist the setting — the next launch
   * will read it.
   *
   * Either way, persist the setting so it sticks across sessions. The
   * orchestrator's truth wins for the live UI, the setting wins for
   * the next launch.
   */
  async handleSetStepMode(mode: StepMode): Promise<void> {
    await vscode.workspace
      .getConfiguration("sim-flow")
      .update("flow.stepMode", mode, vscode.ConfigurationTarget.Workspace);
    const session = this.activeSession();
    if (session && typeof session.pump.setStepMode === "function") {
      session.pump.setStepMode(mode);
      // The orchestrator will echo `StepModeChanged` and our pump
      // listener will refresh the dashboard once that arrives.
      return;
    }
    // No live session: optimistic refresh so the toggle visually
    // reflects the new persisted value immediately.
    await this.refresh();
  }

  /**
   * Trailing-edge debounced refresh for watcher events. Resets a
   * 200ms timer on every call; the timer fires exactly one
   * `refresh()` once the file-write storm dies down. See the field
   * docstring on `watcherRefreshTimer` for the failure mode this
   * guards against.
   */
  private requestWatcherRefresh(): void {
    if (this.watcherRefreshTimer !== undefined) {
      clearTimeout(this.watcherRefreshTimer);
    }
    this.watcherRefreshTimer = setTimeout(() => {
      this.watcherRefreshTimer = undefined;
      void this.refresh();
    }, 200);
  }

  /**
   * Recompute the dashboard state from disk + CLI and post an update
   * to the webview. Serialized so concurrent file-change events
   * coalesce into a single refresh.
   */
  async refresh(): Promise<void> {
    if (!this.panel) {
      return;
    }
    if (this.refreshing) {
      this.refreshQueued = true;
      return;
    }
    this.refreshing = true;
    try {
      const state = await this.buildState();
      // Re-subscribe to the (possibly rotated) pump's
      // `StepModeChanged` and sub-session bracket channels so the
      // toggle and the per-step buttons reflect the orchestrator's
      // truth between refreshes.
      this.syncStepModeListener();
      this.syncSubSessionListener();
      this.syncGateResultListener();
      await this.post({ type: "state-update", state });
      await this.postLlmConfig();
      await this.postBlockDiagram();
    } catch (err) {
      await this.post({
        type: "error",
        message: "Failed to load dashboard state",
        detail: String((err as Error).message ?? err),
      });
    } finally {
      this.refreshing = false;
      if (this.refreshQueued) {
        this.refreshQueued = false;
        void this.refresh();
      }
    }
  }

  /**
   * If a sim-flow single-session driver is running for this project
   * (control socket present + reachable), forward a Run Step / Run
   * Critique button click to it as an `inject` command. The running
   * `claude` immediately sees the step's prompt without us launching
   * a fresh chat tab. Returns `true` when the socket handled the
   * action; `false` when the caller should fall through to the
   * legacy chat-pane path.
   */
  /**
   * Stop button: send `/exit` over the control socket so the running
   * claude TUI receives the same input the user would type to leave
   * the session. The orchestrator sees the agent exit and parks at
   * "waiting for the next dashboard command" -- the next Run/Resume
   * (or Run Step) respawns claude. If no socket is present (no
   * single-session sim-flow auto running), surface a friendly
   * notice rather than failing silently.
   */
  /**
   * End-to-end automated flow: walks DM0 → DM4b without manual
   * intervention, retrying critique blockers up to the configured
   * cap. Requires a spec because the agent has no way to derive
   * spec.md from thin air in unattended mode (and `--dm0-interactive`
   * would defeat the point). Routes by `sim-flow.llm.source`:
   * CLI agents (claude-cli / codex-cli / gh-copilot-cli) launch in
   * a per-step terminal session where the orchestrator auto-
   * advances; non-CLI sources route through the chat-pane `/auto`
   * participant which already drives the full work / critique /
   * advance loop via the JSONL host. Confirmation modal up front
   * because the run can take a long time and burn meaningful
   * LLM credits.
   */
  async runAutoEndToEnd(specPath: string): Promise<void> {
    await runAutoEndToEndAction(this as unknown as ActionsContext, specPath);
  }

  async generateVerilog(): Promise<void> {
    await generateVerilogAction(this as unknown as ActionsContext);
  }

  async stopAuto(): Promise<void> {
    await stopAutoAction(this as unknown as ActionsContext);
  }

  async tryControlSocketRunStep(step: string, kind: "work" | "critique"): Promise<boolean> {
    return tryControlSocketRunStepAction(this as unknown as ActionsContext, step, kind);
  }

  async tryControlSocketRunGate(step: string): Promise<boolean> {
    return tryControlSocketRunGateAction(this as unknown as ActionsContext, step);
  }

  async tryControlSocketAdvance(step: string): Promise<boolean> {
    return tryControlSocketAdvanceAction(this as unknown as ActionsContext, step);
  }

  async tryControlSocketReset(step: string): Promise<boolean> {
    return tryControlSocketResetAction(this as unknown as ActionsContext, step);
  }

  async sendGateForStep(step: string): Promise<void> {
    await sendGateForStepAction(this as unknown as ActionsContext, step);
  }

  async advanceStep(step: string): Promise<void> {
    await advanceStepAction(this as unknown as ActionsContext, step);
  }

  // -------------------------------------------------------------
  // State aggregation
  // -------------------------------------------------------------

  private async buildState(): Promise<DashboardState> {
    const [flow, critiques, runs, baselines] = await Promise.all([
      loadFlowState(this.options.cli),
      loadCritiques(this.options.cli),
      loadRuns(this.options.cli),
      loadBaselines(this.options.cli),
    ]);
    const documents = await loadDocuments(this.options.cli, flow.flow);
    const planProgress = await loadPlanProgress(this.options.cli, flow.current_step);
    // All-kinds progress so the dashboard can show milestone
    // pipelines under any plan-related step (DM2c outline,
    // DM2cd detail, DM2d execution, etc.) regardless of which
    // step is current. Each kind is scanned independently so
    // missing-on-disk plans render as empty boxes rather than
    // hiding the section.
    const planProgressByKind = await loadAllPlanProgress(this.options.cli);
    const specPath = this.readSpecPath();
    // Coverage settings live in the project's `.sim-flow/config.toml`
    // (the orchestrator side reads them too). Read failures fall
    // back to defaults so the dashboard keeps rendering even when
    // the file is missing or malformed.
    const coverage = await readCoverageState(this.options.projectDir);
    const cfg = vscode.workspace.getConfiguration("sim-flow");
    const fullyAutomatedEnabled = cfg.get<boolean>("dashboard.showFullyAutomated") ?? false;
    const verilogSimEnabled = cfg.get<boolean>("dashboard.verilogSimEnabled") ?? false;
    const verilogSimulatorPath = (cfg.get<string>("dashboard.verilogSimulatorPath") ?? "").trim();
    // Resolve the step-axis mode: orchestrator's truth when a pump is
    // attached for this project, otherwise the persisted setting. The
    // pump echoes `StepModeChanged` on connect, so once the dashboard
    // has refreshed after the first echo the toggle matches reality
    // even if the user changed the setting after launch.
    const session = this.activeSession();
    const stepMode: StepMode = session?.pump.stepMode ?? readStepModeSetting(cfg);
    // sessionActive must reflect ANY backend that's currently running
    // for this project, not just AutoSessionManager-registered pumps.
    // CLI sources (claude-cli / codex-cli / gh-copilot-cli) run
    // sim-flow auto in a VS Code terminal with no pump registration;
    // their only signal is the single-session control socket at
    // .sim-flow/control.sock. Without this OR, switching editor tabs
    // back to the dashboard would always show "disconnected" for a
    // live CLI single-session run, and the Connect button would
    // re-enable -- letting the user spawn a duplicate sim-flow.
    const sessionActive = this.isSessionActive();
    // True while the orchestrator is inside a sub-session (Work or
    // Critique). The pump tracks this from the
    // `sub-session-started` / `-ended` bracket events. When no pump
    // is attached we report `false` so the dashboard's "no
    // orchestrator" path applies its own gating.
    const inSubSession = session?.pump.inSubSession ?? false;
    const isViewer = session?.pump.isViewer ?? false;
    return aggregateDashboardState({
      projectDir: this.options.projectDir,
      flow,
      critiques,
      runs,
      baselines,
      documents,
      planProgress,
      planProgressByKind,
      specPath,
      fullyAutomatedEnabled,
      verilogSimEnabled,
      verilogSimulatorPath,
      llmServers: cfg.get<unknown>("llm.servers") as
        | import("./messages").LlmServerEntry[]
        | undefined,
      coverage,
      stepMode,
      sessionActive,
      inSubSession,
      isViewer,
      maxRuns: MAX_DASHBOARD_RUNS,
    });
  }

  /**
   * Active orchestrator pump for the dashboard's project, or
   * undefined when none is attached. Used by the per-step button
   * dispatcher and the step-mode toggle to decide whether to send a
   * host event over the transport socket or fall back to the legacy
   * chat-tab path.
   */
  private activeSession(): ManagedAutoSessionState | undefined {
    const session = this.options.autoSessions?.getActiveSession();
    if (!session || session.projectDir !== this.options.projectDir) {
      return undefined;
    }
    return session;
  }

  /**
   * Single source of truth for "is sim-flow currently running for this
   * project". Returns true when EITHER an AutoSessionManager-registered
   * pump is alive for this project (non-CLI backends) OR the
   * single-session control socket file is present (CLI backends, which
   * run in a terminal and don't register with the pump manager).
   *
   * Used both for the dashboard's `sessionActive` flag and for the
   * single-instance guard in the `run-auto` message handler. Both
   * call sites need the same definition; centralizing here keeps them
   * from drifting.
   */
  private isSessionActive(): boolean {
    return (
      this.activeSession() !== undefined ||
      controlSocketLikelyPresent(this.options.projectDir)
    );
  }

  /**
   * Dispatch a manual-mode command over the live pump's transport
   * socket. Returns true when a pump was found and the dispatcher
   * was invoked; returns false when no pump is attached so the
   * caller can fall back to the legacy paths (PTY control socket or
   * chat-tab spawn).
   *
   * The dispatcher takes the pump itself so each call site picks the
   * specific method (`runStep`, `runCritique`, …). Methods are
   * declared optional on `LiveSessionTransport` to keep the PTY
   * transport — which doesn't speak this protocol — out of scope; we
   * verify presence here so a stray call against the wrong transport
   * still falls through cleanly.
   */
  routeManualCommand(dispatch: (pump: ManagedAutoSessionState["pump"]) => void): boolean {
    const session = this.activeSession();
    if (!session) {
      return false;
    }
    if (typeof session.pump.setStepMode !== "function") {
      // Wrong transport (PTY-mode pump, or a mock without these
      // methods). Fall through.
      return false;
    }
    dispatch(session.pump);
    return true;
  }

  /**
   * Persisted spec-path key. Per-project so two projects in the
   * same workspace remember different specs.
   */
  private specPathKey(): string {
    return `sim-flow.specPath.${this.options.projectDir}`;
  }

  private readSpecPath(): string {
    return this.options.workspaceState.get<string>(this.specPathKey()) ?? "";
  }

  /**
   * Persist the user's Spec field input. Two stores:
   *
   * - VS Code `workspaceState`, keyed per-project, so the dashboard's
   *   own UI seed (the value the field shows on first render after a
   *   reload) survives a window reload without depending on the
   *   project's on-disk state.
   * - `.sim-flow/config.toml::spec_path`, so the orchestrator's
   *   pre-DM0 ingestion hook can find the spec regardless of which
   *   launch path the user uses (Play, red Play, chat-participant,
   *   or a future `sim-flow run DM0`).
   *
   * The two stores are kept in sync best-effort. Failures to write
   * the project config are surfaced via an error post but do not
   * block the workspaceState write -- typing in the field stays
   * responsive even when `.sim-flow/` is read-only or missing.
   */
  async writeSpecPath(value: string): Promise<void> {
    await this.options.workspaceState.update(this.specPathKey(), value);
    try {
      const { writeSpecPath } = await import("../state/projectConfig");
      await writeSpecPath(this.options.projectDir, value);
    } catch (err) {
      await this.post({
        type: "error",
        message: "Failed to persist spec path to .sim-flow/config.toml",
        detail: String((err as Error).message ?? err),
      });
    }
  }

  /**
   * Persist the coverage section. Triggers a refresh so the panel
   * sees the (clamped) value the helper actually wrote.
   */
  async writeCoverage(value: import("./messages").CoverageState): Promise<void> {
    try {
      const { writeCoverageSettings } = await import("../state/projectConfig");
      await writeCoverageSettings(this.options.projectDir, {
        thresholdPct: value.thresholdPct,
        level: value.level,
      });
    } catch (err) {
      await this.post({
        type: "error",
        message: "Failed to persist coverage settings to .sim-flow/config.toml",
        detail: String((err as Error).message ?? err),
      });
      return;
    }
    await this.refresh();
  }

  // -------------------------------------------------------------
  // Watcher and plumbing
  // -------------------------------------------------------------

  private attachWatcher(): void {
    this.watcher = createStateWatcher(this.options.projectDir);
    this.watcher.onDidChange(() => {
      // Debounce: during an active flow the watcher fires per-file
      // for every artifact write / checkbox flip / critique render /
      // state.toml update. Without the 200ms quiet window the
      // webview burns frames re-rendering the full #app tree and
      // hover/click input becomes unusable.
      this.requestWatcherRefresh();
    });
    this.disposables.push(
      vscode.workspace.onDidChangeConfiguration((evt) => {
        if (evt.affectsConfiguration("sim-flow.llm")) {
          void this.postLlmConfig();
        }
        if (evt.affectsConfiguration("sim-flow.dashboard.experimentalUi")) {
          void this.reloadWebviewMode();
        }
      }),
    );
  }

  private async reloadWebviewMode(): Promise<void> {
    if (!this.panel) {
      return;
    }
    this.panel.webview.html = await this.renderHtml(this.panel.webview);
    await this.refresh();
  }

  private async postLlmConfig(): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const rawSource = (config.get<string>("llm.source") ?? "vscode") as string;
    const servers =
      (config.get<unknown>("llm.servers") as import("./messages").LlmServerEntry[] | undefined) ??
      [];
    const resolved = resolveLlmSource(rawSource, servers);
    const globalModel = config.get<string>("llm.model")?.trim();
    const globalModelFamilyId = config.get<string>("llm.modelFamily")?.trim();
    const globalRuntimeProfileId = config.get<string>("llm.runtimeProfile")?.trim();
    const model = resolved?.model ?? globalModel;
    const modelFamilyId = resolved?.modelFamilyId ?? globalModelFamilyId;
    const runtimeProfileId = resolved?.runtimeProfileId ?? globalRuntimeProfileId;
    const verbose = config.get<boolean>("llm.verbose") ?? true;
    const debugAdaptation = config.get<boolean>("llm.debugAdaptation") ?? false;
    await this.post({
      type: "llm-config",
      source: rawSource,
      model: model && model.length > 0 ? model : undefined,
      modelFamilyId: modelFamilyId && modelFamilyId.length > 0 ? modelFamilyId : undefined,
      runtimeProfileId:
        runtimeProfileId && runtimeProfileId.length > 0 ? runtimeProfileId : undefined,
      verbose,
      debugAdaptation,
    });
  }

  /**
   * Enumerate models for the supplied source and post the result to
   * the dashboard. Called both on the dashboard's explicit
   * `request-model-list` and on source changes.
   */
  async sendModelList(source: LlmSourceTag | string): Promise<void> {
    await sendModelListUi(this as unknown as UiContext, source);
  }

  async sendPromptsList(): Promise<void> {
    await sendPromptsListUi(this as unknown as UiContext);
  }

  async openPromptInEditor(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global",
  ): Promise<void> {
    await openPromptInEditorUi(this as unknown as UiContext, slug, kind, scope);
  }

  async resetPromptOverride(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global" | "all",
  ): Promise<void> {
    await resetPromptOverrideUi(this as unknown as UiContext, slug, kind, scope);
  }

  async pickSpecFile(): Promise<void> {
    await pickSpecFileUi(this as unknown as UiContext);
  }

  async regenerateBlockDiagram(): Promise<void> {
    await regenerateBlockDiagramUi(this as unknown as UiContext);
  }

  private async postBlockDiagram(): Promise<void> {
    await postBlockDiagramUi(this as unknown as UiContext);
  }

  async openDocumentInEditor(absPath: string): Promise<void> {
    await openDocumentInEditorUi(absPath);
  }

  async openCritiqueInEditor(stepId: string): Promise<void> {
    await openCritiqueInEditorUi(this as unknown as UiContext, stepId);
  }

  async openAnalysisFolder(): Promise<void> {
    await openAnalysisFolderUi(this as unknown as UiContext);
  }

  private async post(msg: HostMessage): Promise<boolean> {
    if (!this.panel) {
      return false;
    }
    return this.panel.webview.postMessage(msg);
  }

  // -------------------------------------------------------------
  // HTML template
  // -------------------------------------------------------------

  private async renderHtml(webview: vscode.Webview): Promise<string> {
    const nonce = randomNonce();
    const experimental =
      vscode.workspace.getConfiguration("sim-flow").get<boolean>("dashboard.experimentalUi") ===
      true;
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(
        this.options.extensionUri,
        "dist",
        "webview",
        experimental ? "panelExperimental.js" : "panel.js",
      ),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(
        this.options.extensionUri,
        "media",
        experimental ? "panelExperimental.css" : "panel.css",
      ),
    );
    const csp = [
      `default-src 'none'`,
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
    <link rel="stylesheet" href="${styleUri}" />
    <title>sim-flow Dashboard</title>
  </head>
  <body>
    <main id="app"></main>
    <script nonce="${nonce}" src="${scriptUri}"></script>
  </body>
</html>`;
  }
}


/**
 * Build the "Simulate and iterate" appendix that's tacked onto the
 * Generate Verilog prompt when the user has enabled simulation in the
 * Settings tab AND supplied a simulator path. Kept out of the static
 * prompt file so the file still represents the baseline emit-only
 * behavior; this section is only relevant when there's a simulator on
 * the user's machine that the agent can actually invoke.
 */
