// Extension-side host for the live perf-data panel.
//
// MVP architecture: this host is a thin presenter that talks to the
// orchestrator (sim-flow CLI) for all data and actions. It does NOT
// open `experiments.db` directly -- runs flow in via
// `sim-flow runs --json`, and diffs flow in via `sim-flow diff`.
// The watcher fires on `experiments-db` change events as a hint to
// re-poll the CLI, not as a direct read trigger.
//
// One host per project (keyed off canonicalized project dir, same
// pattern the dashboard uses) so multiple open projects don't share
// state.

import { execFile } from "node:child_process";
import * as path from "node:path";
import * as vscode from "vscode";

import type { SimFlowCli } from "../cli/simflow";
import type { RunRow } from "../cli/types";
import { createStateWatcher, type SimFlowStateWatcher } from "../state/watcher";

import type {
  HostMessage,
  PerfPanelState,
  PerfRunRow,
  PerfStudyGroup,
  WebviewMessage,
} from "./messages";

const MAX_RUNS_PER_GROUP = 200;

export interface PerfPanelHostOptions {
  extensionUri: vscode.Uri;
  projectDir: string;
  cli: SimFlowCli;
  /** Path to the `sim-flow` binary -- forwarded to `sim-flow diff`
   * when the user requests a comparison. The diff subcommand isn't
   * yet exposed via `SimFlowCli` so we shell out directly. */
  binary: string;
}

export class PerfPanelHost {
  private panel: vscode.WebviewPanel | undefined;
  private watcher: SimFlowStateWatcher | undefined;
  private disposables: vscode.Disposable[] = [];
  private lastDiff: { markdown: string | null; status: string | null } = {
    markdown: null,
    status: null,
  };
  private refreshInFlight = false;

  constructor(private readonly opts: PerfPanelHostOptions) {}

