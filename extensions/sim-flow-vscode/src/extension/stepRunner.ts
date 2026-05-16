/**
 * Step / flow launch commands. The dashboard's Run Step / Run Critique
 * / Reset / Play buttons route through these helpers, which pick the
 * right surface for the configured LLM source -- terminal CLI agent,
 * built-in VS Code chat, or the in-extension chat panel.
 *
 * Lives outside extension.ts so the activate file stays under the
 * refactor threshold. Cross-extension deps (chat panel, in-terminal
 * launcher) come in through `StepRunnerDeps`.
 */

import * as vscode from "vscode";

import type { ChatPanelProvider } from "../chatPanel/host";
import { cliBackendArgFor, isTerminalLlmSource, type LlmSourceTag } from "../webview/messages";

export interface StepRunnerDeps {
  readonly chatPanelProvider: ChatPanelProvider | undefined;
  readonly runCliInTerminal: (subcommand: string[], projectDirHint?: string) => Promise<void>;
  readonly usesBuiltInChatSurface: (source: LlmSourceTag) => boolean;
}

export async function runStepCommand(
  deps: StepRunnerDeps,
  step: unknown,
  kind: "runStep" | "runCritique" | "resetStep",
  projectDirHint: string | undefined,
): Promise<void> {
  if (typeof step !== "string" || step.trim().length === 0) {
    await vscode.window.showErrorMessage(`sim-flow: ${kind} needs a step id (e.g. "DM0").`);
    return;
  }
  const source = (vscode.workspace.getConfiguration("sim-flow").get<string>("llm.source") ??
    "vscode") as LlmSourceTag;
  switch (kind) {
    case "runStep":
      if (isTerminalLlmSource(source)) {
        await runStepInTerminal(deps, step, "work", source, projectDirHint);
        return;
      }
      if (deps.usesBuiltInChatSurface(source)) {
        await openChatForStep(step, "work", projectDirHint);
        return;
      }
      if (await tryLaunchStepInChatPanel(deps, step, "work", projectDirHint)) {
        return;
      }
      await openChatForStep(step, "work", projectDirHint);
      return;
    case "runCritique":
      if (isTerminalLlmSource(source)) {
        await runStepInTerminal(deps, step, "critique", source, projectDirHint);
        return;
      }
      if (deps.usesBuiltInChatSurface(source)) {
        await openChatForStep(step, "critique", projectDirHint);
        return;
      }
      if (await tryLaunchStepInChatPanel(deps, step, "critique", projectDirHint)) {
        return;
      }
      await openChatForStep(step, "critique", projectDirHint);
      return;
    case "resetStep":
      await deps.runCliInTerminal(["reset", step], projectDirHint);
      return;
  }
}

async function tryLaunchStepInChatPanel(
  deps: StepRunnerDeps,
  step: string,
  kind: "work" | "critique",
  projectDirHint: string | undefined,
): Promise<boolean> {
  const source = (vscode.workspace.getConfiguration("sim-flow").get<string>("llm.source") ??
    "vscode") as LlmSourceTag;
  if (isTerminalLlmSource(source) || !deps.chatPanelProvider) {
    return false;
  }
  await deps.chatPanelProvider.launchStepSession(step, kind, projectDirHint);
  return true;
}

async function runStepInTerminal(
  deps: StepRunnerDeps,
  step: string,
  kind: "work" | "critique",
  source: LlmSourceTag,
  projectDirHint: string | undefined,
): Promise<void> {
  const sub: string[] = ["session", `${step}.${kind}`, "--llm-backend", cliBackendArgFor(source)];
  const model = vscode.workspace.getConfiguration("sim-flow").get<string>("llm.model")?.trim();
  if (model && model.length > 0) {
    sub.push("--llm-model", model);
  }
  await deps.runCliInTerminal(sub, projectDirHint);
}

async function openChatForStep(
  step: string,
  kind: "work" | "critique",
  projectDirHint: string | undefined,
): Promise<void> {
  const projectFlag = projectDirHint ? ` --project ${shellQuote(projectDirHint)}` : "";
  const query = `@sim-flow /step ${step}.${kind}${projectFlag}`;
  await openChatWithQuery(query);
}

async function openChatForAuto(
  specPath: string | undefined,
  projectDirHint: string | undefined,
): Promise<void> {
  const specFlag = specPath?.trim() ? ` --spec ${shellQuote(specPath.trim())}` : "";
  const projectFlag = projectDirHint ? ` --project ${shellQuote(projectDirHint)}` : "";
  const query = `@sim-flow /auto${specFlag}${projectFlag}`;
  await openChatWithQuery(query);
}

/**
 * Hand the dashboard's Play click off to the chat participant for
 * non-CLI LLM sources. Despite the underlying CLI subcommand being
 * `sim-flow auto`, this is NOT specifically the "automated mode"
 * red-Play path -- this is the general "drive the flow" launch.
 * Whether the agent runs unattended or with the user in the loop is
 * controlled by sim-flow's `auto` flag (set by the participant /
 * orchestrator), independent of how the launch is named here.
 */
