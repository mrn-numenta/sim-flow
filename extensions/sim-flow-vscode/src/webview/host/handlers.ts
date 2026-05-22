/**
 * Webview-message dispatcher for the dashboard host. The
 * `DashboardHost.onWebviewMessage` switch lives here so `host.ts`
 * stays under the refactor threshold; the host class implements
 * the `MessageHandlerContext` interface structurally (all the
 * fields / methods are private members it already has, exposed via
 * the cast at the call site).
 */

import * as vscode from "vscode";

import type { ManagedAutoSessionState } from "../../chatPanel/autoSessionManager";
import type { StepMode } from "../../session/protocol-types";
import {
  cliBackendArgFor,
  isTerminalLlmSource,
  type LlmSourceTag,
  type WebviewMessage,
} from "../messages";

export interface MessageHandlerContext {
  readonly options: { projectDir: string };
  refresh(): Promise<void>;
  routeManualCommand(dispatch: (pump: ManagedAutoSessionState["pump"]) => void): boolean;
  tryControlSocketRunStep(step: string, kind: "work" | "critique"): Promise<boolean>;
  tryControlSocketRunGate(step: string): Promise<boolean>;
  tryControlSocketAdvance(step: string): Promise<boolean>;
  tryControlSocketReset(step: string): Promise<boolean>;
  sendGateForStep(step: string): Promise<void>;
  advanceStep(step: string): Promise<void>;
  isSessionActive(): boolean;
  activeSession(): ManagedAutoSessionState | undefined;
  stopAuto(): Promise<void>;
  runAutoEndToEnd(specPath: string): Promise<void>;
  pickSpecFile(): Promise<void>;
  writeSpecPath(value: string): Promise<void>;
  writeCoverage(value: import("../messages").CoverageState): Promise<void>;
  sendModelList(source: LlmSourceTag | string): Promise<void>;
  sendPromptsList(): Promise<void>;
  openPromptInEditor(slug: string, kind: "work" | "critique", scope: string): Promise<void>;
  resetPromptOverride(slug: string, kind: "work" | "critique", scope: string): Promise<void>;
  openCritiqueInEditor(step: string): Promise<void>;
  openDocumentInEditor(absPath: string): Promise<void>;
  openAnalysisFolder(): Promise<void>;
  regenerateBlockDiagram(): Promise<void>;
  generateVerilog(): Promise<void>;
  post(msg: import("../messages").HostMessage): Promise<boolean>;
  handleSetStepMode(mode: StepMode): Promise<void>;
}

