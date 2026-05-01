// Resolve the active sim-flow project directory + CLI wrapper. Shared
// by the dashboard host and the chat participant so both see the same
// project scope and settings.

import { existsSync, statSync } from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";

import { SimFlowCli, SimFlowCliError, bundledCandidates, resolveBinary } from "./cli";

export interface ProjectContext {
  projectDir: string;
  cli: SimFlowCli;
}

export interface ResolveContextOptions {
  /**
   * Caller-supplied absolute project directory (e.g. from the chat
   * participant's `--project <path>` argument). If provided, the
   * resolver skips workspace-scanning and trusts the caller. The
   * directory must contain `.sim-flow/state.toml`; otherwise the
   * resolver shows an error and returns null.
   */
  projectDir?: string;
  /**
   * Show VS Code error notifications for resolution failures.
   * Defaults to true. The chat participant sets this to false so
   * it can render the error in the chat stream instead.
   */
  showErrors?: boolean;
}

/**
 * Resolve the current project and CLI wrapper.
 *
 * Resolution order:
 *   1. `options.projectDir` (the chat `--project` flag or a
 *      dashboard-passed project) — verified to have
 *      `.sim-flow/state.toml`, otherwise we error.
 *   2. Walk up from the active editor / workspace folder roots.
 *   3. Workspace-wide scan via `findProjectCandidates()`; if
 *      exactly one candidate exists we use it, if multiple exist
 *      we error and list them (the caller should pass `--project`).
 *
 * Returns `null` when no sim-flow project is available or the CLI
 * cannot be located; shows a VS Code notification describing the
 * reason so the caller does not have to. `showErrors` suppresses
 * the UI notification for callers that would rather surface errors
 * in-channel (e.g. the chat participant).
 */
export async function resolveContext(
  options: ResolveContextOptions = {},
): Promise<ProjectContext | null> {
  const showErrors = options.showErrors ?? true;
  const projectDir = await resolveAnyProjectDir(options, showErrors);
  if (!projectDir) {
    return null;
  }
  const binary = tryResolveBinary();
  if (!binary) {
    return null;
  }
  const cli = new SimFlowCli({
    binary,
    projectDir,
    foundationRoot: getStringSetting("foundationRoot", ""),
  });
  return { projectDir, cli };
}

async function resolveAnyProjectDir(
  options: ResolveContextOptions,
  showErrors: boolean,
): Promise<string | undefined> {
  if (options.projectDir) {
    const verified = verifyProjectDir(options.projectDir);
    if (!verified && showErrors) {
      void vscode.window.showErrorMessage(
        `sim-flow: --project path "${options.projectDir}" does not contain a .sim-flow/state.toml.`,
      );
    }
    return verified;
  }

  const walkUp = resolveProjectDir();
  if (walkUp) {
    return walkUp;
  }

  // Walk-up missed. Try a workspace scan.
  const candidates = await findProjectCandidates();
  if (candidates.length === 1) {
    return candidates[0];
  }
  if (candidates.length > 1) {
    if (showErrors) {
      void vscode.window.showErrorMessage(
        `sim-flow: multiple projects found in this workspace. Pass \`--project <path>\`: ${candidates.join(", ")}`,
      );
    }
    return undefined;
  }
  if (showErrors) {
    void vscode.window.showErrorMessage(
      "sim-flow: open a workspace folder that contains .sim-flow/state.toml to use this command.",
    );
  }
  return undefined;
}

/**
 * Walk up from the active editor (or the first workspace folder) to
 * find the nearest directory that contains `.sim-flow/state.toml`.
 */
export function resolveProjectDir(): string | undefined {
  const candidates: string[] = [];
  const active = vscode.window.activeTextEditor?.document.uri.fsPath;
  if (active) {
    candidates.push(active);
  }
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    candidates.push(folder.uri.fsPath);
  }
  for (const start of candidates) {
    const found = findUpwards(start);
    if (found) {
      return found;
    }
  }
  // Fallback: if only one workspace folder exists, use it even if
  // state.toml is missing; the caller can still initialize.
  const folders = vscode.workspace.workspaceFolders;
  if (folders && folders.length === 1) {
    return folders[0].uri.fsPath;
  }
  return undefined;
}

