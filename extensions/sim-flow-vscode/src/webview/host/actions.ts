/**
 * Step / flow / control-socket actions for the dashboard host.
 *
 * These were `DashboardHost` methods that drove the orchestrator
 * over the control socket or the chat-panel transport. Pulled out
 * verbatim so `host.ts` stays under the 1000-line refactor
 * threshold; the host implements the `ActionsContext` interface
 * structurally and the call sites cast `this` to bridge TS's
 * nominal privacy check.
 */

import * as vscode from "vscode";

import type { AutoSessionManager, ManagedAutoSessionState } from "../../chatPanel/autoSessionManager";
import type { SimFlowCli } from "../../cli/simflow";
import {
  ControlSocketError,
  controlSocketLikelyPresent,
  sendCommand as sendControlCommand,
} from "../../session/control-client";
import { cliBackendArgFor, isTerminalLlmSource, type HostMessage, type LlmSourceTag } from "../messages";
import { buildSimulateAndIterateAppendix } from "./helpers";
import { loadFlowState } from "./loaders";

export interface ActionsContext {
  readonly options: {
    projectDir: string;
    cli: SimFlowCli;
    autoSessions?: AutoSessionManager;
  };
  readonly panel: vscode.WebviewPanel | undefined;
  post(msg: HostMessage): Promise<boolean>;
  refresh(): Promise<void>;
  activeSession(): ManagedAutoSessionState | undefined;
}

export async function runAutoEndToEnd(ctx: ActionsContext, specPath: string): Promise<void> {
  const trimmed = specPath.trim();
  if (trimmed.length === 0) {
    await ctx.post({
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
      ctx.options.projectDir,
    );
  } else {
    await vscode.commands.executeCommand("sim-flow.runFlow", trimmed, ctx.options.projectDir);
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
export async function generateVerilog(ctx: ActionsContext): Promise<void> {
  const flow = await loadFlowState(ctx.options.cli);
  const dm2dPassed = flow.gates?.["DM2d"]?.passed === true;
  if (!dm2dPassed) {
    await ctx.post({
      type: "error",
      message: "Generate Verilog requires DM2d to have passed.",
      detail:
        "The Foundation model needs to build and pass its tests before " +
        "SystemVerilog emission. Finish DM2d (Model) first, then click " +
        "Generate Verilog again.",
    });
    return;
  }
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    await ctx.post({
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
    prompt = await ctx.options.cli.promptShow("generate-verilog", "work");
  } catch (err) {
    await ctx.post({
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
  await sendControlOrFallback(ctx, { command: "inject", text: prompt }, "generate-verilog");
}

export async function stopAuto(ctx: ActionsContext): Promise<void> {
  // Routing order:
  // 1. JSONL transport -- when a `SocketSessionPump` is attached
  //    for this project, use its escalation path: clean shutdown
  //    over the socket, then SIGTERM, then SIGKILL. This is the
  //    only path that guarantees the spawned `sim-flow auto`
  //    child is reaped (the control-socket `/exit` inject only
  //    nudges the inner CLI agent and can leave a zombie if the
  //    orchestrator is blocked in an LLM call).
  // 2. PTY control socket -- single-session CLI-agent mode where
  //    we inject `/exit` to the terminal. Falls through.
  // 3. No socket reachable -- surface an error.
  const session = ctx.activeSession();
  if (session && typeof session.pump.disconnectWithEscalation === "function") {
    try {
      const outcome = await session.pump.disconnectWithEscalation();
      if (outcome === "sigkill") {
        await ctx.post({
          type: "error",
          message: "Disconnect required SIGKILL.",
          detail:
            "The orchestrator did not exit after `shutdown` and SIGTERM " +
            "within the timeout. The child has been killed; check the " +
            "debug log if this happens repeatedly.",
        });
      }
    } finally {
      await ctx.options.autoSessions?.clearIfActive(session);
      await ctx.refresh();
    }
    return;
  }
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    await ctx.post({
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
  await sendControlOrFallback(ctx, { command: "inject", text: "/exit" }, "stop-auto");
}

export async function tryControlSocketRunStep(
  ctx: ActionsContext,
  step: string,
  kind: "work" | "critique",
): Promise<boolean> {
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    return await cliSourceGuard(ctx);
  }
  const text =
    kind === "work"
      ? `Begin step ${step} (work session). Read the step's instruction file under \`instructions/\` and the relevant predecessor artifacts before producing output.`
      : `Begin step ${step} critique. Review this step's artifacts and write \`docs/critiques/${step}-critique.md\` per the critique-instruction conventions. Do NOT write under \`.sim-flow/\` -- that's the orchestrator's private state.`;
  return await sendControlOrFallback(ctx, { command: "inject", text }, "run-step");
}

export async function tryControlSocketRunGate(ctx: ActionsContext, step: string): Promise<boolean> {
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    return false;
  }
  return await sendControlOrFallback(ctx, { command: "run-gate", step }, "run-gate");
}

export async function tryControlSocketAdvance(ctx: ActionsContext, step: string): Promise<boolean> {
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    return false;
  }
  return await sendControlOrFallback(ctx, { command: "advance", step }, "advance");
}

export async function tryControlSocketReset(ctx: ActionsContext, step: string): Promise<boolean> {
  if (!controlSocketLikelyPresent(ctx.options.projectDir)) {
    return false;
  }
  return await sendControlOrFallback(ctx, { command: "reset", step }, "reset");
}

async function cliSourceGuard(ctx: ActionsContext): Promise<boolean> {
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
  await ctx.post({
    type: "error",
    message: `Agent isn't reachable for source \`${source}\`.`,
    detail: tip,
  });
  return true;
}

async function sendControlOrFallback(
  ctx: ActionsContext,
  command: Parameters<typeof sendControlCommand>[1],
  label: string,
): Promise<boolean> {
  try {
    await sendControlCommand(ctx.options.projectDir, command);
    return true;
  } catch (err) {
    if (err instanceof ControlSocketError && err.kind === "missing-socket") {
      return false; // socket disappeared between the stat() and the connect; fall back
    }
    await ctx.post({
      type: "error",
      message: `${label}: control socket reachable but rejected the command`,
      detail: String((err as Error).message ?? err),
    });
    return true;
  }
}

export async function sendGateForStep(ctx: ActionsContext, step: string): Promise<void> {
  if (!ctx.panel) {
    return;
  }
  try {
    const result = await ctx.options.cli.gate(step);
    await ctx.post({ type: "gate-result", step, result });
  } catch (err) {
    await ctx.post({
      type: "error",
      message: `Gate check for ${step} failed`,
      detail: String((err as Error).message ?? err),
    });
  }
}

export async function advanceStep(ctx: ActionsContext, step: string): Promise<void> {
  if (!ctx.panel) {
    return;
  }
  try {
    const result = await ctx.options.cli.advance(step);
    // CLI returns the gate report whether or not the advance
    // succeeded; surface it so the user sees blockers when the gate
    // isn't clean.
    await ctx.post({
      type: "gate-result",
      step,
      result: { step, clean: result.clean, failures: result.failures },
    });
    if (!result.clean) {
      await ctx.post({
        type: "error",
        message: `Cannot advance ${step}: gate has ${result.failures.length} failure(s).`,
        detail: result.failures.map((f) => `- ${f.description}: ${f.reason}`).join("\n"),
      });
    }
  } catch (err) {
    await ctx.post({
      type: "error",
      message: `Advance ${step} failed`,
      detail: String((err as Error).message ?? err),
    });
  } finally {
    // Pull fresh state so the rail reflects any current_step bump.
    await ctx.refresh();
  }
}