export async function onWebviewMessage(
  ctx: MessageHandlerContext,
  msg: WebviewMessage,
): Promise<void> {
  switch (msg.type) {
    case "ready":
    case "refresh":
      await ctx.refresh();
      return;
    case "select-step":
      // Selection is webview-local; the rail is visual-only. Don't
      // run the gate as a side effect — the explicit "Run Gate"
      // button is the only path that fetches a gate result.
      // Also refresh state from disk: the user may have run /advance
      // or otherwise mutated the project since the last watcher
      // event (file watchers can miss changes for projects outside
      // the open workspace folder).
      await ctx.refresh();
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
      if (ctx.routeManualCommand((pump) => pump.runStep?.(msg.step, "work"))) {
        return;
      }
      if (await ctx.tryControlSocketRunStep(msg.step, "work")) {
        return;
      }
      await vscode.commands.executeCommand("sim-flow.runStep", msg.step, ctx.options.projectDir);
      return;
    case "run-critique":
      if (ctx.routeManualCommand((pump) => pump.runCritique?.(msg.step))) {
        return;
      }
      if (await ctx.tryControlSocketRunStep(msg.step, "critique")) {
        return;
      }
      await vscode.commands.executeCommand(
        "sim-flow.runCritique",
        msg.step,
        ctx.options.projectDir,
      );
      return;
    case "gate-step":
      if (ctx.routeManualCommand((pump) => pump.runGate?.(msg.step))) {
        return;
      }
      if (await ctx.tryControlSocketRunGate(msg.step)) {
        return;
      }
      await ctx.sendGateForStep(msg.step);
      return;
    case "advance-step":
      if (ctx.routeManualCommand((pump) => pump.advance?.(msg.step))) {
        return;
      }
      if (await ctx.tryControlSocketAdvance(msg.step)) {
        return;
      }
      await ctx.advanceStep(msg.step);
      return;
    case "run-auto": {
      // Connect button. CLI-agent sources (`claude-cli`,
      // `codex-cli`, `gh-copilot-cli`) bypass the chat participant
      // and run `sim-flow auto --llm-backend <name>` in a
      // per-project terminal. Single-instance guard: refuse a
      // second launch when one is already alive for this project.
      if (ctx.isSessionActive()) {
        await ctx.post({
          type: "error",
          message: "sim-flow is already running for this project.",
          detail:
            "Click Disconnect (\u{23FB}) to stop the existing session before " +
            "starting a new one.",
        });
        return;
      }
      const config = vscode.workspace.getConfiguration("sim-flow");
      const source = (config.get<string>("llm.source") ?? "vscode") as LlmSourceTag;
      if (isTerminalLlmSource(source)) {
        await vscode.commands.executeCommand(
          "sim-flow.runFlowTerminal",
          cliBackendArgFor(source),
          msg.specPath ?? "",
          ctx.options.projectDir,
        );
      } else {
        await vscode.commands.executeCommand(
          "sim-flow.runFlow",
          msg.specPath ?? "",
          ctx.options.projectDir,
        );
      }
      return;
    }
    case "stop-auto":
      await ctx.stopAuto();
      return;
    case "run-auto-end-to-end":
      await ctx.runAutoEndToEnd(msg.specPath);
      return;
    case "pick-spec-file":
      await ctx.pickSpecFile();
      return;
    case "set-spec-path":
      await ctx.writeSpecPath(msg.path);
      return;
    case "set-fully-auto-enabled":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("dashboard.showFullyAutomated", msg.enabled, vscode.ConfigurationTarget.Workspace);
      await ctx.refresh();
      return;
    case "set-verilog-sim-enabled":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("dashboard.verilogSimEnabled", msg.enabled, vscode.ConfigurationTarget.Workspace);
      await ctx.refresh();
      return;
    case "set-verilog-simulator-path":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("dashboard.verilogSimulatorPath", msg.path, vscode.ConfigurationTarget.Workspace);
      await ctx.refresh();
      return;
    case "switch-project":
      await vscode.commands.executeCommand("sim-flow.switchProject");
      return;
    case "new-project":
      await vscode.commands.executeCommand("sim-flow.newProject", msg.name, ctx.options.projectDir);
      return;
    case "rename-project":
      await vscode.commands.executeCommand("sim-flow.renameProject", ctx.options.projectDir);
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
    case "set-llm-model-family":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("llm.modelFamily", msg.modelFamilyId, vscode.ConfigurationTarget.Workspace);
      return;
    case "set-llm-runtime-profile":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("llm.runtimeProfile", msg.runtimeProfileId, vscode.ConfigurationTarget.Workspace);
      return;
    case "set-llm-verbose":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("llm.verbose", msg.verbose, vscode.ConfigurationTarget.Workspace);
      return;
    case "set-llm-debug-adaptation":
      await vscode.workspace
        .getConfiguration("sim-flow")
        .update("llm.debugAdaptation", msg.debugAdaptation, vscode.ConfigurationTarget.Workspace);
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
      await ctx.refresh();
      return;
    case "set-coverage":
      await ctx.writeCoverage(msg.coverage);
      return;
    case "request-model-list":
      await ctx.sendModelList(msg.source);
      return;
    case "prompts-list":
      await ctx.sendPromptsList();
      return;
    case "prompt-open-in-editor":
      await ctx.openPromptInEditor(msg.slug, msg.kind, msg.scope);
      return;
    case "prompt-reset":
      await ctx.resetPromptOverride(msg.slug, msg.kind, msg.scope);
      return;
    case "reset-step": {
      // Reset is destructive. The detail copy depends on whether
      // an orchestrator is attached: when it is, the reset deletes
      // generated work artifacts and critique files in addition to
      // clearing gate flags; when it isn't, the CLI fallback only
      // mutates state.toml. Be honest about which one will happen.
      const hasOrchestrator = ctx.activeSession() !== undefined;
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
      if (ctx.routeManualCommand((pump) => pump.reset?.(msg.step))) {
        return;
      }
      if (await ctx.tryControlSocketReset(msg.step)) {
        return;
      }
      await vscode.commands.executeCommand("sim-flow.resetStep", msg.step, ctx.options.projectDir);
      return;
    }
    case "open-critique":
      await ctx.openCritiqueInEditor(msg.step);
      return;
    case "open-document":
      await ctx.openDocumentInEditor(msg.path);
      return;
    case "regenerate-block-diagram":
      await ctx.regenerateBlockDiagram();
      return;
    case "open-analysis-report":
      await ctx.openAnalysisFolder();
      return;
    case "generate-verilog":
      await ctx.generateVerilog();
      return;
    case "set-step-mode":
      await ctx.handleSetStepMode(msg.mode);
      return;
    default:
      // Exhaustive-check guard: unknown messages are silently ignored
      // rather than throwing, to avoid crashing the panel during
      // development.
      return;
  }
}
