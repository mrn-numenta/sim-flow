// Extension-side host for the Flow Dashboard webview. Owns panel
// lifecycle, loads HTML, fulfills webview messages by delegating to
// the CLI wrapper and the state readers, and broadcasts state updates
// on file-watcher events.

import * as path from "node:path";
import * as vscode from "vscode";

import type { SimFlowCli } from "../cli/simflow";
import type { CritiqueFile } from "../state/critiques";
import { listCritiqueFiles } from "../state/critiques";
import type { FlowState } from "../state/flowState";
import { readAllPlanProgress, readPlanProgress } from "../state/planProgress";
import { readFlowState } from "../state/flowState";
import { openExperiments } from "../state/experiments";
import { createStateWatcher, type SimFlowStateWatcher } from "../state/watcher";
import { enumerateProjectDocuments } from "../state/documents";

import { enumerateModels } from "../llm/enumerate";
import {
  ControlSocketError,
  controlSocketLikelyPresent,
  sendCommand as sendControlCommand,
} from "../session/control-client";

import type { AutoSessionManager, ManagedAutoSessionState } from "../chatPanel/autoSessionManager";
import type { StepMode } from "../session/protocol-types";

import { aggregateDashboardState } from "./aggregate";
import {
  cliBackendArgFor,
  isTerminalLlmSource,
  type DashboardState,
  type HostMessage,
  type LlmSourceTag,
  type WebviewMessage,
} from "./messages";
import type { LlmSource } from "../llm/types";

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
    switch (msg.type) {
      case "ready":
      case "refresh":
        await this.refresh();
        return;
      case "select-step":
        // Selection is webview-local; the rail is visual-only. Don't
        // run the gate as a side effect — the explicit "Run Gate"
        // button is the only path that fetches a gate result.
        // Also refresh state from disk: the user may have run /advance
        // or otherwise mutated the project since the last watcher
        // event (file watchers can miss changes for projects outside
        // the open workspace folder).
        await this.refresh();
        return;
      case "run-step":
        // Routing order:
        // 1. JSONL transport socket — when the chat panel is running
        //    a manual-mode `SocketSessionPump` for this project, the
        //    dashboard's per-step buttons dispatch as `RunStep` host
        //    events over the same transport. Auto mode rejects the
        //    command with a Diagnostic on the orchestrator side; the
        //    dashboard prevents that by disabling the buttons (see
        //    `panel.ts::stepBox`), but if a misbehaving client still
        //    sends one, the user sees a clear warning rather than a
        //    silently-dropped click.
        // 2. PTY control socket — single-session CLI-agent mode
        //    where a `claude` PTY is listening; falls through.
        // 3. Legacy chat-tab spawn — `sim-flow.runStep` opens a
        //    fresh chat tab and runs the step there.
        if (this.routeManualCommand((pump) => pump.runStep?.(msg.step, "work"))) {
          return;
        }
        if (await this.tryControlSocketRunStep(msg.step, "work")) {
          return;
        }
        await vscode.commands.executeCommand("sim-flow.runStep", msg.step, this.options.projectDir);
        return;
      case "run-critique":
        if (this.routeManualCommand((pump) => pump.runCritique?.(msg.step))) {
          return;
        }
        if (await this.tryControlSocketRunStep(msg.step, "critique")) {
          return;
        }
        await vscode.commands.executeCommand(
          "sim-flow.runCritique",
          msg.step,
          this.options.projectDir,
        );
        return;
      case "gate-step":
        if (this.routeManualCommand((pump) => pump.runGate?.(msg.step))) {
          return;
        }
        if (await this.tryControlSocketRunGate(msg.step)) {
          return;
        }
        await this.sendGateForStep(msg.step);
        return;
      case "advance-step":
        if (this.routeManualCommand((pump) => pump.advance?.(msg.step))) {
          return;
        }
        if (await this.tryControlSocketAdvance(msg.step)) {
          return;
        }
        await this.advanceStep(msg.step);
        return;
      case "run-auto": {
        // Connect button. CLI-agent sources (`claude-cli`,
        // `codex-cli`, `gh-copilot-cli`) bypass the chat participant
        // and run `sim-flow auto --llm-backend <name>` in a
        // per-project terminal. Their auth comes from the user's
        // existing CLI login (claude /login, codex login, gh auth
        // login). Other sources continue to route through the chat
        // pane via `sim-flow.runFlow` so the orchestrator can render
        // streaming chunks, tool invocations, and gate results in
        // the same participant. The webview message is still named
        // `run-auto` (matching the underlying `sim-flow auto` CLI
        // subcommand), but the dashboard surfaces it as Connect:
        // it just establishes the session. Driving steps is left
        // to the explicit step-rail buttons.
        const config = vscode.workspace.getConfiguration("sim-flow");
        const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
        if (isTerminalLlmSource(source)) {
          // Single-session mode: if a sim-flow auto process is
          // already running and idle (the user typed `/exit` in
          // claude and sim-flow is "Waiting for the next dashboard
          // command"), don't spawn a second one and don't auto-run
          // a step -- just acknowledge the existing connection. The
          // user drives the next step explicitly from the rail.
          if (controlSocketLikelyPresent(this.options.projectDir)) {
            return;
          }
          await vscode.commands.executeCommand(
            "sim-flow.runFlowTerminal",
            cliBackendArgFor(source),
            msg.specPath ?? "",
            this.options.projectDir,
          );
        } else {
          await vscode.commands.executeCommand(
            "sim-flow.runFlow",
            msg.specPath ?? "",
            this.options.projectDir,
          );
        }
        return;
      }
      case "stop-auto":
        await this.stopAuto();
        return;
      case "run-auto-end-to-end":
        await this.runAutoEndToEnd(msg.specPath);
        return;
      case "pick-spec-file":
        await this.pickSpecFile();
        return;
      case "set-spec-path":
        await this.writeSpecPath(msg.path);
        return;
      case "set-fully-auto-enabled":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update(
            "dashboard.showFullyAutomated",
            msg.enabled,
            vscode.ConfigurationTarget.Workspace,
          );
        await this.refresh();
        return;
      case "set-verilog-sim-enabled":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update("dashboard.verilogSimEnabled", msg.enabled, vscode.ConfigurationTarget.Workspace);
        await this.refresh();
        return;
      case "set-verilog-simulator-path":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update("dashboard.verilogSimulatorPath", msg.path, vscode.ConfigurationTarget.Workspace);
        await this.refresh();
        return;
      case "switch-project":
        await vscode.commands.executeCommand("sim-flow.switchProject");
        return;
      case "new-project":
        await vscode.commands.executeCommand(
          "sim-flow.newProject",
          msg.name,
          this.options.projectDir,
        );
        return;
      case "rename-project":
        await vscode.commands.executeCommand("sim-flow.renameProject", this.options.projectDir);
        return;
      case "set-llm-source": {
        // Persist at workspace scope so the change is project-y by
        // default; the global setting still applies elsewhere. Also
        // clear `llm.model` because the previous source's id format
        // (e.g. `claude-code/claude-sonnet-4.6` for the `vscode`
        // source) is rarely valid for the new source (e.g. the
        // `claude` CLI wants `claude-sonnet-4-6` or `sonnet`). The
        // model dropdown will re-populate against the new source on
        // its next request-model-list cycle and let the user pick.
        const cfg = vscode.workspace.getConfiguration("sim-flow");
        await cfg.update("llm.source", msg.source, vscode.ConfigurationTarget.Workspace);
        await cfg.update("llm.model", "", vscode.ConfigurationTarget.Workspace);
        // The configuration listener will post llm-config back; no
        // explicit re-post needed here.
        return;
      }
      case "set-llm-model":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update("llm.model", msg.model, vscode.ConfigurationTarget.Workspace);
        return;
      case "set-llm-verbose":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update("llm.verbose", msg.verbose, vscode.ConfigurationTarget.Workspace);
        return;
      case "set-llm-servers":
        await vscode.workspace
          .getConfiguration("sim-flow")
          .update("llm.servers", msg.servers, vscode.ConfigurationTarget.Workspace);
        // Push a fresh state-update so the table re-renders from
        // the post-write servers array. Without this, the
        // `onDidChangeConfiguration` handler fires `postLlmConfig`
        // which triggers a webview re-render against `ui.data` --
        // and `ui.data.llmServers` is still the pre-write value
        // until the next `state-update`. The result is that any
        // edit to a server row (port, host, name, model) snaps
        // back to the prior value on blur.
        await this.refresh();
        return;
      case "set-coverage":
        await this.writeCoverage(msg.coverage);
        return;
      case "request-model-list":
        await this.sendModelList(msg.source);
        return;
      case "prompts-list":
        await this.sendPromptsList();
        return;
      case "prompt-open-in-editor":
        await this.openPromptInEditor(msg.slug, msg.kind, msg.scope);
        return;
      case "prompt-reset":
        await this.resetPromptOverride(msg.slug, msg.kind, msg.scope);
        return;
      case "reset-step": {
        // Reset is destructive. The detail copy depends on whether
        // an orchestrator is attached: when it is, the reset deletes
        // generated work artifacts and critique files in addition to
        // clearing gate flags; when it isn't, the CLI fallback only
        // mutates state.toml. Be honest about which one will happen.
        const hasOrchestrator = this.activeSession() !== undefined;
        const detail = hasOrchestrator
          ? `Deletes generated work artifacts and the critique file for ${msg.step} ` +
            `and every downstream step in the flow, and clears their gate flags. ` +
            `Source spec, conversation transcript, and git history are not touched.\n\n` +
            `This cannot be undone.`
          : `Clears gate flags for ${msg.step} and every downstream step. ` +
            `No orchestrator is attached, so generated artifacts on disk are NOT deleted — ` +
            `Connect first if you want a full reset.\n\n` +
            `This cannot be undone.`;
        const confirmed = await vscode.window.showWarningMessage(
          `Reset ${msg.step}?`,
          { modal: true, detail },
          "Reset",
        );
        if (confirmed !== "Reset") {
          return;
        }
        if (this.routeManualCommand((pump) => pump.reset?.(msg.step))) {
          return;
        }
        if (await this.tryControlSocketReset(msg.step)) {
          return;
        }
        await vscode.commands.executeCommand(
          "sim-flow.resetStep",
          msg.step,
          this.options.projectDir,
        );
        return;
      }
      case "open-critique":
        await this.openCritiqueInEditor(msg.step);
        return;
      case "open-document":
        await this.openDocumentInEditor(msg.path);
        return;
      case "regenerate-block-diagram":
        await this.regenerateBlockDiagram();
        return;
      case "open-analysis-report":
        await this.openAnalysisFolder();
        return;
      case "generate-verilog":
        await this.generateVerilog();
        return;
      case "set-step-mode":
        await this.handleSetStepMode(msg.mode);
        return;
      default:
        // Exhaustive-check guard: unknown messages are silently ignored
        // rather than throwing, to avoid crashing the panel during
        // development.
        return;
    }
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
  private async handleSetStepMode(mode: StepMode): Promise<void> {
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
  private async runAutoEndToEnd(specPath: string): Promise<void> {
    const trimmed = specPath.trim();
    if (trimmed.length === 0) {
      await this.post({
        type: "error",
        message: "Fully-automated flow needs a spec path.",
        detail:
          "Type or browse to a `.md` / `.pdf` / `.txt` spec in the Spec field, " +
          "then click the red play button again. Without a spec the agent has " +
          "nothing to derive `docs/spec.md` from in unattended mode.",
      });
      return;
    }
    const choice = await vscode.window.showWarningMessage(
      "Start fully-automated end-to-end run?",
      {
        modal: true,
        detail:
          "The agent will walk every step (DM0 → DM4b) without stopping for " +
          "review, retrying critique blockers up to the configured iteration " +
          "cap. This can take a long time and burn meaningful LLM credits. " +
          "You can stop it at any time with the ■ button.",
      },
      "Run",
    );
    if (choice !== "Run") {
      return;
    }
    const config = vscode.workspace.getConfiguration("sim-flow");
    const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
    if (isTerminalLlmSource(source)) {
      await vscode.commands.executeCommand(
        "sim-flow.runAutoFullyAutomatedTerminal",
        cliBackendArgFor(source),
        trimmed,
        this.options.projectDir,
      );
    } else {
      await vscode.commands.executeCommand("sim-flow.runFlow", trimmed, this.options.projectDir);
    }
  }

  /**
   * One-off "Generate Verilog" button: emit synthesizable SystemVerilog
   * RTL plus a UVM testbench from the current Foundation model. Loads
   * `tools/sim-flow/prompts/generate-verilog.md` (resolving project /
   * global / default scope the same way step prompts do) and injects
   * the full text into the running agent. Gated on DM2d having passed
   * because the model has to compile + pass tests before SV emission
   * makes sense -- the button is also disabled in the UI, but we
   * re-check here as defense in depth.
   *
   * Requires the agent to be running (control socket reachable). The
   * chat-pane participant has no free-form one-off entry point, and
   * the CLI agents run in a terminal, so without the socket there is
   * no usable target. Posts a friendly "click Play first" error in
   * that case.
   */
  private async generateVerilog(): Promise<void> {
    const flow = await readFlowStateSafe(this.options.projectDir);
    const dm2dPassed = flow.gates?.["DM2d"]?.passed === true;
    if (!dm2dPassed) {
      await this.post({
        type: "error",
        message: "Generate Verilog requires DM2d to have passed.",
        detail:
          "The Foundation model needs to build and pass its tests before " +
          "SystemVerilog emission. Finish DM2d (Model) first, then click " +
          "Generate Verilog again.",
      });
      return;
    }
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      await this.post({
        type: "error",
        message: "Generate Verilog needs the agent to be running.",
        detail:
          "Click the Connect (\u{1F50C}) button to launch the flow first, then click " +
          "Generate Verilog. The prompt is injected over the single-session " +
          "control socket, which only exists while sim-flow is running in " +
          "single-session mode.",
      });
      return;
    }
    let prompt: string;
    try {
      prompt = await this.options.cli.promptShow("generate-verilog", "work");
    } catch (err) {
      await this.post({
        type: "error",
        message: "Failed to load generate-verilog prompt.",
        detail: String((err as Error).message ?? err),
      });
      return;
    }
    const cfg = vscode.workspace.getConfiguration("sim-flow");
    const simEnabled = cfg.get<boolean>("dashboard.verilogSimEnabled") ?? false;
    const simPath = (cfg.get<string>("dashboard.verilogSimulatorPath") ?? "").trim();
    if (simEnabled && simPath.length > 0) {
      prompt = `${prompt.trimEnd()}\n\n${buildSimulateAndIterateAppendix(simPath)}\n`;
    }
    await this.sendControlOrFallback({ command: "inject", text: prompt }, "generate-verilog");
  }

  private async stopAuto(): Promise<void> {
    // Routing order:
    // 1. JSONL transport — when a `SocketSessionPump` is attached
    //    for this project, use its escalation path: clean shutdown
    //    over the socket, then SIGTERM, then SIGKILL. This is the
    //    only path that guarantees the spawned `sim-flow auto`
    //    child is reaped (the control-socket `/exit` inject only
    //    nudges the inner CLI agent and can leave a zombie if the
    //    orchestrator is blocked in an LLM call).
    // 2. PTY control socket — single-session CLI-agent mode where
    //    we inject `/exit` to the terminal. Falls through.
    // 3. No socket reachable — surface an error.
    const session = this.activeSession();
    if (session && typeof session.pump.disconnectWithEscalation === "function") {
      try {
        const outcome = await session.pump.disconnectWithEscalation();
        if (outcome === "sigkill") {
          await this.post({
            type: "error",
            message: "Disconnect required SIGKILL.",
            detail:
              "The orchestrator did not exit after `shutdown` and SIGTERM " +
              "within the timeout. The child has been killed; check the " +
              "debug log if this happens repeatedly.",
          });
        }
      } finally {
        await this.options.autoSessions?.clearIfActive(session);
        await this.refresh();
      }
      return;
    }
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      await this.post({
        type: "error",
        message: "No running flow to disconnect.",
        detail:
          "Disconnect sends `/exit` over the single-session control socket, " +
          "but no socket is reachable for this project. Either the " +
          "flow isn't running (click Connect) or it's running " +
          "in per-step mode, in which case typing `/exit` directly in " +
          "the terminal is the way to stop it.",
      });
      return;
    }
    await this.sendControlOrFallback({ command: "inject", text: "/exit" }, "stop-auto");
  }

  private async tryControlSocketRunStep(step: string, kind: "work" | "critique"): Promise<boolean> {
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      return await this.cliSourceGuard();
    }
    // The interactive driver re-builds and injects the step prompt
    // when it sees an `advance` command. For an explicit "Run Step"
    // click on a step that's NOT the next-after-current, fall back
    // to a generic inject that asks claude to start that step. (The
    // orchestrator-side prompt-rendering helpers aren't reachable
    // from the extension; doing this round-trip via the socket
    // requires a small extension to the protocol — Pass 2 polish.)
    const text =
      kind === "work"
        ? `Begin step ${step} (work session). Read the step's instruction file under \`instructions/\` and the relevant predecessor artifacts before producing output.`
        : `Begin step ${step} critique. Review this step's artifacts and write \`docs/critiques/${step}-critique.md\` per the critique-instruction conventions. Do NOT write under \`.sim-flow/\` -- that's the orchestrator's private state.`;
    return await this.sendControlOrFallback({ command: "inject", text }, "run-step");
  }

  /**
   * Forward a Run Gate click. When the agent is running (socket up),
   * the driver runs the gate AND injects the formatted result into
   * claude so the user sees it inline. When the agent isn't running,
   * we fall through to the local-CLI path -- gate evaluation doesn't
   * need an agent, so this works in both per-step and single-session
   * modes regardless of whether the agent is alive.
   */
  private async tryControlSocketRunGate(step: string): Promise<boolean> {
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      return false;
    }
    return await this.sendControlOrFallback({ command: "run-gate", step }, "run-gate");
  }

  /**
   * Forward an Advance click. With agent running: gate + mark passed +
   * bump current_step + inject the next step's prompt. Without:
   * fall through to the local-CLI advance which mutates state.toml
   * but doesn't notify any agent (there isn't one).
   */
  private async tryControlSocketAdvance(step: string): Promise<boolean> {
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      return false;
    }
    return await this.sendControlOrFallback({ command: "advance", step }, "advance");
  }

  /**
   * Forward a Reset click. State mutation; works without an agent
   * (falls through to the local-CLI path when no socket).
   */
  private async tryControlSocketReset(step: string): Promise<boolean> {
    if (!controlSocketLikelyPresent(this.options.projectDir)) {
      return false;
    }
    return await this.sendControlOrFallback({ command: "reset", step }, "reset");
  }

  /**
   * Called when the user clicks Run Step / Gate / Advance / Reset and
   * the control socket isn't present. Two cases:
   *
   * - The source is a CLI agent (`claude-cli`, `codex-cli`, etc.) ->
   *   the chat-tab fallback wouldn't work either (the LLM factory
   *   rejects CLI sources for in-pane use). Post a clear
   *   "start the agent first" instruction and claim the click.
   *
   * - The source is an API backend (`vscode`, `anthropic`, etc.) ->
   *   the chat-tab fallback IS valid; return `false` so the caller
   *   falls through to its legacy chat-tab path.
   */
  private async cliSourceGuard(): Promise<boolean> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
    if (!isTerminalLlmSource(source)) {
      return false;
    }
    const sessionMode = (config.get<string>("session.mode") ?? "per-step").trim();
    const tip =
      sessionMode === "single"
        ? "Click 'Run / Resume Automated Flow' first; the agent's control socket only opens when sim-flow is running in single-session mode."
        : "Per-step mode advances automatically when you type `/exit` in the running terminal. To remote-control individual buttons from the dashboard, set `sim-flow.session.mode` to `single` and re-launch the automated flow.";
    await this.post({
      type: "error",
      message: `Agent isn't reachable for source \`${source}\`.`,
      detail: tip,
    });
    return true;
  }

  private async sendControlOrFallback(
    command: Parameters<typeof sendControlCommand>[1],
    label: string,
  ): Promise<boolean> {
    try {
      await sendControlCommand(this.options.projectDir, command);
      return true;
    } catch (err) {
      if (err instanceof ControlSocketError && err.kind === "missing-socket") {
        return false; // socket disappeared between the stat() and the connect; fall back
      }
      await this.post({
        type: "error",
        message: `${label}: control socket reachable but rejected the command`,
        detail: String((err as Error).message ?? err),
      });
      return true;
    }
  }

  private async sendGateForStep(step: string): Promise<void> {
    if (!this.panel) {
      return;
    }
    try {
      const result = await this.options.cli.gate(step);
      await this.post({ type: "gate-result", step, result });
    } catch (err) {
      await this.post({
        type: "error",
        message: `Gate check for ${step} failed`,
        detail: String((err as Error).message ?? err),
      });
    }
  }

  private async advanceStep(step: string): Promise<void> {
    if (!this.panel) {
      return;
    }
    try {
      const result = await this.options.cli.advance(step);
      // CLI returns the gate report whether or not the advance
      // succeeded; surface it so the user sees blockers when the gate
      // isn't clean.
      await this.post({
        type: "gate-result",
        step,
        result: { step, clean: result.clean, failures: result.failures },
      });
      if (!result.clean) {
        await this.post({
          type: "error",
          message: `Cannot advance ${step}: gate has ${result.failures.length} failure(s).`,
          detail: result.failures.map((f) => `- ${f.description}: ${f.reason}`).join("\n"),
        });
      }
    } catch (err) {
      await this.post({
        type: "error",
        message: `Advance ${step} failed`,
        detail: String((err as Error).message ?? err),
      });
    } finally {
      // Pull fresh state so the rail reflects any current_step bump.
      await this.refresh();
    }
  }

  // -------------------------------------------------------------
  // State aggregation
  // -------------------------------------------------------------

  private async buildState(): Promise<DashboardState> {
    const [flow, critiques, runs, baselines] = await Promise.all([
      readFlowStateSafe(this.options.projectDir),
      listCritiqueFilesSafe(this.options.projectDir),
      this.loadRuns(),
      this.loadBaselines(),
    ]);
    const documents = enumerateProjectDocuments({
      projectDir: this.options.projectDir,
      flow: flow.flow,
    });
    const planProgress = await readPlanProgress(this.options.projectDir, flow.current_step);
    // All-kinds progress so the dashboard can show milestone
    // pipelines under any plan-related step (DM2c outline,
    // DM2cd detail, DM2d execution, etc.) regardless of which
    // step is current. Each kind is scanned independently so
    // missing-on-disk plans render as empty boxes rather than
    // hiding the section.
    const planProgressByKind = await readAllPlanProgress(this.options.projectDir);
    const specPath = this.readSpecPath();
    // Coverage settings live in the project's `.sim-flow/config.toml`
    // (the orchestrator side reads them too). Read failures fall
    // back to defaults so the dashboard keeps rendering even when
    // the file is missing or malformed.
    const coverage = await this.readCoverage();
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
    const sessionActive = !!session;
    // True while the orchestrator is inside a sub-session (Work or
    // Critique). The pump tracks this from the
    // `sub-session-started` / `-ended` bracket events. When no pump
    // is attached we report `false` so the dashboard's "no
    // orchestrator" path applies its own gating.
    const inSubSession = session?.pump.inSubSession ?? false;
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
  private routeManualCommand(dispatch: (pump: ManagedAutoSessionState["pump"]) => void): boolean {
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
  private async writeSpecPath(value: string): Promise<void> {
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
   * Read `[coverage]` from `.sim-flow/config.toml`. Falls back to
   * defaults silently -- the dashboard always has a value to seed
   * the Settings UI with, even on a fresh project.
   */
  private async readCoverage(): Promise<import("./messages").CoverageState> {
    try {
      const { readCoverageSettings } = await import("../state/projectConfig");
      const settings = await readCoverageSettings(this.options.projectDir);
      return { thresholdPct: settings.thresholdPct, level: settings.level };
    } catch {
      // Don't surface read failures: the agent side will catch a
      // malformed file when it next loads the config, and the
      // user can still edit fields here to overwrite a broken
      // section.
      return { thresholdPct: 90, level: "total" };
    }
  }

  /**
   * Persist the coverage section. Triggers a refresh so the panel
   * sees the (clamped) value the helper actually wrote.
   */
  private async writeCoverage(value: import("./messages").CoverageState): Promise<void> {
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

  private async loadRuns(): Promise<DashboardState["runs"]> {
    const reader = openExperiments(this.options.projectDir);
    if (!reader) {
      return [];
    }
    try {
      return reader.listRuns({ limit: MAX_DASHBOARD_RUNS });
    } finally {
      reader.close();
    }
  }

  private async loadBaselines(): Promise<DashboardState["baselines"]> {
    const reader = openExperiments(this.options.projectDir);
    if (!reader) {
      return [];
    }
    try {
      return reader.listBaselines();
    } finally {
      reader.close();
    }
  }

  // -------------------------------------------------------------
  // Watcher and plumbing
  // -------------------------------------------------------------

  private attachWatcher(): void {
    this.watcher = createStateWatcher(this.options.projectDir);
    this.watcher.onDidChange(() => {
      void this.refresh();
    });
    this.disposables.push(
      vscode.workspace.onDidChangeConfiguration((evt) => {
        if (evt.affectsConfiguration("sim-flow.llm")) {
          void this.postLlmConfig();
        }
      }),
    );
  }

  private async postLlmConfig(): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
    const model = config.get<string>("llm.model")?.trim();
    const verbose = config.get<boolean>("llm.verbose") ?? true;
    await this.post({
      type: "llm-config",
      source,
      model: model && model.length > 0 ? model : undefined,
      verbose,
    });
  }

  /**
   * Enumerate models for the supplied source and post the result to
   * the dashboard. Called both on the dashboard's explicit
   * `request-model-list` and on source changes.
   */
  private async sendModelList(source: LlmSourceTag | string): Promise<void> {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const ollamaBaseUrl = config.get<string>("llm.ollama.baseUrl") ?? undefined;
    const lmstudioBaseUrl = config.get<string>("llm.lmstudio.baseUrl") ?? undefined;
    // Resolve `server:<name>` against the user-defined servers
    // array so we enumerate against the right host/port. Falls
    // back to the built-in source enumerators when no server
    // matches.
    let resolvedSource: LlmSource = source as LlmSource;
    let baseUrl: string | undefined;
    if (typeof source === "string" && source.startsWith("server:")) {
      const name = source.slice("server:".length);
      const servers = (config.get<unknown>("llm.servers") as
        | import("./messages").LlmServerEntry[]
        | undefined) ?? [];
      const entry = servers.find((s) => s.name === name);
      if (entry) {
        resolvedSource = entry.kind as LlmSource;
        const path = entry.path && entry.path.length > 0 ? entry.path : "/v1";
        const normalisedPath = path.startsWith("/") ? path : `/${path}`;
        baseUrl = `http://${entry.host}:${entry.port}${normalisedPath}`;
      }
    }
    const result = await enumerateModels({
      source: resolvedSource,
      ollamaBaseUrl,
      lmstudioBaseUrl,
      baseUrl,
    });
    await this.post({
      type: "model-list",
      source: source as LlmSourceTag,
      models: result.models,
      emptyReason: result.emptyReason,
      error: result.error,
    });
  }

  private async post(msg: HostMessage): Promise<boolean> {
    if (!this.panel) {
      return false;
    }
    return this.panel.webview.postMessage(msg);
  }

  private async sendPromptsList(): Promise<void> {
    try {
      const entries = await this.options.cli.promptsList();
      await this.post({ type: "prompts-list-result", entries });
    } catch (err) {
      await this.post({
        type: "error",
        message: "Failed to list prompts",
        detail: String((err as Error).message ?? err),
      });
    }
  }

  /**
   * Open a prompt override in a regular VS Code editor tab.
   *
   * The foundation-default prompt is intentionally never opened from
   * here -- the user can only edit at the `project` or `global`
   * override scope, which means VS Code's normal save flow can't
   * write back to the foundation tree. If the chosen override file
   * doesn't yet exist, we seed it with the currently-effective
   * resolved content (foundation default OR whatever the active
   * scope is) so the user has a working starting point rather than
   * an empty buffer.
   *
   * Saves use VS Code's standard file save -- nothing extra to do
   * on this side. The prompt resolver inside the orchestrator
   * already prefers project > global > default, so saving the file
   * is sufficient to make the override active.
   */
  private async openPromptInEditor(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global",
  ): Promise<void> {
    try {
      const entries = await this.options.cli.promptsList();
      const entry = entries.find((e) => e.slug === slug && e.kind === kind);
      if (!entry) {
        await this.post({
          type: "error",
          message: `Prompt ${slug}.${kind} is not in the registry.`,
        });
        return;
      }
      const target = scope === "project" ? entry.project_path : entry.global_path;
      if (!target) {
        await this.post({
          type: "error",
          message: `No global prompt path is configured.`,
          detail:
            "The CLI did not return a global override location for this prompt. " +
            'Pick "Edit (project)" instead, or set up a user-config directory ' +
            "before retrying.",
        });
        return;
      }
      const targetUri = vscode.Uri.file(target);
      let exists = true;
      try {
        await vscode.workspace.fs.stat(targetUri);
      } catch {
        exists = false;
      }
      if (!exists) {
        // Seed with the current effective content so the editor opens
        // on a meaningful starting point rather than an empty buffer.
        // `cli.promptShow` returns the resolved prompt -- the override
        // we're about to create / customize, or the foundation default
        // if none exists yet.
        const seed = await this.options.cli.promptShow(slug, kind);
        const parent = vscode.Uri.file(path.dirname(target));
        await vscode.workspace.fs.createDirectory(parent);
        await vscode.workspace.fs.writeFile(targetUri, Buffer.from(seed, "utf8"));
        // Refresh the dashboard so the row's "Project / Global" column
        // updates to reflect the newly-created override.
        await this.sendPromptsList();
      }
      const doc = await vscode.workspace.openTextDocument(targetUri);
      await vscode.window.showTextDocument(doc, { preview: false });
    } catch (err) {
      await this.post({
        type: "error",
        message: `Failed to open ${slug}.${kind} (${scope})`,
        detail: String((err as Error).message ?? err),
      });
    }
  }

  private async resetPromptOverride(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global" | "all",
  ): Promise<void> {
    try {
      await this.options.cli.promptReset(slug, kind, scope);
      void vscode.window.showInformationMessage(`Reset ${slug}.${kind} override (${scope}).`);
      await this.sendPromptsList();
    } catch (err) {
      await this.post({
        type: "error",
        message: `Failed to reset ${slug}.${kind} (${scope})`,
        detail: String((err as Error).message ?? err),
      });
    }
  }

  private async pickSpecFile(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
      openLabel: "Select spec",
      filters: {
        Spec: ["pdf", "md", "txt"],
        "All files": ["*"],
      },
    });
    if (!picked || picked.length === 0) {
      return;
    }
    await this.post({ type: "spec-path-picked", path: picked[0]!.fsPath });
  }

  private async regenerateBlockDiagram(): Promise<void> {
    try {
      await this.options.cli.blockDiagram();
    } catch (err) {
      await this.post({
        type: "error",
        message: "Block diagram generation failed",
        detail: String((err as Error).message ?? err),
      });
      return;
    }
    await this.postBlockDiagram();
  }

  private async postBlockDiagram(): Promise<void> {
    const svgPath = path.join(this.options.projectDir, ".sim-flow", "block-diagram.svg");
    let svg: string | null = null;
    try {
      svg = await import("node:fs").then((fs) => fs.readFileSync(svgPath, "utf8"));
    } catch {
      svg = null;
    }
    await this.post({ type: "block-diagram", svg });
  }

  private async openDocumentInEditor(absPath: string): Promise<void> {
    const uri = vscode.Uri.file(absPath);
    try {
      await vscode.window.showTextDocument(uri, { preview: false });
    } catch (err) {
      void vscode.window.showWarningMessage(
        `sim-flow: cannot open ${absPath}: ${(err as Error).message ?? String(err)}`,
      );
    }
  }

  private async openCritiqueInEditor(stepId: string): Promise<void> {
    const critique = path.join(
      this.options.projectDir,
      ".sim-flow",
      "critiques",
      `${stepId}-critique.md`,
    );
    const uri = vscode.Uri.file(critique);
    try {
      await vscode.window.showTextDocument(uri, { preview: true });
    } catch {
      void vscode.window.showWarningMessage(`No critique file found for ${stepId} at ${critique}`);
    }
  }

  private async openAnalysisFolder(): Promise<void> {
    const dir = path.join(this.options.projectDir, "docs", "analysis");
    const uri = vscode.Uri.file(dir);
    try {
      await vscode.commands.executeCommand("revealInExplorer", uri);
    } catch {
      void vscode.window.showWarningMessage(`Could not open ${dir}`);
    }
  }

  // -------------------------------------------------------------
  // HTML template
  // -------------------------------------------------------------

  private async renderHtml(webview: vscode.Webview): Promise<string> {
    const nonce = randomNonce();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.options.extensionUri, "dist", "webview", "panel.js"),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.options.extensionUri, "media", "panel.css"),
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

