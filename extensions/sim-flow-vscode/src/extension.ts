// Entry point for the sim-flow VS Code extension.

import * as fs from "node:fs";
import * as path from "node:path";

import * as vscode from "vscode";

import { clearApiKey, setApiKey } from "./apiKey";
import {
  bundledCandidates,
  resolveBinary,
  setBundledRoot,
  SimFlowCli,
  SimFlowCliError,
} from "./cli";
import { findProjectCandidates, pickProject, resolveProjectDir } from "./context";
import { registerChatParticipant } from "./participant";
import { SimFlowTerminal } from "./terminal";
import { DashboardHost } from "./webview/host";

const dashboardHosts = new Map<string, DashboardHost>();
const terminals = new Map<string, SimFlowTerminal>();

export function activate(context: vscode.ExtensionContext): void {
  console.log("sim-flow: extension activated");
  setBundledRoot(context.extensionUri.fsPath);

  context.subscriptions.push(
    vscode.commands.registerCommand("sim-flow.openDashboard", () => openDashboard(context)),
    vscode.commands.registerCommand("sim-flow.runStep", (step: unknown, projectDir?: unknown) =>
      runStepCommand(step, "runStep", asString(projectDir)),
    ),
    vscode.commands.registerCommand("sim-flow.runCritique", (step: unknown, projectDir?: unknown) =>
      runStepCommand(step, "runCritique", asString(projectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runAuto",
      (specPath?: unknown, projectDir?: unknown) =>
        runAutoCommand(asString(specPath), asString(projectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runAutoTerminal",
      (backend?: unknown, specPath?: unknown, projectDir?: unknown) =>
        runAutoInTerminal(asString(backend), asString(specPath), asString(projectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.runAutoFullyAutomatedTerminal",
      (backend?: unknown, specPath?: unknown, projectDir?: unknown) =>
        runFullyAutomatedInTerminal(
          asString(backend),
          asString(specPath),
          asString(projectDir),
        ),
    ),
    vscode.commands.registerCommand("sim-flow.resetStep", (step: unknown, projectDir?: unknown) =>
      runStepCommand(step, "resetStep", asString(projectDir)),
    ),
    vscode.commands.registerCommand("sim-flow.setApiKey", () => setApiKey(context)),
    vscode.commands.registerCommand("sim-flow.clearApiKey", () => clearApiKey(context)),
    vscode.commands.registerCommand("sim-flow.dumpAvailableLmModels", () =>
      dumpAvailableLmModels(),
    ),
    vscode.commands.registerCommand("sim-flow.testLmModel", () => testLmModel(context)),
    vscode.commands.registerCommand("sim-flow.switchProject", () => switchProjectCommand(context)),
    vscode.commands.registerCommand(
      "sim-flow.newProject",
      (name?: unknown, currentProjectDir?: unknown) =>
        newProjectCommand(context, asString(name), asString(currentProjectDir)),
    ),
    vscode.commands.registerCommand(
      "sim-flow.renameProject",
      (currentProjectDir?: unknown) =>
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

function disposeAllResources(): void {
  for (const host of dashboardHosts.values()) {
    host.dispose();
  }
  dashboardHosts.clear();
  for (const term of terminals.values()) {
    term.dispose();
  }
  terminals.clear();
}

/**
 * Open (or reveal) the dashboard for a sim-flow project. Scans the
 * workspace for `.sim-flow/state.toml` files; if more than one is
 * found, the user picks which project's dashboard to open. Each
 * selected project gets its own `DashboardHost` with an isolated
 * file watcher.
 */
async function openDashboard(context: vscode.ExtensionContext): Promise<void> {
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

  let host = dashboardHosts.get(projectDir);
  if (!host) {
    const cli = new SimFlowCli({
      binary,
      projectDir,
      foundationRoot: getStringSetting("foundationRoot", ""),
    });
    host = new DashboardHost({
      extensionUri: context.extensionUri,
      projectDir,
      cli,
      workspaceState: context.workspaceState,
    });
    dashboardHosts.set(projectDir, host);
  }
  await host.open();
}

async function switchProjectCommand(context: vscode.ExtensionContext): Promise<void> {
  const candidates = await findProjectCandidates();
  if (candidates.length === 0) {
    void vscode.window.showWarningMessage(
      "sim-flow: no projects found. Use \"New project...\" to create one.",
    );
    return;
  }
  const picked = await pickProject(candidates);
  if (!picked) {
    return;
  }
  await openDashboardForProject(context, picked);
}

async function newProjectCommand(
  context: vscode.ExtensionContext,
  nameArg: string | undefined,
  currentProjectDir: string | undefined,
): Promise<void> {
  const binary = tryResolveBinary();
  if (!binary) {
    return;
  }

  // Two-step prompt: first the parent directory (where the new project
  // will live), then the project name. We can't show both fields in
  // one modal -- VS Code's `showInputBox` is single-field and the only
  // multi-field surface is a full webview, which is overkill for a
  // bootstrap dialog. Chained inputs are the standard VS Code pattern
  // (compare: the built-in "Create New File" / "Move To" wizards).
  const defaultParent = defaultProjectDestination(currentProjectDir);
  const parent = await vscode.window.showInputBox({
    title: "New sim-flow project — directory (1 of 2)",
    prompt: "Parent directory in which the project folder will be created.",
    value: defaultParent,
    valueSelection: [defaultParent.length, defaultParent.length],
    ignoreFocusOut: true,
    validateInput: (v) => {
      const t = v.trim();
      if (t.length === 0) {return "directory is required";}
      if (!path.isAbsolute(t)) {return "use an absolute path";}
      // A non-existent path is fine -- the CLI mkdir-p's it -- but
      // an existing FILE at the same path is not.
      try {
        const stat = fs.statSync(t);
        if (!stat.isDirectory()) {return `${t} exists but is not a directory`;}
      } catch {
        // missing is OK
      }
      return undefined;
    },
  });
  if (!parent) {
    return;
  }
  const parentTrimmed = parent.trim();

  let name: string | undefined;
  if (nameArg && nameArg.trim().length > 0) {
    name = nameArg.trim();
  } else {
    const fullPathPreview = (candidate: string): string =>
      path.join(parentTrimmed, candidate.trim() || "<name>");
    name = await vscode.window.showInputBox({
      title: `New sim-flow project — name (2 of 2). Will be created at: ${fullPathPreview("<name>")}`,
      prompt: "Project name (appended to the directory above).",
      placeHolder: "e.g. my-accelerator",
      ignoreFocusOut: true,
      validateInput: (v) => {
        const t = v.trim();
        if (t.length === 0) {return "name is required";}
        if (!/^[a-zA-Z0-9._-]+$/.test(t)) {return "use letters, digits, ., _, -";}
        if (fs.existsSync(path.join(parentTrimmed, t))) {
          return `${path.join(parentTrimmed, t)} already exists`;
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
    projectDir: currentProjectDir ?? process.cwd(),
    foundationRoot: getStringSetting("foundationRoot", ""),
  });
  try {
    const result = await cli.newModel({ name, destination: parentTrimmed });
    void vscode.window.showInformationMessage(
      `Created project "${name}" at ${result.project_dir}.`,
    );
    await openDashboardForProject(context, result.project_dir);
  } catch (err) {
    await vscode.window.showErrorMessage(
      `sim-flow new model "${name}" failed: ${String((err as Error).message ?? err)}`,
    );
  }
}

/**
 * Default *parent* directory for a new project. The CLI's
 * `sim-flow new model <name> --destination <parent>` treats the
 * destination as the parent and appends `<name>` itself, so we MUST
 * NOT append `<name>` here -- doing that produces the doubled
 * `.../htm-smoke-test/htm-smoke-test/` that prompted the original
 * fix. Always resolves to `<sim-models>/users/<USER>` because the
 * extension is hard-coded to live inside that repo; if the workspace
 * isn't sim-models, every entry point that reaches this function has
 * already been gated by `bootstrapDefaultProject` showing an error.
 * The fallback branches stay only as defense for unusual call orders.
 */
function defaultProjectDestination(currentProjectDir: string | undefined): string {
  const username = currentUsername();
  const simModels = findSimModelsWorkspaceRoot();
  if (simModels) {
    return path.join(simModels, "users", username);
  }
  // Defensive fallback: walk up from the current project (if any) to
  // re-detect the library root signature. Reachable only when this
  // function is invoked outside the dashboard's bootstrap path.
  if (currentProjectDir) {
    let cursor = currentProjectDir;
    for (let i = 0; i < 16; i++) {
      if (looksLikeLibraryRoot(cursor)) {
        return path.join(cursor, "users", username);
      }
      const parent = path.dirname(cursor);
      if (parent === cursor) {break;}
      cursor = parent;
    }
  }
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
}

/**
 * Resolve a project for a dashboard action:
 * 1. If exactly one project is visible in the workspace, use it.
 * 2. If multiple exist, QuickPick.
 * 3. Walk-up from the active editor for projects that sit outside
 *    workspace.findFiles reach.
 * 4. Bootstrap: when none of the above find a project, verify the
 *    workspace is rooted at `sim-models` and auto-create
 *    `<sim-models>/users/<USER>/untitled_project`. The user picked
 *    this UX so the first launch lands on a usable project without
 *    a multi-step wizard.
 */
async function selectProjectDir(): Promise<string | undefined> {
  const candidates = await findProjectCandidates();
  if (candidates.length > 0) {
    return pickProject(candidates);
  }
  const fallback = resolveProjectDir();
  if (fallback && fs.existsSync(path.join(fallback, ".sim-flow", "state.toml"))) {
    return fallback;
  }
  return await bootstrapDefaultProject();
}

/**
 * Auto-bootstrap a default project when the workspace has none. The
 * extension is hard-coded to live inside the `sim-models` repo, so
 * this function:
 *
 *   1. Locates the sim-models workspace root (single-folder workspace
 *      OR one of the roots in a multi-root workspace). If absent we
 *      surface a popup explaining the constraint and bail; the
 *      dashboard never opens against a foreign repo.
 *   2. Ensures `<sim-models>/users/<USER>/` exists.
 *   3. Creates `untitled_project` (or the next available
 *      `untitled_project_N`) inside that user dir via the CLI's
 *      `new model`, and returns its path.
 *
 * If the project already exists on disk with a `state.toml` we just
 * return it -- subsequent bootstraps reuse the existing default.
 */
async function bootstrapDefaultProject(): Promise<string | undefined> {
  const simModelsRoot = findSimModelsWorkspaceRoot();
  if (!simModelsRoot) {
    void vscode.window.showErrorMessage(
      "sim-flow only runs against the sim-models repository. Open the sim-models repo as your workspace, or add it as a root in a multi-root workspace, then re-run \"Open Flow Dashboard\".",
    );
    return undefined;
  }
  const binary = tryResolveBinary();
  if (!binary) {
    return undefined;
  }
  const username = currentUsername();
  const userDir = path.join(simModelsRoot, "users", username);
  try {
    fs.mkdirSync(userDir, { recursive: true });
  } catch (err) {
    void vscode.window.showErrorMessage(
      `sim-flow: could not create ${userDir}: ${(err as Error).message ?? String(err)}`,
    );
    return undefined;
  }
  const projectName = pickFirstAvailableProjectName(userDir, "untitled_project");
  const existing = path.join(userDir, projectName);
  if (fs.existsSync(path.join(existing, ".sim-flow", "state.toml"))) {
    return existing;
  }
  const cli = new SimFlowCli({
    binary,
    projectDir: simModelsRoot,
    foundationRoot: getStringSetting("foundationRoot", ""),
  });
  try {
    const result = await cli.newModel({ name: projectName, destination: userDir });
    void vscode.window.showInformationMessage(
      `Created default project at ${result.project_dir}. Use "Rename..." to give it a permanent name.`,
    );
    return result.project_dir;
  } catch (err) {
    void vscode.window.showErrorMessage(
      `sim-flow: failed to create default project: ${(err as Error).message ?? String(err)}`,
    );
    return undefined;
  }
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
 * Find the first project name in the form `<base>` / `<base>_2` /
 * `<base>_3` / ... whose target directory either doesn't exist or
 * already holds a valid sim-flow project (state.toml present). Lets
 * the bootstrap reuse an existing `untitled_project` while still
 * sidestepping a half-created stub directory.
 */
function pickFirstAvailableProjectName(parentDir: string, base: string): string {
  for (let i = 1; i <= 1000; i++) {
    const name = i === 1 ? base : `${base}_${i}`;
    const target = path.join(parentDir, name);
    if (!fs.existsSync(target)) {
      return name;
    }
    if (fs.existsSync(path.join(target, ".sim-flow", "state.toml"))) {
      return name;
    }
  }
  // Astronomically unlikely; fall back to a timestamped name so we
  // don't loop forever.
  return `${base}_${Date.now()}`;
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
      if (t.length === 0) {return "name is required";}
      if (!/^[a-zA-Z0-9._-]+$/.test(t)) {return "use letters, digits, ., _, -";}
      if (t === oldName) {return "name unchanged";}
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
  void vscode.window.showInformationMessage(
    `Renamed project: ${oldName} -> ${newName.trim()}.`,
  );
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
async function runStepCommand(
  step: unknown,
  kind: "runStep" | "runCritique" | "resetStep",
  projectDirHint: string | undefined,
): Promise<void> {
  if (typeof step !== "string" || step.trim().length === 0) {
    await vscode.window.showErrorMessage(`sim-flow: ${kind} needs a step id (e.g. "DM0").`);
    return;
  }
  switch (kind) {
    case "runStep":
      await openChatForStep(step, "work", projectDirHint);
      return;
    case "runCritique":
      await openChatForStep(step, "critique", projectDirHint);
      return;
    case "resetStep":
      await runCliInTerminal(["reset", step], projectDirHint);
      return;
  }
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

async function runAutoCommand(
  specPath: string | undefined,
  projectDirHint: string | undefined,
): Promise<void> {
  const projectFlag = projectDirHint ? ` --project ${shellQuote(projectDirHint)}` : "";
  const trimmedSpec = specPath?.trim() ?? "";
  const specFlag = trimmedSpec.length > 0 ? ` --spec ${shellQuote(trimmedSpec)}` : "";
  const query = `@sim-flow /auto${projectFlag}${specFlag}`;
  await openChatWithQuery(query);
}

/**
 * Launch `sim-flow auto --llm-backend <name>` in the project's
 * terminal. Used by the dashboard's Run/Resume button when the user
 * has picked a CLI-agent source (`claude-cli`, `codex-cli`,
 * `gh-copilot-cli`) — those don't have an HTTP backend the chat
 * participant can drive, so we hand the work to the in-terminal
 * subprocess instead. Auth comes from the user's existing CLI
 * login (claude /login, codex login, gh auth login).
 */
async function runAutoInTerminal(
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
  await runCliInTerminal(sub, projectDirHint);
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
async function runFullyAutomatedInTerminal(
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
    await vscode.window.showErrorMessage(
      "sim-flow: fully-automated mode requires a spec path.",
    );
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
  const model = vscode.workspace
    .getConfiguration("sim-flow")
    .get<string>("llm.model")
    ?.trim();
  if (model && model.length > 0) {
    sub.push("--llm-model", model);
  }
  await runCliInTerminal(sub, projectDirHint);
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

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
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
    term = new SimFlowTerminal({
      projectDir,
      name: terminalNameFor(projectDir),
    });
    terminals.set(projectDir, term);
  }
  return term;
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
async function testLmModel(_context: vscode.ExtensionContext): Promise<void> {
  void _context;
  const channel = vscode.window.createOutputChannel("sim-flow: LM test");
  channel.show(true);
  const log = (line: string): void => {
    channel.appendLine(line);
  };
  log(`# LM model probe ${new Date().toISOString()}`);
  log("");

  const config = vscode.workspace.getConfiguration("sim-flow");
  const modelHint = config.get<string>("llm.model")?.trim() ?? "";
  log(`sim-flow.llm.model: \`${modelHint || "(empty -> any)"}\``);

  // Same vendor/family parsing the real backend uses.
  const idx = modelHint.indexOf("/");
  const vendor = idx >= 0 ? modelHint.slice(0, idx).trim() : undefined;
  const family = idx >= 0 ? modelHint.slice(idx + 1).trim() : modelHint || undefined;
  const selector: vscode.LanguageModelChatSelector = {};
  if (vendor) {selector.vendor = vendor;}
  if (family) {selector.family = family;}

  let models: vscode.LanguageModelChat[];
  try {
    models = await vscode.lm.selectChatModels(selector);
  } catch (err) {
    log(`selectChatModels threw: ${(err as Error).message ?? String(err)}`);
    return;
  }
  log(`selectChatModels returned ${models.length} model(s).`);
  if (models.length === 0) {
    log("Empty result. Try clearing `sim-flow.llm.model` or running `List Available Language Models`.");
    return;
  }
  const model = models[0];
  log(`Using: id=${model.id} vendor=${model.vendor} family=${model.family} maxInputTokens=${model.maxInputTokens}`);
  log("");

  type ProbeResult = { ok: boolean; partCount: number; durationMs: number; sample?: string };
  const probes: Array<{ label: string; run: () => Promise<ProbeResult> }> = [
    {
      label: "A. stream(), no modelOptions",
      run: () => probeStream(model, log, undefined),
    },
    {
      label: "B. text(), no modelOptions",
      run: () => probeText(model, log, undefined),
    },
    {
      label: "C. stream(), modelOptions={max_tokens: 4096}",
      run: () => probeStream(model, log, { max_tokens: 4096 }),
    },
  ];

  // Compare against a known-working copilot model so we can tell
  // whether an empty stream is provider-specific or our plumbing.
  let copilotModel: vscode.LanguageModelChat | undefined;
  try {
    const copilotMatches = await vscode.lm.selectChatModels({ vendor: "copilot" });
    copilotModel = copilotMatches.find((m) => m.family === "gpt-4o") ?? copilotMatches[0];
  } catch {
    // ignore; copilot may not be installed
  }
  if (copilotModel) {
    probes.push({
      label: `D. stream() against ${copilotModel.vendor}/${copilotModel.family} (control)`,
      run: () => probeStream(copilotModel as vscode.LanguageModelChat, log, undefined),
    });
  } else {
    log("(no copilot model found for control probe)");
    log("");
  }

  for (const probe of probes) {
    log(`---- ${probe.label} ----`);
    let result: ProbeResult;
    try {
      result = await probe.run();
    } catch (err) {
      log(`  threw: ${(err as Error).message ?? String(err)}`);
      log("");
      continue;
    }
    log(
      `  ${result.ok ? "OK" : "EMPTY"} -- ${result.partCount} part(s) in ${result.durationMs}ms${result.sample ? ` :: ${result.sample}` : ""}`,
    );
    log("");
  }
  log("Interpretation:");
  log("  * If A and B both empty but D non-empty -> Claude Code's provider rejects sim-flow callers.");
  log("  * If C non-empty but A empty -> provider requires modelOptions.max_tokens; we'll plumb it.");
  log("  * If A and D both empty -> something in our code path is wrong (rebuild and retry).");
  log("  * If everything non-empty -> the failure was specific to a long step prompt (e.g. DM2d); collapse system messages.");
}

async function probeStream(
  model: vscode.LanguageModelChat,
  log: (line: string) => void,
  modelOptions: Record<string, unknown> | undefined,
): Promise<{ ok: boolean; partCount: number; durationMs: number; sample?: string }> {
  const tokenSource = new vscode.CancellationTokenSource();
  const opts: vscode.LanguageModelChatRequestOptions = {
    justification: "sim-flow: LM model probe",
  };
  if (modelOptions) {
    opts.modelOptions = modelOptions;
  }
  const startedAt = Date.now();
  const request = await model.sendRequest(
    [vscode.LanguageModelChatMessage.User("respond with the single word: hi")],
    opts,
    tokenSource.token,
  );
  const streamLike = (request as unknown as { stream?: AsyncIterable<unknown> }).stream;
  let partCount = 0;
  let sample: string | undefined;
  if (!streamLike) {
    log("  (request.stream not present)");
    return { ok: false, partCount: 0, durationMs: Date.now() - startedAt };
  }
  for await (const part of streamLike) {
    partCount++;
    if (partCount <= 2) {
      const ctor = (part as { constructor?: { name?: string } })?.constructor?.name ?? typeof part;
      const keys =
        part && typeof part === "object"
          ? Object.keys(part as object).join(", ")
          : "(scalar)";
      let preview = "";
      try {
        preview = JSON.stringify(part)?.slice(0, 96) ?? "";
      } catch {
        preview = "(unserializable)";
      }
      log(`  [${partCount}] ctor=${ctor} keys={${keys}} preview=${preview}`);
    }
    if (partCount === 1 && part && typeof part === "object" && "value" in part) {
      sample = String((part as { value: unknown }).value).slice(0, 32);
    }
  }
  return { ok: partCount > 0, partCount, durationMs: Date.now() - startedAt, sample };
}

async function probeText(
  model: vscode.LanguageModelChat,
  log: (line: string) => void,
  modelOptions: Record<string, unknown> | undefined,
): Promise<{ ok: boolean; partCount: number; durationMs: number; sample?: string }> {
  const tokenSource = new vscode.CancellationTokenSource();
  const opts: vscode.LanguageModelChatRequestOptions = {
    justification: "sim-flow: LM model probe",
  };
  if (modelOptions) {
    opts.modelOptions = modelOptions;
  }
  const startedAt = Date.now();
  const request = await model.sendRequest(
    [vscode.LanguageModelChatMessage.User("respond with the single word: hi")],
    opts,
    tokenSource.token,
  );
  let partCount = 0;
  let sample: string | undefined;
  for await (const fragment of request.text) {
    partCount++;
    if (partCount <= 2) {
      log(`  [${partCount}] text=${JSON.stringify(fragment).slice(0, 96)}`);
    }
    if (partCount === 1) {
      sample = String(fragment).slice(0, 32);
    }
  }
  return { ok: partCount > 0, partCount, durationMs: Date.now() - startedAt, sample };
}

/**
 * Dump every chat model the VS Code Language Model API exposes to a
 * dedicated output channel. Useful for verifying whether an
 * external extension (e.g. Anthropic's Claude Code extension) has
 * registered itself as a chat-model provider via
 * `vscode.lm.registerChatModelProvider`. If it has, the model shows
 * up here and sim-flow's existing `vscode` backend can use it
 * without any code change. If it doesn't, only Copilot models
 * (and possibly nothing) appear.
 */
async function dumpAvailableLmModels(): Promise<void> {
  const channel = vscode.window.createOutputChannel("sim-flow: LM models");
  channel.show(true);
  channel.appendLine(`# Available chat models (queried at ${new Date().toISOString()})`);
  channel.appendLine("");
  let models: vscode.LanguageModelChat[];
  try {
    models = await vscode.lm.selectChatModels({});
  } catch (err) {
    channel.appendLine(`ERROR: vscode.lm.selectChatModels({}) threw: ${String(err)}`);
    return;
  }
  if (models.length === 0) {
    channel.appendLine(
      "No chat models were returned. Install Copilot (or the Claude Code extension, " +
        "or any other extension that registers a chat-model provider via " +
        "`vscode.lm.registerChatModelProvider`) and re-run this command.",
    );
    return;
  }
  channel.appendLine(`Found ${models.length} model(s):\n`);
  for (const m of models) {
    channel.appendLine(`- id:               ${m.id}`);
    channel.appendLine(`  vendor:           ${m.vendor}`);
    channel.appendLine(`  family:           ${m.family}`);
    channel.appendLine(`  name:             ${m.name}`);
    channel.appendLine(`  version:          ${m.version}`);
    channel.appendLine(`  maxInputTokens:   ${m.maxInputTokens}`);
    channel.appendLine("");
  }
  channel.appendLine(
    "Tip: a `vendor` of `anthropic` (or `claude-code`) here means the Claude Code " +
      "extension is registered as a provider, and sim-flow's `vscode` source will " +
      "pick it up automatically. Constrain via `sim-flow.llm.model` (matches the " +
      "`family` field) if multiple models are available.",
  );
}