export async function runFlowChatCommand(
  deps: StepRunnerDeps,
  specPath: string | undefined,
  projectDirHint: string | undefined,
): Promise<void> {
  const source = (vscode.workspace.getConfiguration("sim-flow").get<string>("llm.source") ??
    "vscode") as LlmSourceTag;
  if (deps.usesBuiltInChatSurface(source)) {
    await openChatForAuto(specPath, projectDirHint);
    return;
  }
  if (!deps.chatPanelProvider) {
    await vscode.window.showErrorMessage(
      "sim-flow: chat panel is not available yet. Reload the window and try again.",
    );
    return;
  }
  await deps.chatPanelProvider.launchAutoSession(specPath, projectDirHint);
}

/**
 * Launch `sim-flow auto --llm-backend <name>` in the project's
 * terminal. Used by the dashboard's Play button when the user has
 * picked a CLI-agent source (`claude-cli`, `codex-cli`,
 * `gh-copilot-cli`) -- those don't have an HTTP backend the chat
 * participant can drive, so we hand the work to the in-terminal
 * subprocess instead. Auth comes from the user's existing CLI
 * login (claude /login, codex login, gh auth login).
 *
 * Note: this is the **general** "run the flow" launch path. It is
 * NOT specifically the fully-automated red-Play flow (which lives
 * in `runFullyAutomatedInTerminal` and forces `--session-mode
 * per-step` plus a required spec). The CLI subcommand happens to
 * be named `sim-flow auto` because it drives the orchestrator, but
 * the orchestrator's automated-vs-manual mode is set separately.
 */
export async function runFlowInTerminal(
  deps: StepRunnerDeps,
  backend: string | undefined,
  specPath: string | undefined,
  projectDirHint: string | undefined,
): Promise<void> {
  if (!backend) {
    await vscode.window.showErrorMessage(
      "sim-flow: missing CLI agent backend; expected `claude`, `codex`, or `gh-copilot`.",
    );
    return;
  }
  const sub: string[] = ["auto", "--llm-backend", backend];
  // Pull in `sim-flow.session.mode` so the user's per-step / single
  // choice in the dashboard's settings actually reaches the CLI.
  // Single-session opens the control socket the dashboard buttons
  // talk to via `src/session/control-client.ts`.
  const sessionMode = (
    vscode.workspace.getConfiguration("sim-flow").get<string>("session.mode") ?? "per-step"
  ).trim();
  if (sessionMode === "single") {
    sub.push("--session-mode", "single");
  } else {
    sub.push("--session-mode", "per-step");
  }
  // Honor `sim-flow.llm.model` if set so the user's chosen claude
  // model id (`sonnet` / `opus` / etc.) flows through.
  const model = vscode.workspace.getConfiguration("sim-flow").get<string>("llm.model")?.trim();
  if (model && model.length > 0) {
    sub.push("--llm-model", model);
  }
  const trimmedSpec = specPath?.trim() ?? "";
  if (trimmedSpec.length > 0) {
    sub.push("--spec", trimmedSpec);
  } else {
    // No spec: drop DM0.work into interactive mode so the agent
    // asks the user what to build instead of fabricating a spec
    // from thin air. The DM0 work instructions already include the
    // "if no spec.md, walk the user through filling it in" branch
    // -- it only fires when the orchestrator's auto flag is off,
    // which `--dm0-interactive` controls. Subsequent steps still
    // run unattended once DM0 has produced a real spec.md.
    sub.push("--dm0-interactive");
  }
  await deps.runCliInTerminal(sub, projectDirHint);
}

/**
 * End-to-end automated flow for CLI agents (claude / codex /
 * gh-copilot). Spawns `sim-flow auto --session-mode per-step`
 * with the spec pre-ingested; the per-step PTY driver auto-walks
 * DM0 → DM4b in order, spawning a fresh agent per step. No
 * `--dm0-interactive` (would defeat the unattended intent), no
 * single-session control socket (the dashboard's manual buttons
 * are not used in this mode).
 */
export async function runFullyAutomatedInTerminal(
  deps: StepRunnerDeps,
  backend: string | undefined,
  specPath: string | undefined,
  projectDirHint: string | undefined,
): Promise<void> {
  if (!backend) {
    await vscode.window.showErrorMessage(
      "sim-flow: missing CLI agent backend; expected `claude`, `codex`, or `gh-copilot`.",
    );
    return;
  }
  if (!specPath || !specPath.trim()) {
    await vscode.window.showErrorMessage("sim-flow: fully-automated mode requires a spec path.");
    return;
  }
  const sub: string[] = [
    "auto",
    "--llm-backend",
    backend,
    "--session-mode",
    "per-step",
    "--spec",
    specPath.trim(),
  ];
  const model = vscode.workspace.getConfiguration("sim-flow").get<string>("llm.model")?.trim();
  if (model && model.length > 0) {
    sub.push("--llm-model", model);
  }
  await deps.runCliInTerminal(sub, projectDirHint);
}

async function openChatWithQuery(query: string): Promise<void> {
  try {
    await vscode.commands.executeCommand("workbench.action.chat.open", { query });
  } catch {
    try {
      await vscode.commands.executeCommand("workbench.action.chat.open", query);
    } catch (err) {
      await vscode.window.showErrorMessage(
        `sim-flow: could not open a chat tab — ${String((err as Error).message ?? err)}.`,
      );
    }
  }
}

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_./:@%^+=-]+$/.test(value)) {
    return value;
  }
  return `"${value.replace(/"/g, '\\"')}"`;
}