async function readFlowStateSafe(projectDir: string): Promise<FlowState> {
  try {
    return await readFlowState(projectDir);
  } catch {
    return {
      flow: "direct-modeling",
      current_step: "DM0",
      started: null,
      gates: {},
      archived_gates: {},
    };
  }
}

async function listCritiqueFilesSafe(projectDir: string): Promise<CritiqueFile[]> {
  try {
    return await listCritiqueFiles(projectDir);
  } catch {
    return [];
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
function buildSimulateAndIterateAppendix(simulatorPath: string): string {
  return [
    "## Simulate and iterate",
    "",
    "After every generated file is written, drive the SystemVerilog through",
    "the simulator the user has configured and iterate until simulation",
    "matches the Foundation model.",
    "",
    `- **Simulator binary**: \`${simulatorPath}\` (configured via the`,
    "  dashboard's Settings tab; `sim-flow.dashboard.verilogSimulatorPath`).",
    "  Invoke it from the project root, not from inside `generated/`.",
    "- **Compile + run**: prefer the `make sim` target from",
    "  `generated/test/Makefile`. If the Makefile's default tool doesn't",
    "  match the configured simulator, edit the Makefile so `make sim`",
    "  drives that simulator (don't add a second flow). The Makefile,",
    "  `sim.f`, and `tb_top.sv` together must produce a working invocation.",
    "- **Per-test runs**: `make sim TEST=<name>` should run a single UVM",
    "  test class. Use it to bisect failures.",
    "",
    "### Iteration loop",
    "",
    "1. Run `make sim`. Capture full stdout/stderr.",
    "2. Classify failures:",
    "   - **Compile / elaboration errors** -- syntax, missing typedefs,",
    "     port-width mismatches, struct layout drift. Fix the offending",
    "     file under `generated/`. NEVER edit `src/` or `tests/` to make",
    "     the SV happy -- the Foundation model is the reference.",
    "   - **UVM runtime errors** -- typically TB wiring (config_db, virtual",
    "     interfaces, analysis-port hookups). Fix in `generated/test/`.",
    "   - **Scoreboard mismatches** -- the SV behavior diverges from the",
    "     Rust model. The Rust model is the source of truth: re-derive the",
    "     RTL combinational body from `evaluate()`, the registered state",
    "     from `update()`, and re-emit the offending module. Confirm the",
    "     payload struct field order matches Rust before assuming the bug",
    "     is in the logic.",
    "3. Re-run `make sim`. Repeat until every test in the test plan passes.",
    "4. If you hit the same failure twice in a row without progress, stop",
    "   and report what you tried, the failing test, and the relevant",
    "   waveform / log excerpt. Don't churn the same file blindly.",
    "",
    "### Boundaries",
    "",
    "- All edits remain under `generated/`. `src/`, `tests/`, `.sim-flow/`,",
    "  and `docs/` stay read-only for this generation, including during",
    "  the simulate-and-iterate loop.",
    "- Don't lower coverage or skip tests to make simulation pass. If a",
    "  test is genuinely wrong (e.g. it encoded a Rust-only assumption),",
    "  call it out in your reply rather than silently disabling it.",
    "- Don't hand-edit waveforms or VCD output -- those are diagnostic",
    "  artifacts, not source. Fix the SV.",
  ].join("\n");
}

function randomNonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let out = "";
  for (let i = 0; i < 32; i++) {
    out += chars[Math.floor(Math.random() * chars.length)];
  }
  return out;
}

function readStepModeSetting(config: vscode.WorkspaceConfiguration): StepMode {
  const raw = (config.get<string>("flow.stepMode") ?? "manual").trim();
  return raw === "auto" ? "auto" : "manual";
}
