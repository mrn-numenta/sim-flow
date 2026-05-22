// Entry point for the sim-flow VS Code extension.

import * as fs from "node:fs";
import * as path from "node:path";

import * as vscode from "vscode";

import { clearApiKey, setApiKey } from "./apiKey";
import {
  bundledCandidates,
  bundledFrameworkDocsRoot,
  resolveBinary,
  setBundledRoot,
  SimFlowCli,
  SimFlowCliError,
} from "./cli";
import { PICK_PROJECT_NEW, findProjectCandidates, pickProject, resolveProjectDir } from "./context";
import { registerChatParticipant } from "./participant";
import { SimFlowTerminal } from "./terminal";
import { CHAT_PANEL_CONTAINER_ID, CHAT_PANEL_VIEW_ID, ChatPanelProvider } from "./chatPanel/host";
import { AutoSessionManager } from "./chatPanel/autoSessionManager";
import { cleanupStalePidsAsync } from "./session/processRegistry";
import { DashboardHost } from "./webview/host";
import { type LlmSourceTag } from "./webview/messages";
import { PerfPanelHost } from "./perfPanel/host";
import { attachWatcherCommand } from "./extension/attachWatcher";
import { dumpAvailableLmModels, testLmModel } from "./extension/lmStudio";
import {
  runFlowChatCommand,
  runFlowInTerminal,
  runFullyAutomatedInTerminal,
  runStepCommand,
  type StepRunnerDeps,
} from "./extension/stepRunner";

const dashboardHosts = new Map<string, DashboardHost>();
const perfPanelHosts = new Map<string, PerfPanelHost>();
const terminals = new Map<string, SimFlowTerminal>();
let chatPanelProvider: ChatPanelProvider | undefined;
let autoSessionManager: AutoSessionManager | undefined;
let extensionContext: vscode.ExtensionContext | undefined;

/** Best-effort access to the extension context. Throws on access
 * before `activate` ran (which shouldn't happen for any user-
 * triggered command since activation is `onStartupFinished`). */
function globalContext(): vscode.ExtensionContext {
  if (!extensionContext) {
    throw new Error("sim-flow: extension context not initialised");
  }
  return extensionContext;
}

