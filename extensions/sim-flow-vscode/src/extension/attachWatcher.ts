/**
 * `sim-flow: Attach to Running Watcher` command implementation.
 *
 * Lives here so `extension.ts` stays under the refactor threshold.
 * The command is otherwise self-contained: it shells out to
 * `sim-flow watchers list --json`, presents a quick-pick, and hands
 * the chosen watcher off to the chat-panel provider that the caller
 * passes in. The dashboard re-open step is delegated to the
 * `openDashboardForProject` callback supplied by the caller.
 */

import * as fs from "node:fs";
import * as path from "node:path";

import * as vscode from "vscode";

import { bundledCandidates, resolveBinary } from "../cli";
import type { ChatPanelProvider } from "../chatPanel/host";

export interface WatcherEntry {
  pid: number;
  socket_path: string;
  project_dir: string;
  started_at: string;
  llm_backend: string;
  llm_model: string | null;
}

export interface AttachWatcherDeps {
  readonly chatPanelProvider: ChatPanelProvider | undefined;
  readonly openDashboardForProject: (projectDir: string) => Promise<void>;
}

export async function attachWatcherCommand(deps: AttachWatcherDeps): Promise<void> {
  const cliBinary = (() => {
    try {
      const setting = vscode.workspace.getConfiguration("sim-flow").get<string>("binaryPath");
      return resolveBinary({
        settingOverride: setting,
        bundledCandidates,
      });
    } catch (err) {
      void vscode.window.showErrorMessage(
        `sim-flow: cannot resolve sim-flow binary: ${(err as Error).message ?? String(err)}`,
      );
      return undefined;
    }
  })();
  if (!cliBinary) {
    return;
  }

  const { execFile } = await import("node:child_process");
  const stdout = await new Promise<string>((resolve, reject) => {
    execFile(cliBinary, ["watchers", "list", "--json"], { encoding: "utf8" }, (err, out) => {
      if (err) {
        reject(err);
        return;
      }
      resolve(out);
    });
  }).catch((err) => {
    void vscode.window.showErrorMessage(
      `sim-flow: \`watchers list\` failed: ${err.message ?? String(err)}`,
    );
    return undefined;
  });
  if (stdout === undefined) {
    return;
  }

  let entries: WatcherEntry[];
  try {
    entries = JSON.parse(stdout) as WatcherEntry[];
  } catch (err) {
    void vscode.window.showErrorMessage(
      `sim-flow: malformed JSON from \`watchers list\`: ${(err as Error).message ?? String(err)}`,
    );
    return;
  }

  if (entries.length === 0) {
    void vscode.window.showInformationMessage(
      "sim-flow: no live watchers. Start an orchestrator with `sim-flow auto --watch-socket <path>` (or pass `--watch-socket` through `e2e_manual` / your launcher of choice) to make it discoverable here.",
    );
    return;
  }

  const items: (vscode.QuickPickItem & { entry: WatcherEntry })[] = entries.map((e) => ({
    label: `pid ${e.pid} · ${path.basename(e.project_dir)}`,
    description: e.llm_model ? `${e.llm_backend}/${e.llm_model}` : e.llm_backend,
    detail: `${e.project_dir} · ${e.socket_path}`,
    entry: e,
  }));

  const picked = await vscode.window.showQuickPick(items, {
    title: "Attach to running sim-flow watcher",
    placeHolder: "Pick the orchestrator to observe (read-only)",
    matchOnDescription: true,
    matchOnDetail: true,
  });
  if (!picked) {
    return;
  }

  // Hand the chosen watcher off to the chat-panel provider so it
  // attaches a viewer SocketSessionPump (read-only) and the
  // dashboard's onActiveSessionChanged hook refreshes per-step
  // button gating + chat-panel composer state.
  //
  // Canonicalize the project path BEFORE handing it off. Reason:
  // VS Code's `vscode.window.activeTextEditor.document.uri.fsPath`
  // resolves macOS `/tmp -> /private/tmp` symlinks (and similar on
  // other platforms), so the chat-panel's per-message project
  // resolver returns the realpath. The watcher registry, however,
  // stores whatever string the orchestrator was launched with --
  // typically `/tmp/...` raw. If we don't canonicalize here, the
  // chat panel's `activePump.projectDir === context.projectDir`
  // check fails and the panel shows OFFLINE with the live viewer
  // pump invisible to it.
  if (!deps.chatPanelProvider) {
    void vscode.window.showErrorMessage(
      "sim-flow: chat panel not initialised; viewer attach unavailable.",
    );
    return;
  }
  const canonProjectDir = canonicalizePath(picked.entry.project_dir);
  await deps.chatPanelProvider.attachWatcherSession({
    socketPath: picked.entry.socket_path,
    projectDir: canonProjectDir,
    pid: picked.entry.pid,
    llmBackend: picked.entry.llm_backend,
    llmModel: picked.entry.llm_model,
  });
  // Also reveal the dashboard for that project so the user sees
  // step / critique / gate state alongside the chat panel.
  await deps.openDashboardForProject(canonProjectDir);
}

/**
 * Resolve symlinks (`fs.realpath`) so the path matches whatever
 * `vscode.window.activeTextEditor.document.uri.fsPath` produces for
 * a file under it. Falls back to the input on error (e.g. the
 * directory was just removed) -- viewer attach will still proceed
 * but the chat panel may render OFFLINE because of the path
 * mismatch.
 */
function canonicalizePath(p: string): string {
  try {
    return fs.realpathSync(p);
  } catch {
    return p;
  }
}