  async open(): Promise<void> {
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active);
      void this.refresh();
      return;
    }

    this.panel = vscode.window.createWebviewPanel(
      "simFlow.perfPanel",
      `sim-flow Perf · ${path.basename(this.opts.projectDir)}`,
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [
          vscode.Uri.joinPath(this.opts.extensionUri, "dist", "webview"),
        ],
      },
    );

    this.panel.webview.html = this.renderHtml(this.panel.webview);

    this.disposables.push(
      this.panel.webview.onDidReceiveMessage(
        async (msg: WebviewMessage) => this.onWebviewMessage(msg),
      ),
      this.panel.onDidDispose(() => this.dispose()),
    );

    this.watcher = createStateWatcher(this.opts.projectDir);
    this.disposables.push(
      this.watcher.onDidChange((change) => {
        if (change.kind === "experiments-db") {
          void this.refresh();
        }
      }),
    );

    void this.refresh();
  }

  private dispose(): void {
    for (const d of this.disposables) {
      try {
        d.dispose();
      } catch {
        /* best-effort */
      }
    }
    this.disposables = [];
    this.watcher?.dispose();
    this.watcher = undefined;
    this.panel = undefined;
  }

  private async refresh(): Promise<void> {
    if (!this.panel || this.refreshInFlight) {
      return;
    }
    this.refreshInFlight = true;
    try {
      const state = await this.aggregate();
      const msg: HostMessage = { type: "state-update", state };
      void this.panel.webview.postMessage(msg);
    } finally {
      this.refreshInFlight = false;
    }
  }

  private async aggregate(): Promise<PerfPanelState> {
    // All run data flows through the CLI -- the extension never
    // reads experiments.db directly. The CLI is the model layer;
    // the extension is the view.
    let runs: RunRow[] = [];
    let databaseAbsent = false;
    try {
      runs = await this.opts.cli.runs({ limit: MAX_RUNS_PER_GROUP * 8 });
    } catch (err) {
      // `sim-flow runs --json` fails when the DB is missing. Treat
      // that as "no runs yet" rather than a hard error.
      databaseAbsent = isMissingDbError(err);
      if (!databaseAbsent) {
        // Surface unexpected failures in the panel's diff-status
        // line so the user sees what went wrong.
        this.lastDiff = {
          markdown: null,
          status: `runs fetch failed: ${(err as Error).message ?? String(err)}`,
        };
      }
    }
    const groups = groupByStudy(runs);
    return {
      projectDir: this.opts.projectDir,
      lastUpdated: new Date().toISOString(),
      totalRuns: runs.length,
      groups: groups.groups,
      ungrouped: groups.ungrouped,
      databaseAbsent,
      diffMarkdown: this.lastDiff.markdown,
      diffStatus: this.lastDiff.status,
    };
  }

  private async onWebviewMessage(msg: WebviewMessage): Promise<void> {
    switch (msg.type) {
      case "request-refresh":
        await this.refresh();
        return;
      case "run-diff":
        await this.runDiff(msg.lhs, msg.rhs);
        return;
    }
  }

  private async runDiff(lhs: string, rhs: string): Promise<void> {
    this.lastDiff = { markdown: null, status: `Running diff ${lhs} vs ${rhs}...` };
    void this.refresh();
    try {
      const out = await execFilePromise(this.opts.binary, [
        "--project",
        this.opts.projectDir,
        "diff",
        lhs,
        rhs,
      ]);
      this.lastDiff = {
        markdown: out,
        status: `Diff complete (${lhs} vs ${rhs}).`,
      };
    } catch (err) {
      this.lastDiff = {
        markdown: null,
        status: `Diff failed: ${(err as Error).message ?? String(err)}`,
      };
    }
    void this.refresh();
  }

  private renderHtml(webview: vscode.Webview): string {
    const nonce = nonceString();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(
        this.opts.extensionUri,
        "dist",
        "webview",
        "perfPanel",
        "panel.js",
      ),
    );
    const csp = [
      "default-src 'none'",
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
      `font-src ${webview.cspSource}`,
    ].join("; ");

    return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <title>sim-flow Perf</title>
    <style>
      body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); background: var(--vscode-editor-background); padding: 12px; margin: 0; }
      .header { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 8px; }
      .header h1 { margin: 0; font-size: 1.2em; }
      .header .meta { color: var(--vscode-descriptionForeground); font-size: 0.85em; }
      button { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: none; padding: 4px 10px; cursor: pointer; border-radius: 3px; }
      button:hover { background: var(--vscode-button-hoverBackground); }
      button:disabled { opacity: 0.5; cursor: not-allowed; }
      button.secondary { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
      .group { margin: 16px 0; border: 1px solid var(--vscode-panel-border); border-radius: 4px; }
      .group h2 { margin: 0; padding: 8px 12px; font-size: 1em; background: var(--vscode-editor-inactiveSelectionBackground); border-bottom: 1px solid var(--vscode-panel-border); }
      .group .count { color: var(--vscode-descriptionForeground); font-weight: normal; font-size: 0.85em; margin-left: 8px; }
      table { width: 100%; border-collapse: collapse; }
      th, td { text-align: left; padding: 4px 12px; font-size: 0.9em; vertical-align: top; }
      th { font-weight: 600; border-bottom: 1px solid var(--vscode-panel-border); }
      tr.run-row { cursor: pointer; }
      tr.run-row:hover { background: var(--vscode-list-hoverBackground); }
      tr.run-row.selected { background: var(--vscode-list-activeSelectionBackground); color: var(--vscode-list-activeSelectionForeground); }
      .empty { color: var(--vscode-descriptionForeground); padding: 16px; text-align: center; }
      .toolbar { display: flex; gap: 8px; align-items: center; margin: 8px 0 12px; }
      .toolbar .selection { color: var(--vscode-descriptionForeground); font-size: 0.85em; }
      .diff-panel { margin-top: 24px; border-top: 1px solid var(--vscode-panel-border); padding-top: 12px; }
      .diff-panel pre { background: var(--vscode-textCodeBlock-background); padding: 12px; overflow-x: auto; font-family: var(--vscode-editor-font-family); white-space: pre-wrap; }
      .diff-status { color: var(--vscode-descriptionForeground); font-size: 0.85em; margin-bottom: 8px; }
      code { font-family: var(--vscode-editor-font-family); }
    </style>
  </head>
  <body>
    <div id="app"><div class="empty">Loading...</div></div>
    <script nonce="${nonce}" src="${scriptUri.toString()}"></script>
  </body>
</html>`;
  }
}

function groupByStudy(runs: RunRow[]): {
  groups: PerfStudyGroup[];
  ungrouped: PerfRunRow[];
} {
  const byStudy = new Map<string, PerfRunRow[]>();
  const ungrouped: PerfRunRow[] = [];
  for (const row of runs) {
    const slim = trimRow(row);
    if (row.study) {
      const list = byStudy.get(row.study) ?? [];
      if (list.length < MAX_RUNS_PER_GROUP) {
        list.push(slim);
      }
      byStudy.set(row.study, list);
    } else if (ungrouped.length < MAX_RUNS_PER_GROUP) {
      ungrouped.push(slim);
    }
  }
  const groups: PerfStudyGroup[] = Array.from(byStudy.entries())
    .map(([name, rows]) => ({ name, runs: rows }))
    .sort((a, b) => a.name.localeCompare(b.name));
  return { groups, ungrouped };
}

function trimRow(r: RunRow): PerfRunRow {
  return {
    run_id: r.run_id,
    timestamp: r.timestamp,
    workload: r.workload,
    candidate: r.candidate,
    study: r.study,
    parent_run_id: r.parent_run_id,
    manifest_path: r.manifest_path,
    notes: r.notes,
    tags: r.tags,
  };
}

function nonceString(): string {
  let text = "";
  const possible =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 32; i += 1) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}

function isMissingDbError(err: unknown): boolean {
  const msg = (err as Error)?.message ?? String(err);
  return /experiments\.db|no such file|not found/i.test(msg);
}

function execFilePromise(binary: string, args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(binary, args, { encoding: "utf8" }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(stderr || err.message));
        return;
      }
      resolve(stdout);
    });
  });
}