/**
/**
 * Enumerate every sim-flow project visible in the current workspace
 * (every directory under a workspace folder that contains
 * `.sim-flow/state.toml`). Results are absolute paths, deduplicated,
 * and sorted for stable QuickPick ordering. `templates/` subtrees
 * are excluded because sim-foundation ships template projects
 * (under `tools/sim-flow/templates/model-project/`) that are
 * scaffolding for `cargo generate`, not real projects.
 */
export async function findProjectCandidates(): Promise<string[]> {
  const found = new Set<string>();
  const uris = await vscode.workspace.findFiles(
    "**/.sim-flow/state.toml",
    "**/{node_modules,target,.git,templates}/**",
    200,
  );
  for (const uri of uris) {
    // state.toml lives at <projectDir>/.sim-flow/state.toml, so the
    // project root is two levels up from the file.
    const dir = path.dirname(path.dirname(uri.fsPath));
    if (!isTemplateDir(dir)) {
      found.add(dir);
    }
  }
  // Also include workspace folders themselves in case findFiles
  // skipped them (e.g. when `.sim-flow/` is at the folder root).
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const candidate = folder.uri.fsPath;
    if (!isTemplateDir(candidate) && existsSync(path.join(candidate, ".sim-flow", "state.toml"))) {
      found.add(candidate);
    }
  }
  return [...found].sort();
}

/**
 * Heuristic for skipping scaffold / template projects. We reject a
 * path when any of its segments equals `templates`, which covers the
 * current sim-foundation layout
 * (`<foundationRoot>/tools/sim-flow/templates/model-project/...`).
 */
function isTemplateDir(dir: string): boolean {
  return dir.split(path.sep).includes("templates");
}

/**
 * Prompt the user to pick a project from the given list. If only one
 * candidate exists, returns it immediately with no UI. Returns
 * `undefined` when the user cancels.
 */
export async function pickProject(candidates: string[]): Promise<string | undefined> {
  if (candidates.length === 0) {
    return undefined;
  }
  if (candidates.length === 1) {
    return candidates[0];
  }
  const items = candidates.map((dir) => ({
    label: workspaceRelativeLabel(dir),
    description: dir,
    dir,
  }));
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: "Select a sim-flow project",
    matchOnDescription: true,
  });
  return picked?.dir;
}

function workspaceRelativeLabel(dir: string): string {
  const folders = vscode.workspace.workspaceFolders ?? [];
  for (const folder of folders) {
    const root = folder.uri.fsPath;
    if (dir === root) {
      return folder.name;
    }
    if (dir.startsWith(root + path.sep)) {
      const rel = path.relative(root, dir);
      return `${folder.name}/${rel}`;
    }
  }
  return dir;
}

function verifyProjectDir(candidate: string): string | undefined {
  const abs = path.isAbsolute(candidate) ? candidate : path.resolve(candidate);
  if (existsSync(path.join(abs, ".sim-flow", "state.toml"))) {
    return abs;
  }
  return undefined;
}

function findUpwards(start: string): string | undefined {
  let current = path.resolve(start);
  // If the start path is a file, step up once to its directory.
  try {
    const stat = statSync(current);
    if (stat.isFile()) {
      current = path.dirname(current);
    }
  } catch {
    // start may not exist yet; treat as directory path anyway.
  }
  while (true) {
    if (existsSync(path.join(current, ".sim-flow", "state.toml"))) {
      return current;
    }
    const parent = path.dirname(current);
    if (parent === current) {
      return undefined;
    }
    current = parent;
  }
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

function getStringSetting(key: string, fallback: string): string {
  const value = vscode.workspace.getConfiguration("sim-flow").get<string>(key);
  return typeof value === "string" && value.length > 0 ? value : fallback;
}