export function activate(context: vscode.ExtensionContext): void {
  console.log("sim-flow: extension activated");
  extensionContext = context;
  setBundledRoot(context.extensionUri.fsPath);
  // Reap orphaned `sim-flow` processes left behind by a prior
  // extension run (host crash, OS reboot, killed extension host
  // before disconnect, etc.). Each pump writes a pid record under
  // `<project>/.sim-flow/pids/<sessionId>.json` on spawn and clears
  // it on clean exit; anything that survives is a leak we should
  // clean up before the user starts a new session and ends up with
  // duplicates.
  void reapOrphanedSimFlowProcesses();
  autoSessionManager = new AutoSessionManager(context.workspaceState);
  chatPanelProvider = new ChatPanelProvider(
    context.extensionUri,
    context.workspaceState,
    context.secrets,
    autoSessionManager,
  );

  context.subscriptions.push(
    autoSessionManager,
    chatPanelProvider,
    vscode.window.registerWebviewViewProvider(CHAT_PANEL_VIEW_ID, chatPanelProvider, {
      // Keep the webview's HTML + JS state alive while it's hidden
      // (e.g. while the user has an editor tab focused on top). Without
      // this VS Code tears the webview down and rebuilds from scratch
      // on every visibility flip, which the user perceives as the chat
      // panel reverting to "no session / no project".
      webviewOptions: { retainContextWhenHidden: true },
    }),
    vscode.commands.registerCommand("sim-flow.openChatPanel", () => openChatPanel()),
    vscode.commands.registerCommand("sim-flow.openDashboard", () => openDashboard(context)),
    vscode.commands.registerCommand("sim-flow.toggleExperimentalUi", () =>
      toggleExperimentalUiCommand(),
    ),
    vscode.commands.registerCommand("sim-flow.openPerfPanel", () => openPerfPanel(context)),
    vscode.commands.registerCommand("sim-flow.runStep", (step: unknown, projectDir?: unknown) =>
      runStepCommand(stepRunnerDeps(), step, "runStep", asString(projectDir)),
    ),
    vscode.commands.registerCommand("sim-flow.runCritique", (step: unknown, projectDir?: unknown) =>
      runStepCommand(stepRunnerDeps(), step, "runCritique", asString(projectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runFlow",
      (specPath?: unknown, projectDir?: unknown) =>
        runFlowChatCommand(stepRunnerDeps(), asString(specPath), asString(projectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runFlowTerminal",
      (backend?: unknown, specPath?: unknown, projectDir?: unknown) =>
        runFlowInTerminal(
          stepRunnerDeps(),
          asString(backend),
          asString(specPath),
          asString(projectDir),
        ),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runAutoFullyAutomatedTerminal",
      (backend?: unknown, specPath?: unknown, projectDir?: unknown) =>
        runFullyAutomatedInTerminal(
          stepRunnerDeps(),
          asString(backend),
          asString(specPath),
          asString(projectDir),
        ),
    ),
    vscode.commands.registerCommand("sim-flow.resetStep", (step: unknown, projectDir?: unknown) =>
      runStepCommand(stepRunnerDeps(), step, "resetStep", asString(projectDir)),
    ),
    vscode.commands.registerCommand("sim-flow.setApiKey", () => setApiKey(context)),
    vscode.commands.registerCommand("sim-flow.clearApiKey", () => clearApiKey(context)),
    vscode.commands.registerCommand("sim-flow.dumpAvailableLmModels", () =>
      dumpAvailableLmModels(),
    ),
    vscode.commands.registerCommand("sim-flow.attachWatcher", () =>
      attachWatcherCommand({
        chatPanelProvider,
        openDashboardForProject: (projectDir) =>
          openDashboardForProject(globalContext(), projectDir),
      }),
    ),
    vscode.commands.registerCommand("sim-flow.testLmModel", () => testLmModel(context)),
    vscode.commands.registerCommand("sim-flow.switchProject", () => switchProjectCommand(context)),
    vscode.commands.registerCommand(
      "sim-flow.newProject",
      (name?: unknown, currentProjectDir?: unknown) =>
        newProjectCommand(context, asString(name), asString(currentProjectDir)),
    ),
    vscode.commands.registerCommand("sim-flow.renameProject", (currentProjectDir?: unknown) =>
      renameProjectCommand(context, asString(currentProjectDir)),
    ),
    { dispose: disposeAllResources },
  );

  registerChatParticipant(context);

  if (getBooleanSetting("dashboard.openOnActivate", false)) {
    void vscode.commands.executeCommand("sim-flow.openDashboard");
  }
}

export function deactivate(): void {
  disposeAllResources();
  console.log("sim-flow: extension deactivated");
}

async function openChatPanel(): Promise<void> {
  const commandId = `workbench.view.extension.${CHAT_PANEL_CONTAINER_ID}`;
  try {
    await vscode.commands.executeCommand(commandId);
  } catch (error) {
    console.warn(`sim-flow: failed to reveal chat panel via ${commandId}`, error);
    void vscode.window.showErrorMessage(
      'sim-flow: unable to reveal the chat panel automatically. Try "View: Open View" and select "sim-flow Chat".',
    );
  }
}

/**
 * Resolve symlinks (`fs.realpath`) so the path matches whatever
 * `vscode.window.activeTextEditor.document.uri.fsPath` produces for
 * a file under it. Falls back to the input on error (e.g. the
 * directory was just removed) -- the dashboard open call will still
 * proceed but its `options.projectDir` may not match the chat
 * panel's record.
 */
function canonicalizePath(p: string): string {
  try {
    return fs.realpathSync(p);
  } catch {
    return p;
  }
}

function disposeAllResources(): void {
  for (const host of dashboardHosts.values()) {
    host.dispose();
  }
  dashboardHosts.clear();
  for (const term of terminals.values()) {
    term.dispose();
  }
  terminals.clear();
  // Dispose the AutoSessionManager and ChatPanelProvider explicitly
  // so any active JSONL pumps get SIGTERM before VS Code finishes
  // tearing down the extension host. They're also in
  // `context.subscriptions`, but the order in which subscriptions
  // are disposed isn't guaranteed; running them here makes sure the
  // pump children are signaled even when `deactivate()` fires first.
  if (autoSessionManager) {
    try {
      autoSessionManager.dispose();
    } catch (err) {
      console.error(
        `sim-flow: autoSessionManager dispose failed: ${(err as Error).message ?? String(err)}`,
      );
    }
    autoSessionManager = undefined;
  }
  if (chatPanelProvider) {
    try {
      chatPanelProvider.dispose();
    } catch (err) {
      console.error(
        `sim-flow: chatPanelProvider dispose failed: ${(err as Error).message ?? String(err)}`,
      );
    }
    chatPanelProvider = undefined;
  }
}

/**
 * Open (or reveal) the dashboard for a sim-flow project. Scans the
 * workspace for `.sim-flow/state.toml` files; if more than one is
 * found, the user picks which project's dashboard to open. Each
 * selected project gets its own `DashboardHost` with an isolated
 * file watcher.
 */
async function openDashboard(context: vscode.ExtensionContext): Promise<void> {
  // Prefer the project the chat panel last anchored to so opening
  // the dashboard never asks the user to re-pick what they're
  // already working on. The chat panel writes
  // `sim-flow.chatPanel.lastProjectDir` to workspaceState on every
  // launch; if it's still a valid sim-flow project, use it
  // verbatim and skip the picker entirely.
  const remembered = context.workspaceState.get<string>("sim-flow.chatPanel.lastProjectDir");
  if (remembered && fs.existsSync(path.join(remembered, ".sim-flow", "state.toml"))) {
    await openDashboardForProject(context, remembered);
    return;
  }
  const projectDir = await selectProjectDir();
  if (!projectDir) {
    return;
  }
  await openDashboardForProject(context, projectDir);
}

async function openDashboardForProject(
  context: vscode.ExtensionContext,
  projectDir: string,
): Promise<void> {
  const binary = tryResolveBinary();
  if (!binary) {
    return;
  }

  // Canonicalize the project path so the dashboard map's key is
  // stable regardless of whether the caller passed the raw string
  // (e.g. `/tmp/...` from the watcher registry) or the realpath
  // (e.g. `/private/tmp/...` from VS Code's editor URI). Without
  // this, attaching to a watcher creates a SECOND DashboardHost
  // for the canonical path while a previous host still exists for
  // the raw path -- one shows VIEWING / no Connect, the other
  // still shows Connect, and the user can land on either. The
  // `activeSession()` filter inside the host also compares
  // string-equal against this same `projectDir`, so the session
  // record (also canonicalized at attach time) lines up only when
  // both ends agree.
  const canonicalDir = canonicalizePath(projectDir);

  let host = dashboardHosts.get(canonicalDir);
  if (!host) {
    const cli = new SimFlowCli({
      binary,
      projectDir: canonicalDir,
      foundationRoot: getStringSetting("foundationRoot", ""),
    });
    host = new DashboardHost({
      extensionUri: context.extensionUri,
      projectDir: canonicalDir,
      cli,
      workspaceState: context.workspaceState,
      autoSessions: autoSessionManager,
    });
    dashboardHosts.set(canonicalDir, host);
  }
  await host.open();
}

async function toggleExperimentalUiCommand(): Promise<void> {
  const config = vscode.workspace.getConfiguration("sim-flow");
  const enabled = config.get<boolean>("dashboard.experimentalUi") === true;
  const next = !enabled;
  await config.update("dashboard.experimentalUi", next, vscode.ConfigurationTarget.Workspace);
  void vscode.window.showInformationMessage(
    next
      ? "sim-flow: experimental dashboard UI enabled."
      : "sim-flow: reverted to the standard dashboard UI.",
  );
}

async function openPerfPanel(context: vscode.ExtensionContext): Promise<void> {
  const projectDir = await selectProjectDir();
  if (!projectDir) {
    return;
  }
  const binary = tryResolveBinary();
  if (!binary) {
    return;
  }
  const canonicalDir = canonicalizePath(projectDir);
  let host = perfPanelHosts.get(canonicalDir);
  if (!host) {
    const cli = new SimFlowCli({
      binary,
      projectDir: canonicalDir,
      foundationRoot: getStringSetting("foundationRoot", ""),
    });
    host = new PerfPanelHost({
      extensionUri: context.extensionUri,
      projectDir: canonicalDir,
      cli,
      binary,
    });
    perfPanelHosts.set(canonicalDir, host);
  }
  await host.open();
}

async function switchProjectCommand(context: vscode.ExtensionContext): Promise<void> {
  const candidates = await findProjectCandidates();
  if (candidates.length === 0) {
    await vscode.commands.executeCommand("sim-flow.newProject");
    return;
  }
  const picked = await pickProject(candidates, { allowNew: true });
  if (!picked) {
    return;
  }
  if (picked === PICK_PROJECT_NEW) {
    await vscode.commands.executeCommand("sim-flow.newProject");
    return;
  }
  await openDashboardForProject(context, picked);
}

async function newProjectCommand(
  context: vscode.ExtensionContext,
  nameArg: string | undefined,
  currentProjectDir: string | undefined,
): Promise<void> {
  const simModelsRoot = findSimModelsWorkspaceRoot();
  if (!simModelsRoot) {
    void vscode.window.showErrorMessage(
      "sim-flow only creates projects inside the sim-models repository. Open sim-models as a workspace root and try again.",
    );
    return;
  }
  const binary = tryResolveBinary();
  if (!binary) {
    return;
  }
  const userDir = path.join(simModelsRoot, "users", currentUsername());
  try {
    fs.mkdirSync(userDir, { recursive: true });
  } catch (err) {
    void vscode.window.showErrorMessage(
      `sim-flow: could not create ${userDir}: ${(err as Error).message ?? String(err)}`,
    );
    return;
  }

  let name: string | undefined;
  if (nameArg && nameArg.trim().length > 0) {
    name = nameArg.trim();
  } else {
    name = await vscode.window.showInputBox({
      title: "New sim-flow project",
      prompt: `Project name. It will be created under ${userDir}.`,
      placeHolder: "e.g. my-accelerator",
      ignoreFocusOut: true,
      validateInput: (v) => {
        const t = v.trim();
        if (t.length === 0) {
          return "name is required";
        }
        if (!/^[a-zA-Z0-9._-]+$/.test(t)) {
          return "use letters, digits, ., _, -";
        }
        if (fs.existsSync(path.join(userDir, t))) {
          return `${path.join(userDir, t)} already exists`;
        }
        return undefined;
      },
    });
  }
  if (!name) {
    return;
  }

  const cli = new SimFlowCli({
    binary,
    projectDir: currentProjectDir ?? simModelsRoot,
    foundationRoot: getStringSetting("foundationRoot", ""),
  });
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `sim-flow: creating project "${name}"`,
        cancellable: false,
      },
      async (progress) => {
        progress.report({ message: "scaffolding project files…" });
        const created = await cli.newModel({ name, destination: userDir });
        progress.report({ message: "opening dashboard…" });
        await openDashboardForProject(context, created.project_dir);
        return created;
      },
    );
    void vscode.window.showInformationMessage(
      `Created project "${name}" at ${result.project_dir}.`,
    );
  } catch (err) {
    await vscode.window.showErrorMessage(
      `sim-flow new model "${name}" failed: ${String((err as Error).message ?? err)}`,
    );
  }
}

/**
 * Resolve a project for a dashboard action:
 * 1. If exactly one project is visible in the workspace, use it.
 * 2. If multiple exist, QuickPick.
 * 3. Walk-up from the active editor for projects that sit outside
 *    workspace.findFiles reach.
 * 4. If none of the above find a project, create a new user project
 *    under `sim-models/users/<USER>/`.
 */
async function selectProjectDir(): Promise<string | undefined> {
  const candidates = await findProjectCandidates();
  if (candidates.length > 0) {
    const picked = await pickProject(candidates, { allowNew: true });
    if (picked === PICK_PROJECT_NEW) {
      // newProjectCommand opens its own dashboard for the freshly
      // created project, so we return undefined to skip the caller's
      // `openDashboardForProject` step.
      await vscode.commands.executeCommand("sim-flow.newProject");
      return undefined;
    }
    return picked;
  }
  const fallback = resolveProjectDir();
  if (fallback && fs.existsSync(path.join(fallback, ".sim-flow", "state.toml"))) {
    return fallback;
  }
  await vscode.commands.executeCommand("sim-flow.newProject");
  return undefined;
}

/**
 * Find the workspace folder that points at the sim-models repo. We
 * accept any workspace folder whose basename is `sim-models` or that
 * carries the library-root signature (`docs/modeling-guide/` +
 * `examples/`). The signature check is robust to forks renamed to
 * something other than `sim-models`.
 */
function findSimModelsWorkspaceRoot(): string | undefined {
  const folders = vscode.workspace.workspaceFolders ?? [];
  for (const folder of folders) {
    if (path.basename(folder.uri.fsPath) === "sim-models") {
      return folder.uri.fsPath;
    }
  }
  for (const folder of folders) {
    if (looksLikeLibraryRoot(folder.uri.fsPath)) {
      return folder.uri.fsPath;
    }
  }
  return undefined;
}

function looksLikeLibraryRoot(dir: string): boolean {
  try {
    return (
      fs.statSync(path.join(dir, "docs", "modeling-guide")).isDirectory() &&
      fs.statSync(path.join(dir, "examples")).isDirectory()
    );
  } catch {
    return false;
  }
}

function currentUsername(): string {
  return process.env.USER ?? process.env.USERNAME ?? process.env.LOGNAME ?? "user";
}

/**
 * Rename the active project on disk and re-open the dashboard.
 * Renaming is `mv <parent>/<old> <parent>/<new>` followed by a
 * dashboard re-open against the new path. We dispose the old
 * dashboard host and per-project terminal first so the file watcher
 * lets go of the moved directory and the user doesn't end up with
 * two dashboards pointing at the same project.
 */
async function renameProjectCommand(
  context: vscode.ExtensionContext,
  currentProjectDir: string | undefined,
): Promise<void> {
  if (!currentProjectDir) {
    void vscode.window.showErrorMessage(
      "sim-flow: rename requires an active project. Open the dashboard first.",
    );
    return;
  }
  const oldName = path.basename(currentProjectDir);
  const parent = path.dirname(currentProjectDir);
  const newName = await vscode.window.showInputBox({
    title: `Rename project — currently \`${oldName}\``,
    prompt: "New project name (must not already exist).",
    value: oldName,
    valueSelection: [0, oldName.length],
    ignoreFocusOut: true,
    validateInput: (v) => {
      const t = v.trim();
      if (t.length === 0) {
        return "name is required";
      }
      if (!/^[a-zA-Z0-9._-]+$/.test(t)) {
        return "use letters, digits, ., _, -";
      }
      if (t === oldName) {
        return "name unchanged";
      }
      if (fs.existsSync(path.join(parent, t))) {
        return `${path.join(parent, t)} already exists`;
      }
      return undefined;
    },
  });
  if (!newName) {
    return;
  }
  const newDir = path.join(parent, newName.trim());

  // Dispose pre-rename so the watcher isn't holding the source path
  // when the rename lands.
  const oldHost = dashboardHosts.get(currentProjectDir);
  if (oldHost) {
    oldHost.dispose();
    dashboardHosts.delete(currentProjectDir);
  }
  const oldTerminal = terminals.get(currentProjectDir);
  if (oldTerminal) {
    oldTerminal.dispose();
    terminals.delete(currentProjectDir);
  }

  try {
    fs.renameSync(currentProjectDir, newDir);
  } catch (err) {
    void vscode.window.showErrorMessage(
      `sim-flow: rename to ${newDir} failed: ${(err as Error).message ?? String(err)}`,
    );
    return;
  }
  void vscode.window.showInformationMessage(`Renamed project: ${oldName} -> ${newName.trim()}.`);
  await openDashboardForProject(context, newDir);
}

/**
 * Dashboard webview buttons dispatch `sim-flow.runStep <step>` and
 * `sim-flow.resetStep <step>` with the step id as the single argument.
 *
 * - `runStep` opens a new chat tab seeded with
 *   `@sim-flow /step <step>.work`. The chat participant's
 *   `handleStep` loads the instruction file for that step, streams
 *   the LLM's opening message (which tells the user what input to
 *   paste, e.g. a workload spec for DM0), and writes the step's
 *   artifacts as the session progresses.
 * - `resetStep` runs the fast one-shot `sim-flow reset <step>` in
 *   the shared terminal; the file watcher refreshes the dashboard.
 */

/**
 * Walk every sim-flow project under the open workspace folders and
 * kill any `sim-flow` processes whose pid records are still on disk
 * — those are orphans from a prior extension run that died without a
 * clean disconnect. Best-effort: failures are logged to the console
 * and never thrown, so a permission error on one project doesn't
 * prevent activation. The kill itself is SIGTERM, which gives the
 * orchestrator a chance to flush logs before exiting; if a stuck
 * process ignores SIGTERM, the user will see it in `ps` and can deal
 * with it manually.
 */
async function reapOrphanedSimFlowProcesses(): Promise<void> {
  let candidates: string[];
  try {
    candidates = await findProjectCandidates();
  } catch (err) {
    console.error(
      `sim-flow: pid cleanup: failed to enumerate projects: ${(err as Error).message ?? String(err)}`,
    );
    return;
  }
  let totalKilled = 0;
  let totalStale = 0;
  for (const projectDir of candidates) {
    try {
      const summary = await cleanupStalePidsAsync(projectDir);
      totalKilled += summary.killed;
      totalStale += summary.stale;
      if (summary.killed > 0 || summary.skipped > 0) {
        console.log(
          `sim-flow: pid cleanup [${projectDir}] ` +
            `killed=${summary.killed} stale=${summary.stale} ` +
            `skipped=${summary.skipped} total=${summary.total}`,
        );
      }
    } catch (err) {
      console.error(
        `sim-flow: pid cleanup [${projectDir}] failed: ${(err as Error).message ?? String(err)}`,
      );
    }
  }
  if (totalKilled > 0) {
    console.log(
      `sim-flow: reaped ${totalKilled} orphaned sim-flow process(es); ${totalStale} pid record(s) were already stale.`,
    );
  }
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function usesBuiltInChatSurface(source: LlmSourceTag): boolean {
  return source === "vscode";
}

/**
 * Bind the cross-extension hooks the stepRunner module needs --
 * chat-panel provider, in-terminal launcher, and the LLM-source
 * predicate. Called fresh on every command invocation so the
 * provider reference reflects the current state (it's `undefined`
 * before `activate` finishes wiring it up).
 */
function stepRunnerDeps(): StepRunnerDeps {
  return {
    chatPanelProvider,
    runCliInTerminal,
    usesBuiltInChatSurface,
  };
}

/**
 * Resolve project dir + CLI + a per-project terminal, then send a
 * `sim-flow <subcommand>` command to the terminal. Each project has
 * its own terminal so parallel runs in a multi-project workspace
 * don't interleave output.
 */
async function runCliInTerminal(subcommand: string[], projectDirHint?: string): Promise<void> {
  const projectDir = projectDirHint ?? (await selectProjectDir());
  if (!projectDir) {
    return;
  }
  const binary = tryResolveBinary();
  if (!binary) {
    return;
  }
  const cli = new SimFlowCli({
    binary,
    projectDir,
    foundationRoot: getStringSetting("foundationRoot", ""),
  });
  const terminal = ensureTerminal(projectDir);
  terminal.run(cli.buildCommandLine(subcommand));
}

function ensureTerminal(projectDir: string): SimFlowTerminal {
  let term = terminals.get(projectDir);
  if (!term) {
    const frameworkDocsRoot = bundledFrameworkDocsRoot();
    const debugTokens = resolveDebugTokens();
    const env: Record<string, string> = {};
    if (frameworkDocsRoot) {
      env.SIM_FLOW_FRAMEWORK_DOCS_ROOT = frameworkDocsRoot;
    }
    if (debugTokens.length > 0) {
      env.SIM_FOUNDATION_DEBUG = debugTokens;
    }
    term = new SimFlowTerminal({
      projectDir,
      name: terminalNameFor(projectDir),
      env: Object.keys(env).length > 0 ? env : undefined,
    });
    terminals.set(projectDir, term);
  }
  return term;
}

function resolveDebugTokens(): string {
  const settingTokens = (
    vscode.workspace.getConfiguration("sim-flow").get<string[]>("debug") ?? []
  ).join(",");
  if (settingTokens.length > 0) {
    return settingTokens;
  }
  return (process.env["SIM_FOUNDATION_DEBUG"] ?? "").trim();
}

function terminalNameFor(projectDir: string): string {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.length <= 1) {
    return "sim-flow";
  }
  for (const folder of folders) {
    if (projectDir === folder.uri.fsPath || projectDir.startsWith(folder.uri.fsPath + "/")) {
      return `sim-flow: ${folder.name}`;
    }
  }
  return "sim-flow";
}

function tryResolveBinary(): string | undefined {
  try {
    return resolveBinary({
      settingOverride: getStringSetting("binaryPath", ""),
      bundledCandidates,
    });
  } catch (err) {
    const detail = err instanceof SimFlowCliError ? err.message : `Unknown error: ${String(err)}`;
    void vscode.window.showErrorMessage(`sim-flow CLI not found. ${detail}`);
    return undefined;
  }
}

function getBooleanSetting(key: string, fallback: boolean): boolean {
  const value = vscode.workspace.getConfiguration("sim-flow").get<boolean>(key);
  return typeof value === "boolean" ? value : fallback;
}

function getStringSetting(key: string, fallback: string): string {
  const value = vscode.workspace.getConfiguration("sim-flow").get<string>(key);
  return typeof value === "string" && value.length > 0 ? value : fallback;
}

/**
 * Run a battery of LM-API probe variants so we can isolate why a
 * specific provider returns an empty stream:
 *
 *   1. `request.stream` only (the production path).
 *   2. `request.text` only (in case the provider implements one but
 *      not the other).
 *   3. Stream + `modelOptions: { max_tokens: 4096 }` (Anthropic's
 *      required field — Claude Code might forward it).
 *   4. Stream + a known-good fallback model (`copilot/gpt-4o`) so we
 *      can confirm the LM-API plumbing itself works.
 *
 * Each probe makes its own `sendRequest` so iterators don't fight
 * over a shared underlying source.
 */
